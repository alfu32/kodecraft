use crate::{
    ids::{ScopeId, SemanticIds, SymbolId, public_surface_hash},
    model::*,
};
use kode_core::{ByteOffset, DomainError, Result, TextRange};
use kode_platform::jobs::CancellationToken;
use kode_syntax::parser::ParsedDocument;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tree_sitter::Node;
pub(super) struct Builder<'a> {
    parsed: &'a ParsedDocument,
    source: &'a str,
    cancel: &'a CancellationToken,
    started: Instant,
    visits: usize,
    delta: FileSemanticDelta,
    symbols: HashMap<SymbolId, usize>,
    scopes: HashMap<ScopeId, usize>,
    declarations: HashMap<TextRange, SymbolId>,
    contexts: HashMap<usize, ScopeId>,
}
impl<'a> Builder<'a> {
    pub(super) fn new(
        path: &str,
        parsed: &'a ParsedDocument,
        cancel: &'a CancellationToken,
    ) -> Self {
        Self {
            parsed,
            source: parsed.source(),
            cancel,
            started: Instant::now(),
            visits: 0,
            delta: FileSemanticDelta {
                file: FileRecord {
                    id: SemanticIds::file(path),
                    path: path.into(),
                    language_id: "c".into(),
                    content_hash: SemanticIds::content_hash(parsed.source()),
                    parse_version: parsed.snapshot().revision.0,
                    semantic_version: parsed.snapshot().revision.0,
                    dependency_generation: 0,
                },
                scopes: vec![],
                symbols: vec![],
                occurrences: vec![],
                relations: vec![],
                unresolved_types: vec![],
                imports: vec![],
                type_hints: vec![],
                expression_types: vec![],
                lexical_tokens: vec![],
                syntax_expressions: vec![],
                exported_surface_hash: String::new(),
            },
            symbols: HashMap::new(),
            scopes: HashMap::new(),
            declarations: HashMap::new(),
            contexts: HashMap::new(),
        }
    }
    fn check(&mut self, depth: usize) -> Result<()> {
        self.visits += 1;
        if self.cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if depth > 256
            || self.visits > 2_000_000
            || self.started.elapsed() > Duration::from_secs(2)
            || self.delta.symbols.len() > 100_000
            || self.delta.scopes.len() > 10_000
        {
            return Err(DomainError::Unsupported(
                "C extraction work limit exceeded".into(),
            ));
        }
        Ok(())
    }
    fn text(&self, n: Node<'_>) -> &'a str {
        &self.source[n.byte_range()]
    }
    fn add_scope(
        &mut self,
        n: Node<'_>,
        kind: ScopeKind,
        parent: Option<ScopeId>,
        owner: Option<SymbolId>,
    ) -> ScopeId {
        let id = SemanticIds::scope(self.delta.file.id, ByteOffset(n.start_byte()), kind);
        if !self.scopes.contains_key(&id) {
            self.scopes.insert(id, self.delta.scopes.len());
            self.delta.scopes.push(ScopeRecord {
                id,
                file_id: self.delta.file.id,
                parent_id: parent,
                owner_symbol_id: owner,
                kind,
                range: range(n),
            });
        }
        self.contexts.insert(n.id(), id);
        id
    }
    fn declare(
        &mut self,
        node: Node<'_>,
        name: Option<Node<'_>>,
        kind: SymbolKind,
        scope: ScopeId,
        owner: Option<SymbolId>,
    ) -> SymbolId {
        let name_range = name.map_or_else(
            || {
                TextRange::new(ByteOffset(node.start_byte()), ByteOffset(node.start_byte()))
                    .expect("empty range")
            },
            range,
        );
        let text = name.map_or_else(
            || format!("__anonymous_at_{}", node.start_byte()),
            |n| self.text(n).to_owned(),
        );
        let id = SemanticIds::symbol(self.delta.file.id, name_range.start, kind, &text);
        if self.symbols.contains_key(&id) {
            return id;
        }
        let qualified_name = owner
            .and_then(|o| {
                self.delta.symbols[self.symbols[&o]]
                    .qualified_name
                    .as_deref()
            })
            .map_or_else(|| text.clone(), |parent| format!("{parent}.{text}"));
        self.symbols.insert(id, self.delta.symbols.len());
        self.delta.symbols.push(SymbolRecord {
            id,
            file_id: self.delta.file.id,
            name: text,
            qualified_name: Some(qualified_name),
            kind,
            declaration_range: range(node),
            name_range,
            scope_id: scope,
            owner_symbol_id: owner,
            declared_type_id: None,
            inferred_type_id: None,
            flags: 0,
            confidence: Confidence::tree_sitter(),
        });
        if name.is_some() {
            self.declarations.insert(name_range, id);
        }
        if let Some(owner) = owner
            && !matches!(kind, SymbolKind::Variable | SymbolKind::Parameter)
        {
            for (from, to, kind) in [
                (owner, id, RelationKind::Contains),
                (id, owner, RelationKind::MemberOf),
            ] {
                self.delta.relations.push(RelationRecord {
                    from,
                    to,
                    kind,
                    confidence: Confidence::inferred(0.95).expect("constant"),
                });
            }
        }
        id
    }
    fn type_name(&self, n: Node<'_>) -> String {
        if let Some(prefix) = match n.kind() {
            "struct_specifier" => Some("struct"),
            "union_specifier" => Some("union"),
            "enum_specifier" => Some("enum"),
            _ => None,
        } {
            let tag = n.child_by_field_name("name").map_or_else(
                || format!("__anonymous_at_{}", n.start_byte()),
                |n| self.text(n).to_owned(),
            );
            format!("{prefix} {tag}")
        } else {
            self.text(n).to_owned()
        }
    }
    fn add_type(
        &mut self,
        owner: SymbolId,
        scope: ScopeId,
        ty: Node<'_>,
        declarator: Option<Node<'_>>,
        role: TypeRole,
    ) {
        let mut name = self.type_name(ty);
        if let Some(declarator) = declarator {
            let stars = pointer_depth(declarator);
            for _ in 0..stars {
                name.push_str(" *");
            }
        }
        self.delta.unresolved_types.push(UnresolvedTypeRef {
            owner_symbol_id: owner,
            scope_id: scope,
            name,
            role,
            range: range(ty),
            confidence: Confidence::tree_sitter(),
        });
    }
    pub(super) fn build(mut self) -> Result<FileSemanticDelta> {
        self.check(0)?;
        let root = self.parsed.root_node();
        let scope = self.add_scope(root, ScopeKind::File, None, None);
        self.visit(root, scope, None, 0)?;
        self.collect_occurrences(root, scope, 0)?;
        self.delta.syntax_expressions = crate::frontend::expressions::collect(
            self.parsed,
            &self.delta,
            &self.contexts,
            &HashMap::new(),
            self.cancel,
        )?;
        self.delta
            .scopes
            .sort_by_key(|s| (s.range.start, s.range.end, s.kind));
        self.delta
            .symbols
            .sort_by_key(|s| (s.name_range.start, s.kind));
        self.delta
            .occurrences
            .sort_by_key(|o| (o.range.start, o.range.end));
        self.delta
            .lexical_tokens
            .sort_by_key(|o| (o.range.start, o.range.end));
        self.delta.exported_surface_hash = public_surface_hash(&self.delta);
        self.delta.validate_source(self.source)?;
        Ok(self.delta)
    }
    fn visit(
        &mut self,
        n: Node<'a>,
        mut scope: ScopeId,
        mut owner: Option<SymbolId>,
        depth: usize,
    ) -> Result<()> {
        self.check(depth)?;
        let mut child_contexts = HashMap::new();
        match n.kind() {
            "struct_specifier" | "union_specifier" | "enum_specifier"
                if n.child_by_field_name("body").is_some() =>
            {
                let kind = if n.kind() == "enum_specifier" {
                    SymbolKind::Enum
                } else {
                    SymbolKind::Struct
                };
                let id = self.declare(n, n.child_by_field_name("name"), kind, scope, owner);
                scope = self.add_scope(n, ScopeKind::Type, Some(scope), Some(id));
                owner = Some(id);
            }
            "function_definition" => {
                let declarator = n.child_by_field_name("declarator");
                if let Some(name) = declarator.and_then(declarator_name) {
                    let id = self.declare(n, Some(name), SymbolKind::Function, scope, owner);
                    if let Some(ty) = n.child_by_field_name("type") {
                        self.add_type(id, scope, ty, declarator, TypeRole::Return);
                    }
                    scope = self.add_scope(n, ScopeKind::Function, Some(scope), Some(id));
                    owner = Some(id);
                }
            }
            "compound_statement" | "for_statement" => {
                scope = self.add_scope(n, ScopeKind::Block, Some(scope), owner);
            }
            "declaration" | "field_declaration" | "parameter_declaration" | "type_definition" => {
                let kind = match n.kind() {
                    "type_definition" => SymbolKind::TypeAlias,
                    "field_declaration" => SymbolKind::Field,
                    "parameter_declaration" => SymbolKind::Parameter,
                    _ => SymbolKind::Variable,
                };
                let mut cursor = n.walk();
                for declarator in n.children_by_field_name("declarator", &mut cursor) {
                    self.check(depth)?;
                    let Some(name) = declarator_name(declarator) else {
                        continue;
                    };
                    let function = kind == SymbolKind::Variable && is_function(declarator);
                    let declared_kind = if function { SymbolKind::Function } else { kind };
                    let id = self.declare(n, Some(name), declared_kind, scope, owner);
                    if let Some(ty) = n.child_by_field_name("type") {
                        let role = if function {
                            TypeRole::Return
                        } else if kind == SymbolKind::Parameter {
                            TypeRole::Parameter
                        } else {
                            TypeRole::Declared
                        };
                        self.add_type(id, scope, ty, Some(declarator), role);
                    }
                    if function {
                        let child_scope =
                            self.add_scope(declarator, ScopeKind::Function, Some(scope), Some(id));
                        child_contexts.insert(declarator.id(), (child_scope, Some(id)));
                    }
                }
            }
            "enumerator" => {
                let declaration_scope = self
                    .scopes
                    .get(&scope)
                    .and_then(|index| self.delta.scopes[*index].parent_id)
                    .unwrap_or(scope);
                self.declare(
                    n,
                    n.child_by_field_name("name"),
                    SymbolKind::EnumMember,
                    declaration_scope,
                    owner,
                );
            }
            "preproc_include" => {
                if let Some(path) = n.child_by_field_name("path") {
                    self.delta.imports.push(ImportRecord {
                        file_id: self.delta.file.id,
                        scope_id: scope,
                        path: self.text(path).trim_matches(['"', '<', '>']).into(),
                        alias: None,
                        wildcard: false,
                        range: range(n),
                    });
                }
            }
            _ => {}
        }
        self.contexts.entry(n.id()).or_insert(scope);
        let kind = if n.kind().contains("comment") {
            Some(LexicalTokenKind::Comment)
        } else if matches!(
            n.kind(),
            "string_literal" | "char_literal" | "system_lib_string"
        ) {
            Some(LexicalTokenKind::String)
        } else if n.kind() == "number_literal" {
            Some(LexicalTokenKind::Number)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.delta.lexical_tokens.push(LexicalTokenRecord {
                file_id: self.delta.file.id,
                range: range(n),
                kind,
                confidence: Confidence::tree_sitter(),
            });
        }
        for child in children(n) {
            let (child_scope, child_owner) = child_contexts
                .get(&child.id())
                .copied()
                .unwrap_or((scope, owner));
            self.visit(child, child_scope, child_owner, depth + 1)?;
        }
        Ok(())
    }
    fn collect_occurrences(
        &mut self,
        n: Node<'a>,
        parent_scope: ScopeId,
        depth: usize,
    ) -> Result<()> {
        self.check(depth)?;
        let scope = self.contexts.get(&n.id()).copied().unwrap_or(parent_scope);
        if n.kind().contains("comment")
            || matches!(
                n.kind(),
                "string_literal" | "char_literal" | "system_lib_string"
            )
        {
            return Ok(());
        }
        if matches!(
            n.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) && !n.is_missing()
            && n.end_byte() > n.start_byte()
        {
            let span = range(n);
            let declaration = self.declarations.get(&span).copied();
            let parent = n.parent();
            let receiver = parent
                .filter(|p| {
                    p.kind() == "field_expression"
                        && p.child_by_field_name("field")
                            .is_some_and(|f| f.id() == n.id())
                })
                .and_then(|p| p.child_by_field_name("argument"));
            let receiver_id = receiver
                .and_then(crate::frontend::receiver_node)
                .and_then(|r| {
                    self.delta
                        .occurrences
                        .iter()
                        .rev()
                        .find(|o| o.range == range(r))
                        .map(|o| o.id)
                });
            let kind = if declaration.is_some() {
                OccurrenceKind::Declaration
            } else if receiver.is_some() {
                OccurrenceKind::MemberReference
            } else if n.kind() == "type_identifier" {
                OccurrenceKind::TypeReference
            } else if parent.is_some_and(|p| {
                p.kind() == "call_expression"
                    && p.child_by_field_name("function")
                        .is_some_and(|f| f.id() == n.id())
            }) {
                OccurrenceKind::Call
            } else {
                OccurrenceKind::Read
            };
            self.delta.occurrences.push(OccurrenceRecord {
                id: SemanticIds::occurrence(self.delta.file.id, span),
                file_id: self.delta.file.id,
                range: span,
                text: self.text(n).into(),
                kind,
                scope_id: scope,
                resolved_symbol_id: declaration,
                receiver_occurrence_id: receiver_id,
                confidence: Confidence::tree_sitter(),
            });
        }
        for child in children(n) {
            self.collect_occurrences(child, scope, depth + 1)?;
        }
        Ok(())
    }
}
fn range(n: Node<'_>) -> TextRange {
    TextRange::new(ByteOffset(n.start_byte()), ByteOffset(n.end_byte()))
        .expect("ordered tree range")
}
fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = n.walk();
    n.named_children(&mut cursor).collect()
}
fn declarator_name(mut n: Node<'_>) -> Option<Node<'_>> {
    for _ in 0..256 {
        if matches!(
            n.kind(),
            "identifier" | "field_identifier" | "type_identifier"
        ) {
            return (!n.is_missing() && n.end_byte() > n.start_byte()).then_some(n);
        }
        n = n.child_by_field_name("declarator").or_else(|| {
            if n.kind() == "parenthesized_declarator" {
                children(n).first().copied()
            } else {
                None
            }
        })?;
    }
    None
}
fn pointer_depth(mut n: Node<'_>) -> usize {
    let mut count = 0;
    for _ in 0..256 {
        if n.kind() == "pointer_declarator" {
            count += 1;
        }
        let Some(next) = n.child_by_field_name("declarator").or_else(|| {
            if n.kind() == "parenthesized_declarator" {
                children(n).first().copied()
            } else {
                None
            }
        }) else {
            break;
        };
        n = next;
    }
    count
}
fn is_function(mut n: Node<'_>) -> bool {
    for _ in 0..256 {
        if n.kind() == "function_declarator" {
            return n
                .child_by_field_name("declarator")
                .is_some_and(|d| d.kind() == "identifier");
        }
        let Some(next) = n.child_by_field_name("declarator") else {
            break;
        };
        n = next;
    }
    false
}
