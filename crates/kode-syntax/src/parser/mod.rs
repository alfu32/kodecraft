//! Stateful worker parser with immutable results and an explicit work budget.
mod incremental;
mod kotlin;
use crate::{model::SyntaxSpan, regex_grammar::color_for_qualifier, registry::Grammar};
use kode_core::{
    ByteOffset, DomainError, Result, TextRange, WorkspaceGeneration, document::TextSnapshot,
};
use kode_platform::jobs::CancellationToken;
use serde::{Deserialize, Serialize};
use std::{
    ops::ControlFlow,
    sync::Arc,
    time::{Duration, Instant},
};
use tree_sitter::{Node, ParseOptions, Parser, Tree};
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NODES: usize = 1_000_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseDiagnostic {
    pub range: TextRange,
    pub kind: String,
    pub missing: bool,
}
#[derive(Clone)]
pub struct ParsedDocument(Arc<ParsedData>);
struct ParsedData {
    generation: WorkspaceGeneration,
    snapshot: TextSnapshot,
    source: Arc<str>,
    input: Arc<str>,
    tree: Tree,
    grammar: Arc<Grammar>,
    lexical: Vec<SyntaxSpan>,
    diagnostics: Vec<ParseDiagnostic>,
    changed: Vec<TextRange>,
}
impl ParsedDocument {
    pub fn snapshot(&self) -> &TextSnapshot {
        &self.0.snapshot
    }
    pub fn generation(&self) -> WorkspaceGeneration {
        self.0.generation
    }
    pub fn source(&self) -> &str {
        &self.0.source
    }
    pub fn tree(&self) -> &Tree {
        &self.0.tree
    }
    pub fn root_node(&self) -> Node<'_> {
        self.tree().root_node()
    }
    pub fn grammar(&self) -> &Grammar {
        &self.0.grammar
    }
    pub fn lexical(&self) -> &[SyntaxSpan] {
        &self.0.lexical
    }
    pub fn diagnostics(&self) -> &[ParseDiagnostic] {
        &self.0.diagnostics
    }
    pub fn has_errors(&self) -> bool {
        self.root_node().has_error() || !self.diagnostics().is_empty()
    }
    pub fn changed_ranges(&self) -> &[TextRange] {
        &self.0.changed
    }
}
pub struct ParserSession {
    grammar: Arc<Grammar>,
    parser: Parser,
    previous: Option<ParsedDocument>,
    budget: Duration,
}
impl ParserSession {
    pub fn last_document(&self) -> Option<&ParsedDocument> {
        self.previous.as_ref()
    }
    pub fn new(grammar: Arc<Grammar>) -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&grammar.language).map_err(|error| {
            DomainError::Unsupported(format!("{} grammar ABI failed: {error}", grammar.id))
        })?;
        Ok(Self {
            grammar,
            parser,
            previous: None,
            budget: Duration::from_secs(2),
        })
    }
    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }
    pub fn parse(
        &mut self,
        snapshot: &TextSnapshot,
        generation: WorkspaceGeneration,
        cancel: &CancellationToken,
    ) -> Result<ParsedDocument> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if snapshot.len_bytes() > MAX_SOURCE_BYTES {
            return Err(DomainError::Unsupported(
                "native parser input exceeds 16 MiB".into(),
            ));
        }
        let started = Instant::now();
        let source = snapshot.text();
        let (input, recovery) = if self.grammar.id == "kotlin" {
            kotlin::prepare(source.clone(), cancel)?
        } else {
            (source.clone(), vec![])
        };
        let previous = self.previous.as_ref().filter(|old| {
            old.generation() == generation
                && old.snapshot().id == snapshot.id
                && old.snapshot().origin == snapshot.origin
        });
        if let Some(previous) = previous
            && previous.snapshot().revision == snapshot.revision
            && (Arc::ptr_eq(&source, &previous.0.source) || source == previous.0.source)
        {
            return Ok(previous.clone());
        }
        let mut old_tree = previous.map(|old| old.tree().clone());
        if let (Some(old_tree), Some(previous)) = (&mut old_tree, previous)
            && let Some(edit) = incremental::edit(&previous.0.input, &input)
        {
            old_tree.edit(&edit);
        }
        let mut progress = |_: &tree_sitter::ParseState| {
            if cancel.is_cancelled() || started.elapsed() >= self.budget {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        if started.elapsed() >= self.budget {
            return Err(DomainError::Unsupported(
                "native parse time budget exceeded".into(),
            ));
        }
        let mut read =
            |byte: usize, _: tree_sitter::Point| input.as_bytes().get(byte..).unwrap_or_default();
        let tree = self.parser.parse_with_options(
            &mut read,
            old_tree.as_ref(),
            Some(ParseOptions::new().progress_callback(&mut progress)),
        );
        let Some(tree) = tree else {
            self.parser.reset();
            return Err(if cancel.is_cancelled() {
                DomainError::Cancelled
            } else {
                DomainError::Unsupported(
                    "native parse time budget exceeded; regex fallback remains available".into(),
                )
            });
        };
        if cancel.is_cancelled() {
            self.parser.reset();
            return Err(DomainError::Cancelled);
        }
        let changed = old_tree.as_ref().map_or_else(
            || {
                vec![
                    TextRange::new(ByteOffset(0), ByteOffset(source.len()))
                        .expect("whole source range"),
                ]
            },
            |old| {
                old.changed_ranges(&tree)
                    .map(|range| {
                        TextRange::new(ByteOffset(range.start_byte), ByteOffset(range.end_byte))
                    })
                    .collect::<Result<Vec<_>>>()
                    .expect("tree ranges are ordered")
            },
        );
        if tree.root_node().descendant_count() > MAX_NODES {
            return Err(DomainError::Unsupported(
                "native syntax tree exceeds node limit".into(),
            ));
        }
        let (lexical, mut diagnostics) = collect(&tree, &source, cancel, started, self.budget)?;
        diagnostics.extend(recovery);
        diagnostics.truncate(256);
        let parsed = ParsedDocument(Arc::new(ParsedData {
            generation,
            snapshot: snapshot.clone(),
            source,
            input,
            tree,
            grammar: self.grammar.clone(),
            lexical,
            diagnostics,
            changed,
        }));
        self.previous = Some(parsed.clone());
        Ok(parsed)
    }
}
fn collect(
    tree: &Tree,
    source: &str,
    cancel: &CancellationToken,
    started: Instant,
    budget: Duration,
) -> Result<(Vec<SyntaxSpan>, Vec<ParseDiagnostic>)> {
    let mut cursor = tree.walk();
    let mut spans = vec![];
    let mut diagnostics = vec![];
    let mut visited = 0;
    loop {
        visited += 1;
        if visited > MAX_NODES {
            return Err(DomainError::Unsupported(
                "native syntax tree exceeds node limit".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if started.elapsed() >= budget {
            return Err(DomainError::Unsupported(
                "native lexical scan time budget exceeded".into(),
            ));
        }
        let node = cursor.node();
        let range = TextRange::new(ByteOffset(node.start_byte()), ByteOffset(node.end_byte()))?
            .validate(source)?;
        if (node.is_error() || node.is_missing()) && diagnostics.len() < 256 {
            diagnostics.push(ParseDiagnostic {
                range,
                kind: node.kind().into(),
                missing: node.is_missing(),
            });
        }
        let class = lexical_class(node, source);
        if let Some(class) = class
            && !range.is_empty()
        {
            spans.push(SyntaxSpan {
                range,
                class: class.into(),
                color: color_for_qualifier(class),
            });
        }
        if class.is_none() && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok((spans, diagnostics));
            }
        }
    }
}
fn lexical_class(node: Node<'_>, source: &str) -> Option<&'static str> {
    let kind = node.kind();
    if kind.contains("comment") || kind == "shebang" {
        return Some("comment");
    }
    if matches!(
        kind,
        "string"
            | "string_literal"
            | "raw_string_literal"
            | "interpreted_string_literal"
            | "template_string"
            | "character_literal"
            | "char_literal"
            | "quoted_attribute_value"
    ) || kind.ends_with("_string_literal")
    {
        return Some("string");
    }
    if (kind.contains("number") || kind.contains("integer") || kind.contains("float"))
        && !kind.ends_with("_type")
        && !kind.ends_with("_expression")
        || matches!(
            kind,
            "decimal_literal"
                | "real_literal"
                | "long_literal"
                | "hex_literal"
                | "bin_literal"
                | "unsigned_literal"
        )
    {
        return Some("number");
    }
    if matches!(
        kind,
        "annotation" | "annotation_marker" | "decorator" | "attribute_item"
    ) {
        return Some("annotation");
    }
    if kind == "lifetime" {
        return Some("lifetime");
    }
    if !node.is_named() {
        let text = source.get(node.byte_range())?;
        if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_alphabetic() || ch == '_') {
            return Some("keyword");
        }
        if matches!(
            text,
            "+" | "-"
                | "*"
                | "/"
                | "="
                | "=="
                | "!="
                | "<"
                | ">"
                | "<="
                | ">="
                | "&&"
                | "||"
                | "!"
                | "&"
                | "|"
                | "^"
                | "%"
                | "->"
                | "=>"
        ) {
            return Some("operator");
        }
    }
    None
}
