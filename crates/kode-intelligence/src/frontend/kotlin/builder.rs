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
    visited: usize,
    delta: FileSemanticDelta,
    package: Option<String>,
    declarations: HashMap<TextRange, SymbolId>,
    scopes: HashMap<ScopeId, usize>,
    node_scopes: HashMap<usize, ScopeId>,
    symbols: HashMap<SymbolId, usize>,
    assignments: Vec<Node<'a>>,
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
            visited: 0,
            delta: FileSemanticDelta {
                file: FileRecord {
                    id: SemanticIds::file(path),
                    path: path.into(),
                    language_id: "kotlin".into(),
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
                exported_surface_hash: String::new(),
            },
            package: None,
            declarations: HashMap::new(),
            scopes: HashMap::new(),
            node_scopes: HashMap::new(),
            symbols: HashMap::new(),
            assignments: vec![],
        }
    }
    fn check(&mut self, depth: usize) -> Result<()> {
        self.visited += 1;
        if self.cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if depth > 256
            || self.visited > 2_000_000
            || self.started.elapsed() > Duration::from_secs(2)
            || self.delta.symbols.len() > 100_000
            || self.delta.scopes.len() > 10_000
        {
            return Err(DomainError::Unsupported(
                "Kotlin extraction work limit exceeded".into(),
            ));
        }
        Ok(())
    }
    fn text(&self, node: Node<'_>) -> &'a str {
        &self.source[node.byte_range()]
    }
    fn scope(&self, id: ScopeId) -> &ScopeRecord {
        &self.delta.scopes[self.scopes[&id]]
    }
    fn symbol(&self, id: SymbolId) -> &SymbolRecord {
        &self.delta.symbols[self.symbols[&id]]
    }
    fn add_scope(
        &mut self,
        node: Node<'_>,
        parent: Option<ScopeId>,
        owner: Option<SymbolId>,
        kind: ScopeKind,
        span: TextRange,
    ) -> ScopeId {
        let id = SemanticIds::scope(self.delta.file.id, span.start, kind);
        if !self.scopes.contains_key(&id) {
            self.scopes.insert(id, self.delta.scopes.len());
            self.delta.scopes.push(ScopeRecord {
                id,
                file_id: self.delta.file.id,
                parent_id: parent,
                owner_symbol_id: owner,
                kind,
                range: span,
            });
        }
        self.node_scopes.insert(node.id(), id);
        id
    }
    pub(super) fn build(mut self) -> Result<FileSemanticDelta> {
        self.check(0)?;
        let root = self.parsed.root_node();
        let file_scope = self.add_scope(root, None, None, ScopeKind::File, range(root));
        let mut content_scope = file_scope;
        for node in children(root) {
            if node.kind() != "package_header" {
                continue;
            }
            let name = self
                .text(node)
                .trim_start_matches("package")
                .trim()
                .trim_end_matches(';')
                .trim();
            if name.is_empty() {
                continue;
            }
            let qualified = children(node)
                .into_iter()
                .find(|n| n.kind() == "qualified_identifier");
            let name_node = qualified.and_then(|n| {
                children(n)
                    .into_iter()
                    .rev()
                    .find(|n| n.kind() == "identifier")
            });
            if let Some(name_node) = name_node {
                let id = self.add_symbol(node, name_node, SymbolKind::Package, file_scope, None);
                self.delta.symbols[self.symbols[&id]].qualified_name = Some(name.to_owned());
                self.package = Some(name.to_owned());
                content_scope = self.add_scope(
                    node,
                    Some(file_scope),
                    Some(id),
                    ScopeKind::Package,
                    TextRange::new(ByteOffset(node.start_byte()), ByteOffset(self.source.len()))?,
                );
            }
        }
        // The package scope covers declarations after the header, including
        // imports. Root context is explicit rather than inferred by string names.
        self.node_scopes.insert(root.id(), content_scope);
        for node in children(root) {
            if node.kind() == "import" {
                let raw = self
                    .text(node)
                    .trim_start_matches("import")
                    .trim()
                    .trim_end_matches(';')
                    .trim();
                let (path, alias) = raw.split_once(" as ").map_or((raw, None), |(p, a)| {
                    (p.trim(), Some(a.trim().trim_matches('`').to_owned()))
                });
                self.delta.imports.push(ImportRecord {
                    file_id: self.delta.file.id,
                    scope_id: content_scope,
                    path: path.trim_end_matches(".*").into(),
                    alias,
                    wildcard: path.ends_with(".*"),
                    range: range(node),
                });
            } else if node.kind() != "package_header" {
                self.visit(node, content_scope, None, 0)?;
            }
        }
        self.collect_occurrences(root, content_scope, false, false, 0)?;
        self.collect_assignments()?;
        self.collect_lexical(root, 0)?;
        self.delta
            .scopes
            .sort_by_key(|s| (s.range.start, s.range.end, s.kind));
        self.delta.symbols.sort_by_key(|s| s.name_range.start);
        self.delta
            .occurrences
            .sort_by_key(|s| (s.range.start, s.range.end));
        self.delta
            .lexical_tokens
            .sort_by_key(|s| (s.range.start, s.range.end));
        self.delta.exported_surface_hash = public_surface_hash(&self.delta);
        self.delta.validate_source(self.source)?;
        Ok(self.delta)
    }
    fn add_symbol(
        &mut self,
        declaration: Node<'_>,
        name: Node<'_>,
        kind: SymbolKind,
        scope: ScopeId,
        owner: Option<SymbolId>,
    ) -> SymbolId {
        let text = self.text(name).trim_matches('`');
        let id = SemanticIds::symbol(
            self.delta.file.id,
            ByteOffset(name.start_byte()),
            kind,
            text,
        );
        if self.symbols.contains_key(&id) {
            return id;
        }
        let parent_name = owner
            .and_then(|o| self.symbol(o).qualified_name.as_deref())
            .or(self.package.as_deref());
        let qualified_name =
            Some(parent_name.map_or_else(|| text.to_owned(), |p| format!("{p}.{text}")));
        self.symbols.insert(id, self.delta.symbols.len());
        self.delta.symbols.push(SymbolRecord {
            id,
            file_id: self.delta.file.id,
            name: text.into(),
            qualified_name,
            kind,
            declaration_range: range(declaration),
            name_range: range(name),
            scope_id: scope,
            owner_symbol_id: owner,
            declared_type_id: None,
            inferred_type_id: None,
            flags: visibility(self.source, declaration),
            confidence: Confidence::tree_sitter(),
        });
        self.declarations.insert(range(name), id);
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
                    confidence: Confidence::inferred(0.95).expect("constant confidence"),
                });
            }
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
        match node.kind() {
            "class_declaration" | "object_declaration" | "companion_object" | "type_alias" => {
                if let Some(name) = declaration_name(node) {
                    let kind = match node.kind() {
                        "type_alias" => SymbolKind::TypeAlias,
                        "class_declaration" if has_token(node, "interface") => {
                            SymbolKind::Interface
                        }
                        "class_declaration"
                            if children(node).iter().any(|n| n.kind() == "enum_class_body")
                                || self.text(node).trim_start().starts_with("enum ") =>
                        {
                            SymbolKind::Enum
                        }
                        _ => SymbolKind::Class,
                    };
                    let id = self.add_symbol(node, name, kind, scope, owner);
                    scope =
                        self.add_scope(node, Some(scope), Some(id), ScopeKind::Type, range(node));
                    owner = Some(id);
                    for child in children(node) {
                        if child.kind() == "delegation_specifiers" {
                            for specifier in children(child) {
                                if let Some(ty) = first_type(specifier) {
                                    self.add_type(id, scope, ty, TypeRole::SuperType);
                                }
                            }
                        } else if node.kind() == "type_alias" && is_type(child) {
                            self.add_type(id, scope, child, TypeRole::Declared);
                        }
                    }
                }
            }
            "function_declaration" => {
                if let Some(name) = declaration_name(node) {
                    let kind = if self.scope(scope).kind == ScopeKind::Type {
                        SymbolKind::Method
                    } else {
                        SymbolKind::Function
                    };
                    let id = self.add_symbol(node, name, kind, scope, owner);
                    scope = self.add_scope(
                        node,
                        Some(scope),
                        Some(id),
                        ScopeKind::Function,
                        range(node),
                    );
                    owner = Some(id);
                    let params_end = children(node)
                        .iter()
                        .find(|n| n.kind() == "function_value_parameters")
                        .map_or(name.end_byte(), |n| n.end_byte());
                    for child in children(node) {
                        if child.start_byte() >= params_end && is_type(child) {
                            self.add_type(id, scope, child, TypeRole::Return);
                            break;
                        }
                    }
                }
            }
            "property_declaration" => {
                let variables: Vec<_> = children(node)
                    .into_iter()
                    .filter(|n| {
                        n.kind() == "variable_declaration"
                            || n.kind() == "multi_variable_declaration"
                    })
                    .collect();
                let initializer = initializer(node);
                for variable in variables {
                    let declarations = if variable.kind() == "multi_variable_declaration" {
                        children(variable)
                    } else {
                        vec![variable]
                    };
                    for variable in declarations {
                        if let Some(name) = declaration_name(variable) {
                            let kind = if children(node).iter().any(|n| {
                                n.kind() == "modifiers"
                                    && self.text(*n).split_whitespace().any(|m| m == "const")
                            }) {
                                SymbolKind::Constant
                            } else if self.scope(scope).kind == ScopeKind::Type {
                                SymbolKind::Property
                            } else {
                                SymbolKind::Variable
                            };
                            let id = self.add_symbol(node, name, kind, scope, owner);
                            if let Some(ty) = children(variable).into_iter().find(|n| is_type(*n)) {
                                self.add_type(id, scope, ty, TypeRole::Declared);
                            }
                            if let Some(expression) = initializer {
                                self.add_hint(id, scope, expression);
                            }
                        }
                    }
                }
            }
            "parameter" | "class_parameter" | "enum_entry" => {
                if let Some(name) = declaration_name(node) {
                    let kind = if node.kind() == "class_parameter"
                        && (has_token(node, "val") || has_token(node, "var"))
                    {
                        SymbolKind::Property
                    } else if node.kind() == "enum_entry" {
                        SymbolKind::EnumMember
                    } else {
                        SymbolKind::Parameter
                    };
                    let id = self.add_symbol(node, name, kind, scope, owner);
                    if let Some(ty) = children(node).into_iter().find(|n| is_type(*n)) {
                        self.add_type(id, scope, ty, TypeRole::Parameter);
                    }
                    if let Some(expression) = initializer(node) {
                        self.add_hint(id, scope, expression);
                    }
                }
            }
            "variable_declaration"
                if !children(node)
                    .iter()
                    .any(|n| self.declarations.contains_key(&range(*n))) =>
            {
                // Lambda, loop and catch declarations also participate in scopes.
                if let Some(name) = declaration_name(node)
                    && self.text(name) != "_"
                {
                    let id = self.add_symbol(node, name, SymbolKind::Variable, scope, owner);
                    if let Some(ty) = children(node).into_iter().find(|n| is_type(*n)) {
                        self.add_type(id, scope, ty, TypeRole::Declared);
                    }
                }
            }
            "block" | "lambda_literal" | "for_statement" | "catch_block" => {
                let kind = if node.kind() == "lambda_literal" {
                    ScopeKind::Lambda
                } else {
                    ScopeKind::Block
                };
                scope = self.add_scope(node, Some(scope), owner, kind, range(node));
            }
            "assignment" => self.assignments.push(node),
            _ => {}
        }
        self.node_scopes.entry(node.id()).or_insert(scope);
        for child in children(node) {
            self.visit(child, scope, owner, depth + 1)?;
        }
        Ok(())
    }
    fn add_type(&mut self, owner: SymbolId, scope: ScopeId, node: Node<'_>, role: TypeRole) {
        self.delta.unresolved_types.push(UnresolvedTypeRef {
            owner_symbol_id: owner,
            scope_id: scope,
            name: self.text(node).trim().into(),
            role,
            range: range(node),
            confidence: Confidence::tree_sitter(),
        });
    }
    fn add_hint(&mut self, target: SymbolId, scope: ScopeId, raw: Node<'_>) {
        let expression = unwrap(raw);
        let hint = if expression.kind() == "as_expression" {
            expression.child_by_field_name("right").map(|ty| {
                (
                    self.text(ty).to_owned(),
                    TypeHintKind::Cast,
                    range(expression),
                )
            })
        } else if expression.kind() == "navigation_expression" {
            None // Requires receiver-aware expression resolution, never the first identifier.
        } else if expression.kind() == "call_expression"
            || expression.kind() == "constructor_invocation"
        {
            children(expression)
                .first()
                .filter(|n| matches!(n.kind(), "identifier" | "user_type"))
                .map(|callee| {
                    let name = self.text(*callee).trim_matches('`').to_owned();
                    let kind = if name.chars().next().is_some_and(char::is_uppercase) {
                        TypeHintKind::ConstructorCall
                    } else {
                        TypeHintKind::InitializerCall
                    };
                    (name, kind, range(expression))
                })
        } else if let Some(primitive) = literal_type(self.text(expression), expression.kind()) {
            Some((primitive.into(), TypeHintKind::Literal, range(expression)))
        } else if matches!(
            expression.kind(),
            "identifier" | "this_expression" | "super_expression"
        ) && self.text(expression) != "null"
        {
            Some((
                self.text(expression).trim_matches('`').into(),
                TypeHintKind::Assignment,
                range(expression),
            ))
        } else {
            None
        };
        if let Some((name, kind, span)) = hint {
            self.delta.type_hints.push(TypeHint {
                target_symbol_id: target,
                scope_id: scope,
                expression_range: span,
                referenced_name: name,
                kind,
                confidence: Confidence::inferred(0.85).expect("constant confidence"),
            });
        }
    }
    fn collect_assignments(&mut self) -> Result<()> {
        for node in self.assignments.clone() {
            self.check(0)?;
            let Some(left) = node.child_by_field_name("left") else {
                continue;
            };
            let Some(right) = node.child_by_field_name("right") else {
                continue;
            };
            if left.kind() != "identifier" {
                continue;
            } // Members need receiver resolution.
            let name = self.text(left).trim_matches('`');
            let scope = self.node_scopes[&node.id()];
            let mut current = Some(scope);
            while let Some(id) = current {
                let target = self
                    .delta
                    .symbols
                    .iter()
                    .filter(|s| {
                        s.scope_id == id
                            && s.name == name
                            && s.name_range.start.0 <= left.start_byte()
                    })
                    .max_by_key(|s| s.name_range.start)
                    .map(|s| s.id);
                if let Some(target) = target {
                    self.add_hint(target, scope, right);
                    break;
                }
                current = self.scope(id).parent_id;
            }
        }
        Ok(())
    }
    fn collect_occurrences(
        &mut self,
        node: Node<'a>,
        inherited: ScopeId,
        in_type: bool,
        in_import: bool,
        depth: usize,
    ) -> Result<()> {
        self.check(depth)?;
        let scope = self
            .node_scopes
            .get(&node.id())
            .copied()
            .unwrap_or(inherited);
        let in_type = in_type
            || is_type(node)
            || matches!(
                node.kind(),
                "type_arguments" | "type_projection" | "type_constraint"
            );
        let in_import = in_import || node.kind() == "import";
        if let Some(primitive) = literal_type(self.text(node), node.kind()) {
            self.delta.expression_types.push(ExpressionTypeRecord {
                file_id: self.delta.file.id,
                range: range(node),
                r#type: TypeRef::primitive(primitive),
                confidence: Confidence::tree_sitter(),
            });
            // Interpolated strings still contain real reference expressions.
            if !matches!(node.kind(), "string_literal" | "multiline_string_literal") {
                return Ok(());
            }
        }
        if matches!(node.kind(), "string_literal" | "multiline_string_literal") {
            self.short_interpolations(node, scope)?;
        }
        if node.kind().contains("comment")
            || (node.kind() == "identifier" && matches!(self.text(node), "null" | "_"))
        {
            return Ok(());
        }
        if matches!(
            node.kind(),
            "identifier" | "this_expression" | "super_expression"
        ) {
            let span = range(node);
            let declaration = self.declarations.get(&span).copied();
            let parent = node.parent();
            let member = parent
                .filter(|n| n.kind() == "navigation_expression")
                .filter(|n| {
                    children(*n)
                        .last()
                        .is_some_and(|last| last.id() == node.id())
                });
            let kind = if declaration.is_some() {
                OccurrenceKind::Declaration
            } else if in_import {
                OccurrenceKind::Import
            } else if in_type {
                OccurrenceKind::TypeReference
            } else if member.is_some() {
                OccurrenceKind::MemberReference
            } else if self.source[node.end_byte()..].trim_start().starts_with('(') {
                OccurrenceKind::Call
            } else {
                OccurrenceKind::Read
            };
            let receiver = member
                .and_then(|nav| children(nav).first().copied())
                .and_then(crate::frontend::receiver_node)
                .and_then(|receiver| {
                    self.delta
                        .occurrences
                        .iter()
                        .rev()
                        .find(|occ| occ.range == range(receiver))
                        .map(|occ| occ.id)
                });
            self.delta.occurrences.push(OccurrenceRecord {
                id: SemanticIds::occurrence(self.delta.file.id, span),
                file_id: self.delta.file.id,
                range: span,
                text: self.text(node).trim_matches('`').into(),
                kind,
                scope_id: scope,
                resolved_symbol_id: declaration,
                receiver_occurrence_id: receiver,
                confidence: Confidence::new(
                    SemanticSource::TreeSitter,
                    if declaration.is_some() { 0.95 } else { 0.90 },
                )
                .expect("constant confidence"),
            });
            return Ok(());
        }
        for child in children(node) {
            self.collect_occurrences(child, scope, in_type, in_import, depth + 1)?;
        }
        Ok(())
    }
    fn short_interpolations(&mut self, node: Node<'a>, scope: ScopeId) -> Result<()> {
        // kotlin-ng 1.1.0 can tokenize "$name" as two string_content nodes.
        // Recover only the language's short identifier form from those nodes;
        // ${expression} remains the grammar's responsibility. Escape sequences
        // have separate nodes, so an escaped dollar never enters this scan.
        for content in children(node)
            .into_iter()
            .filter(|n| n.kind() == "string_content")
        {
            for (relative, ch) in self.text(content).char_indices() {
                if ch != '$' {
                    continue;
                }
                self.check(0)?;
                let start = content.start_byte() + relative + 1;
                let suffix = &self.source[start..node.end_byte()];
                if !suffix
                    .chars()
                    .next()
                    .is_some_and(|ch| ch == '_' || ch.is_alphabetic())
                {
                    continue;
                }
                let length: usize = suffix
                    .chars()
                    .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                    .map(char::len_utf8)
                    .sum();
                let span = TextRange::new(ByteOffset(start), ByteOffset(start + length))?;
                self.delta.occurrences.push(OccurrenceRecord {
                    id: SemanticIds::occurrence(self.delta.file.id, span),
                    file_id: self.delta.file.id,
                    range: span,
                    text: self.source[start..start + length].into(),
                    kind: OccurrenceKind::Read,
                    scope_id: scope,
                    resolved_symbol_id: None,
                    receiver_occurrence_id: None,
                    confidence: Confidence::tree_sitter(),
                });
            }
        }
        Ok(())
    }
    fn collect_lexical(&mut self, node: Node<'a>, depth: usize) -> Result<()> {
        self.check(depth)?;
        let text = self.text(node);
        let kind = if node.kind().contains("comment") {
            Some(LexicalTokenKind::Comment)
        } else if matches!(
            node.kind(),
            "string_literal" | "multiline_string_literal" | "character_literal"
        ) {
            Some(LexicalTokenKind::String)
        } else if matches!(node.kind(), "number_literal" | "float_literal") {
            Some(LexicalTokenKind::Number)
        } else if matches!(text, "true" | "false" | "null")
            || (!node.is_named()
                && text.chars().all(|c| c.is_alphabetic() || c == '_')
                && !text.is_empty())
        {
            Some(LexicalTokenKind::Keyword)
        } else {
            None
        };
        if let Some(kind) = kind {
            if node.end_byte() > node.start_byte() {
                self.delta.lexical_tokens.push(LexicalTokenRecord {
                    file_id: self.delta.file.id,
                    range: range(node),
                    kind,
                    confidence: Confidence::tree_sitter(),
                });
            }
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.collect_lexical(child, depth + 1)?;
            }
        }
        Ok(())
    }
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn range(node: Node<'_>) -> TextRange {
    TextRange::new(ByteOffset(node.start_byte()), ByteOffset(node.end_byte()))
        .expect("ordered tree-sitter range")
}
fn declaration_name(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name").or_else(|| {
        children(node)
            .into_iter()
            .find(|n| n.kind() == "identifier")
    })
}
fn has_token(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|n| n.kind() == kind)
}
fn is_type(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "user_type"
            | "nullable_type"
            | "function_type"
            | "parenthesized_type"
            | "non_nullable_type"
    )
}
fn first_type(node: Node<'_>) -> Option<Node<'_>> {
    if is_type(node) {
        return Some(node);
    }
    children(node).into_iter().find_map(|child| {
        if is_type(child) {
            Some(child)
        } else if child.kind() == "constructor_invocation" {
            children(child).into_iter().find(|n| is_type(*n))
        } else {
            None
        }
    })
}
fn initializer(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let mut found = false;
    for child in node.children(&mut cursor) {
        if found && child.is_named() {
            return Some(child);
        }
        if child.kind() == "=" {
            found = true;
        }
    }
    None
}
fn visibility(source: &str, node: Node<'_>) -> u64 {
    let modifiers = children(node)
        .into_iter()
        .find(|n| n.kind() == "modifiers")
        .map_or("", |n| &source[n.byte_range()]);
    for (name, flag) in [
        ("private", SymbolFlags::PRIVATE),
        ("protected", SymbolFlags::PROTECTED),
        ("internal", SymbolFlags::INTERNAL),
    ] {
        if modifiers.split_whitespace().any(|word| word == name) {
            return flag;
        }
    }
    SymbolFlags::PUBLIC
}
fn unwrap(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let list = children(node);
        if list.len() != 1 {
            break;
        }
        node = list[0];
    }
    node
}
fn literal_type(text: &str, kind: &str) -> Option<&'static str> {
    match kind {
        "string_literal" | "multiline_string_literal" => Some("String"),
        "character_literal" => Some("Char"),
        "number_literal" | "float_literal" => {
            let text = text.replace('_', "");
            if text.ends_with(['f', 'F']) && !text.starts_with("0x") {
                Some("Float")
            } else if text.contains('.') || (!text.starts_with("0x") && text.contains(['e', 'E'])) {
                Some("Double")
            } else if text.ends_with(['l', 'L']) {
                Some("Long")
            } else {
                Some("Int")
            }
        }
        "identifier" if matches!(text, "true" | "false") => Some("Boolean"),
        _ => None,
    }
}
