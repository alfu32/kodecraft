//! Conservative AST extraction for the source's remaining active grammars.
mod spec;
use crate::{ids::*, model::*};
use kode_core::{ByteOffset, DomainError, Result, WorkspaceGeneration, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{
    parser::{ParsedDocument, ParserSession},
    registry::GrammarRegistry,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::{Duration, Instant},
};
use tree_sitter::Node;

pub struct GenericSemanticAdapter {
    parser: ParserSession,
}
impl GenericSemanticAdapter {
    pub fn new(registry: &GrammarRegistry, language: &str) -> Result<Self> {
        let grammar = registry.get(language).ok_or_else(|| {
            DomainError::Unsupported(format!("native {language} grammar unavailable"))
        })?;
        Ok(Self {
            parser: ParserSession::new(grammar)?,
        })
    }
    pub fn extract(
        &mut self,
        path: &str,
        snapshot: &TextSnapshot,
        generation: WorkspaceGeneration,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        let parsed = self.parser.parse(snapshot, generation, cancel)?;
        Self::extract_parsed(path, &parsed, cancel)
    }
    pub fn extract_parsed(
        path: &str,
        parsed: &ParsedDocument,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        Builder::new(path, parsed, cancel).build()
    }
}
struct Builder<'a> {
    parsed: &'a ParsedDocument,
    cancel: &'a CancellationToken,
    started: Instant,
    visited: usize,
    delta: FileSemanticDelta,
    declarations: HashMap<SourceRange, SymbolId>,
    scopes: BTreeSet<ScopeId>,
    symbols: BTreeMap<SymbolId, usize>,
    occurrence_nodes: Vec<(Node<'a>, ScopeId)>,
}
impl<'a> Builder<'a> {
    fn new(path: &str, parsed: &'a ParsedDocument, cancel: &'a CancellationToken) -> Self {
        Self {
            parsed,
            cancel,
            started: Instant::now(),
            visited: 0,
            delta: super::empty_delta(
                path,
                parsed.grammar().id,
                parsed.source(),
                parsed.snapshot().revision.0,
            ),
            declarations: HashMap::new(),
            scopes: BTreeSet::new(),
            symbols: BTreeMap::new(),
            occurrence_nodes: vec![],
        }
    }
    fn check(&mut self, depth: usize) -> Result<()> {
        self.visited += 1;
        if self.cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if depth > 256
            || self.visited > 1_000_000
            || self.delta.symbols.len() > 100_000
            || self.delta.scopes.len() > 10_000
            || self.started.elapsed() > Duration::from_secs(2)
        {
            return Err(DomainError::Unsupported(
                "generic extraction work limit exceeded".into(),
            ));
        }
        Ok(())
    }
    fn add_scope(
        &mut self,
        node: Node<'_>,
        parent: Option<ScopeId>,
        owner: Option<SymbolId>,
        kind: ScopeKind,
    ) -> ScopeId {
        let id = SemanticIds::scope(self.delta.file.id, ByteOffset(node.start_byte()), kind);
        if self.scopes.insert(id) {
            self.delta.scopes.push(ScopeRecord {
                id,
                file_id: self.delta.file.id,
                parent_id: parent,
                owner_symbol_id: owner,
                kind,
                range: span(node),
            });
        }
        id
    }
    fn visit(
        &mut self,
        node: Node<'a>,
        mut scope: ScopeId,
        mut owner: Option<SymbolId>,
        depth: usize,
    ) -> Result<()> {
        self.check(depth)?;
        let declaration = spec::declaration(node, self.parsed.source());
        if let Some((name_node, mut kind)) = declaration {
            let name = &self.parsed.source()[name_node.byte_range()];
            if !spec::keyword(name) && !self.declarations.contains_key(&span(name_node)) {
                let parent =
                    owner.and_then(|id| self.symbols.get(&id).map(|i| &self.delta.symbols[*i]));
                let type_member = parent.is_some_and(|s| {
                    matches!(
                        s.kind,
                        SymbolKind::Class
                            | SymbolKind::Interface
                            | SymbolKind::Struct
                            | SymbolKind::Enum
                    )
                });
                if kind == SymbolKind::Function && type_member {
                    kind = SymbolKind::Method;
                }
                let qualified = parent
                    .and_then(|s| s.qualified_name.as_ref())
                    .map_or_else(|| name.to_owned(), |p| format!("{p}.{name}"));
                let id = SemanticIds::symbol(
                    self.delta.file.id,
                    ByteOffset(name_node.start_byte()),
                    kind,
                    name,
                );
                self.symbols.insert(id, self.delta.symbols.len());
                self.delta.symbols.push(SymbolRecord {
                    id,
                    file_id: self.delta.file.id,
                    name: name.into(),
                    qualified_name: Some(qualified),
                    kind,
                    declaration_range: span(node),
                    name_range: span(name_node),
                    scope_id: scope,
                    owner_symbol_id: owner,
                    declared_type_id: None,
                    inferred_type_id: None,
                    flags: 0,
                    confidence: Confidence::tree_sitter(),
                });
                self.declarations.insert(span(name_node), id);
                if !spec::identifier(name_node) {
                    self.occurrence_nodes.push((name_node, scope));
                }
                if let Some(parent) = owner
                    && type_member
                {
                    self.delta.relations.extend([
                        RelationRecord {
                            from: parent,
                            to: id,
                            kind: RelationKind::Contains,
                            confidence: Confidence::tree_sitter(),
                        },
                        RelationRecord {
                            from: id,
                            to: parent,
                            kind: RelationKind::MemberOf,
                            confidence: Confidence::tree_sitter(),
                        },
                    ]);
                }
                // A variable/property/parameter is not a declaration scope.
                if let Some(kind) = spec::scope(node.kind()) {
                    scope = self.add_scope(node, Some(scope), Some(id), kind);
                    owner = Some(id);
                }
            }
        } else if let Some(kind) = spec::scope(node.kind()) {
            scope = self.add_scope(node, Some(scope), owner, kind);
        }
        if spec::identifier(node) && !spec::keyword(&self.parsed.source()[node.byte_range()]) {
            self.occurrence_nodes.push((node, scope));
        }
        self.collect_import(node, scope);
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            self.visit(child, scope, owner, depth + 1)?;
        }
        Ok(())
    }
    fn collect_import(&mut self, node: Node<'_>, scope: ScopeId) {
        if !matches!(
            node.kind(),
            "import_statement"
                | "import_declaration"
                | "import_from_statement"
                | "import_spec"
                | "use_declaration"
        ) {
            return;
        }
        let raw = &self.parsed.source()[node.byte_range()];
        let path = node
            .child_by_field_name("source")
            .or_else(|| node.child_by_field_name("path"))
            .map(|n| &self.parsed.source()[n.byte_range()])
            .unwrap_or_else(|| {
                raw.strip_prefix("import")
                    .or_else(|| raw.strip_prefix("use"))
                    .unwrap_or(raw)
            });
        let path = path
            .trim()
            .trim_end_matches(';')
            .trim()
            .trim_matches(['\'', '"']);
        if !path.is_empty() {
            self.delta.imports.push(ImportRecord {
                file_id: self.delta.file.id,
                scope_id: scope,
                path: path.into(),
                alias: None,
                wildcard: path.ends_with(".*"),
                range: span(node),
            });
        }
    }
    fn build(mut self) -> Result<FileSemanticDelta> {
        let root = self.parsed.root_node();
        let file_scope = self.add_scope(root, None, None, ScopeKind::File);
        self.visit(root, file_scope, None, 0)?;
        let occurrence_ids: BTreeSet<_> = self
            .occurrence_nodes
            .iter()
            .map(|(n, _)| SemanticIds::occurrence(self.delta.file.id, span(*n)))
            .collect();
        let declaration_scopes: BTreeMap<_, _> = self
            .delta
            .symbols
            .iter()
            .map(|s| (s.id, s.scope_id))
            .collect();
        for (node, scope) in self.occurrence_nodes.clone() {
            self.check(0)?;
            let range = span(node);
            let declaration = self.declarations.get(&range).copied();
            let mut kind = if declaration.is_some() {
                OccurrenceKind::Declaration
            } else {
                OccurrenceKind::Unknown
            };
            let mut receiver = None;
            if declaration.is_none()
                && let Some(parent) = node.parent()
            {
                if matches!(
                    parent.kind(),
                    "member_expression" | "field_expression" | "attribute"
                ) {
                    let property = parent
                        .child_by_field_name("property")
                        .or_else(|| parent.child_by_field_name("field"))
                        .or_else(|| parent.child_by_field_name("attribute"));
                    if property == Some(node) {
                        kind = OccurrenceKind::MemberReference;
                        receiver = parent
                            .child_by_field_name("object")
                            .or_else(|| parent.child_by_field_name("argument"))
                            .and_then(super::receiver_node)
                            .map(|n| SemanticIds::occurrence(self.delta.file.id, span(n)))
                            .filter(|id| occurrence_ids.contains(id));
                    }
                } else if parent.kind() == "call_expression"
                    && parent.child_by_field_name("function") == Some(node)
                {
                    kind = OccurrenceKind::Call;
                } else if matches!(node.kind(), "type_identifier") {
                    kind = OccurrenceKind::TypeReference;
                }
            }
            self.delta.occurrences.push(OccurrenceRecord {
                id: SemanticIds::occurrence(self.delta.file.id, range),
                file_id: self.delta.file.id,
                range,
                text: self.parsed.source()[node.byte_range()].into(),
                kind,
                scope_id: declaration
                    .and_then(|id| declaration_scopes.get(&id).copied())
                    .unwrap_or(scope),
                resolved_symbol_id: declaration,
                receiver_occurrence_id: receiver,
                confidence: Confidence::tree_sitter(),
            });
        }
        for token in self.parsed.lexical() {
            let kind = match token.class.as_str() {
                "keyword" => LexicalTokenKind::Keyword,
                "string" => LexicalTokenKind::String,
                "number" => LexicalTokenKind::Number,
                "comment" => LexicalTokenKind::Comment,
                "operator" => LexicalTokenKind::Operator,
                _ => continue,
            };
            self.delta.lexical_tokens.push(LexicalTokenRecord {
                file_id: self.delta.file.id,
                range: token.range,
                kind,
                confidence: Confidence::tree_sitter(),
            });
        }
        self.delta
            .symbols
            .sort_by_key(|s| (s.name_range.start, s.id));
        self.delta.scopes.sort_by_key(|s| (s.range.start, s.id));
        self.delta
            .occurrences
            .sort_by_key(|o| (o.range.start, o.range.end));
        self.delta.occurrences.dedup_by_key(|o| o.id);
        self.delta.imports.sort_by_key(|i| i.range.start);
        self.delta.imports.dedup();
        self.delta.exported_surface_hash = public_surface_hash(&self.delta);
        self.delta.validate_source(self.parsed.source())?;
        Ok(self.delta)
    }
}
fn span(node: Node<'_>) -> SourceRange {
    SourceRange {
        start: ByteOffset(node.start_byte()),
        end: ByteOffset(node.end_byte()),
    }
}
