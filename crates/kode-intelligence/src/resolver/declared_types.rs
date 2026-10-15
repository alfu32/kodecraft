use super::*;
#[derive(Clone, Copy)]
pub(super) struct TypeContext {
    pub scope: ScopeId,
    pub file: FileId,
    pub offset: ByteOffset,
    pub exclude: Option<SymbolId>,
}
impl State<'_> {
    pub fn resolve_declared_types(&mut self) -> Result<()> {
        let facts: Vec<_> = self
            .project
            .deltas
            .values()
            .filter(|d| self.active_files.contains(&d.file.id))
            .flat_map(|d| d.unresolved_types.iter().map(|t| (d.file.id, t.clone())))
            .collect();
        for (file, fact) in facts {
            self.check()?;
            let Some(owner) = self.symbol(fact.owner_symbol_id) else {
                continue;
            };
            let exclude = (owner.kind == SymbolKind::TypeAlias).then_some(owner.id);
            let context = TypeContext {
                scope: fact.scope_id,
                file,
                offset: fact.range.start,
                exclude,
            };
            let Some(ty) = self.parse_type(&fact.name, context, 0) else {
                continue;
            };
            let id = SemanticIds::type_ref(&ty);
            let crosses = named_types(&ty)
                .iter()
                .any(|id| self.symbol(*id).is_some_and(|s| s.file_id != file));
            self.project.types.insert(
                id,
                TypeRecord {
                    id,
                    r#ref: ty.clone(),
                    confidence: fact.confidence,
                    knowledge_level: if crosses {
                        TypeKnowledgeLevel::CrossFileResolved
                    } else {
                        TypeKnowledgeLevel::SyntacticallyDeclared
                    },
                },
            );
            if fact.role != TypeRole::SuperType {
                self.project.symbols[self.symbols[&fact.owner_symbol_id]].declared_type_id =
                    Some(id);
            }
            if let Some(target) = primary_named(&ty) {
                let kind = match fact.role {
                    TypeRole::Declared if exclude.is_some() => RelationKind::AliasOf,
                    TypeRole::Declared => RelationKind::TypeOf,
                    TypeRole::Parameter => RelationKind::ParameterType,
                    TypeRole::Return => RelationKind::Returns,
                    TypeRole::SuperType
                        if self
                            .symbol(target)
                            .is_some_and(|s| s.kind == SymbolKind::Interface) =>
                    {
                        RelationKind::Implements
                    }
                    TypeRole::SuperType => RelationKind::Extends,
                };
                self.relation(fact.owner_symbol_id, target, kind, fact.confidence);
            }
        }
        Ok(())
    }
    pub(super) fn parse_type(
        &self,
        raw: &str,
        context: TypeContext,
        depth: usize,
    ) -> Option<TypeRef> {
        let TypeContext {
            scope,
            file,
            offset,
            exclude,
        } = context;
        if depth > 32 || raw.len() > 65536 {
            return None;
        }
        let mut text = raw.trim();
        if text.is_empty() {
            return None;
        }
        if self.language(file) == "c" {
            let filtered = text
                .split_whitespace()
                .filter(|part| !matches!(*part, "const" | "volatile" | "restrict" | "_Atomic"))
                .collect::<Vec<_>>()
                .join(" ");
            text = &filtered;
            loop {
                let trimmed = text.trim_end();
                if let Some(base) = trimmed.strip_suffix('*') {
                    text = base;
                } else {
                    text = trimmed;
                    break;
                }
            }
            let kind = if let Some(name) = text
                .strip_prefix("struct ")
                .or_else(|| text.strip_prefix("union "))
            {
                text = name.trim();
                NameKind::Tag(SymbolKind::Struct)
            } else if let Some(name) = text.strip_prefix("enum ") {
                text = name.trim();
                NameKind::Tag(SymbolKind::Enum)
            } else {
                NameKind::Type
            };
            return self
                .find_name(text, scope, file, offset, kind, exclude)
                .map(TypeRef::named)
                .or_else(|| is_primitive(text, "c").then(|| TypeRef::primitive(text)));
        }
        if let Some(inside) = outer_parentheses(text) {
            return self.parse_type(inside, context, depth + 1);
        }
        if let Some(arrow) = top_level_arrow(text) {
            let parameters = text[..arrow].trim();
            let parameters = outer_parentheses(parameters).unwrap_or(parameters);
            let parameters = split_top_level(parameters)
                .into_iter()
                .map(|param| {
                    self.parse_type(
                        param.split_once(':').map_or(param, |(_, ty)| ty),
                        context,
                        depth + 1,
                    )
                    .unwrap_or(TypeRef::Unknown)
                })
                .collect();
            let returns = self
                .parse_type(&text[arrow + 2..], context, depth + 1)
                .map(Box::new);
            return Some(TypeRef::Function {
                parameters,
                returns,
            });
        }
        if let Some(base) = text.strip_suffix('?') {
            return self
                .parse_type(base, context, depth + 1)
                .map(|inner| TypeRef::Nullable {
                    inner: Box::new(inner),
                });
        }
        if let Some(start) = text.find('<')
            && text.ends_with('>')
        {
            let base = self
                .parse_type(&text[..start], context, depth + 1)
                .unwrap_or(TypeRef::Unknown);
            let arguments = split_top_level(&text[start + 1..text.len() - 1])
                .into_iter()
                .map(|arg| {
                    let arg = arg
                        .trim()
                        .strip_prefix("out ")
                        .or_else(|| arg.trim().strip_prefix("in "))
                        .unwrap_or(arg)
                        .trim();
                    self.parse_type(arg, context, depth + 1)
                        .unwrap_or(TypeRef::Unknown)
                })
                .collect();
            return Some(TypeRef::Generic {
                base: Box::new(base),
                arguments,
            });
        }
        self.find_name(text, scope, file, offset, NameKind::Type, exclude)
            .map(TypeRef::named)
            .or_else(|| is_primitive(text, self.language(file)).then(|| TypeRef::primitive(text)))
    }
}
pub(super) fn is_primitive(name: &str, language: &str) -> bool {
    if language == "c" {
        matches!(
            name,
            "void"
                | "char"
                | "signed char"
                | "unsigned char"
                | "int"
                | "unsigned"
                | "signed"
                | "unsigned int"
                | "signed int"
                | "short"
                | "short int"
                | "unsigned short"
                | "unsigned short int"
                | "long"
                | "long int"
                | "unsigned long"
                | "unsigned long int"
                | "long long"
                | "long long int"
                | "unsigned long long"
                | "unsigned long long int"
                | "float"
                | "double"
                | "long double"
                | "_Bool"
                | "bool"
                | "size_t"
                | "ssize_t"
                | "ptrdiff_t"
        )
    } else {
        matches!(
            name,
            "Any"
                | "Boolean"
                | "Byte"
                | "Char"
                | "Double"
                | "Float"
                | "Int"
                | "Long"
                | "Nothing"
                | "Short"
                | "String"
                | "Unit"
        )
    }
}
pub(super) fn named_types(ty: &TypeRef) -> BTreeSet<SymbolId> {
    let mut pending = vec![ty];
    let mut result = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        match ty {
            TypeRef::Named { symbol_id } => {
                result.insert(*symbol_id);
            }
            TypeRef::Generic { base, arguments } => {
                pending.push(base);
                pending.extend(arguments);
            }
            TypeRef::Nullable { inner } => pending.push(inner),
            TypeRef::Function {
                parameters,
                returns,
            } => {
                pending.extend(parameters);
                if let Some(returns) = returns {
                    pending.push(returns);
                }
            }
            TypeRef::Union { alternatives } => pending.extend(alternatives),
            _ => {}
        }
    }
    result
}
fn outer_parentheses(text: &str) -> Option<&str> {
    if !text.starts_with('(') || !text.ends_with(')') {
        return None;
    }
    let mut depth = 0i32;
    for (i, ch) in text.char_indices() {
        if ch == '(' {
            depth += 1;
        }
        if ch == ')' {
            depth -= 1;
        }
        if depth == 0 && i + 1 < text.len() {
            return None;
        }
        if depth < 0 {
            return None;
        }
    }
    (depth == 0).then(|| &text[1..text.len() - 1])
}
fn top_level_arrow(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' if i == 0 || text.as_bytes()[i - 1] != b'-' => depth -= 1,
            ')' | ']' => depth -= 1,
            '-' if depth == 0 && text[i..].starts_with("->") => return Some(i),
            _ => {}
        }
    }
    None
}
fn split_top_level(text: &str) -> Vec<&str> {
    if text.trim().is_empty() {
        return vec![];
    }
    let (mut depth, mut start) = (0i32, 0usize);
    let mut result = vec![];
    for (i, ch) in text.char_indices() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' if i == 0 || text.as_bytes()[i - 1] != b'-' => depth -= 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                result.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(text[start..].trim());
    result
}
