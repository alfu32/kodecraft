use super::*;
use super::{declared_types::TypeContext, types::merge_types};
pub(super) type Key = (FileId, ByteOffset, ByteOffset);
#[derive(Clone, Default)]
pub(super) struct Evaluation {
    pub ty: Option<TypeRef>,
    pub symbol: Option<SymbolId>,
}
impl State<'_> {
    pub(super) fn evaluate_one(&self, key: Key) -> Evaluation {
        self.evaluate(key, &mut BTreeMap::new(), &mut BTreeSet::new(), 0)
    }
    pub(super) fn evaluate_all(&mut self) -> Result<BTreeMap<Key, Evaluation>> {
        let mut memo = BTreeMap::new();
        let keys: Vec<_> = self
            .expression_syntax
            .keys()
            .filter(|key| self.active_files.contains(&key.0))
            .copied()
            .collect();
        for key in keys {
            self.check()?;
            self.evaluate(key, &mut memo, &mut BTreeSet::new(), 0);
        }
        Ok(memo)
    }
    fn evaluate(
        &self,
        key: Key,
        memo: &mut BTreeMap<Key, Evaluation>,
        visiting: &mut BTreeSet<Key>,
        depth: usize,
    ) -> Evaluation {
        if let Some(value) = memo.get(&key) {
            return value.clone();
        }
        if depth > 64 || !visiting.insert(key) {
            return Evaluation::default();
        }
        let value = if let Some(expression) = self.expression_syntax.get(&key) {
            let file = key.0;
            let child_key = |range: SourceRange| (file, range.start, range.end);
            match &expression.expression {
                SyntaxExpression::Reference { occurrence_id } => {
                    let occurrence = self
                        .occurrence_indices
                        .get(occurrence_id)
                        .and_then(|i| self.project.occurrences.get(*i));
                    let symbol = occurrence
                        .and_then(|o| o.resolved_symbol_id)
                        .and_then(|id| self.symbol(id));
                    Evaluation {
                        ty: occurrence
                            .zip(symbol)
                            .and_then(|(o, s)| self.occurrence_type(o, s)),
                        symbol: symbol.map(|s| s.id),
                    }
                }
                SyntaxExpression::Literal { type_name } => Evaluation {
                    ty: Some(TypeRef::primitive(type_name)),
                    symbol: None,
                },
                SyntaxExpression::Parenthesized { inner } => {
                    self.evaluate(child_key(*inner), memo, visiting, depth + 1)
                }
                SyntaxExpression::Cast { type_name, .. } => Evaluation {
                    ty: self.parse_type(
                        type_name,
                        TypeContext {
                            scope: expression.scope_id,
                            file,
                            offset: expression.range.start,
                            exclude: None,
                        },
                        0,
                    ),
                    symbol: None,
                },
                SyntaxExpression::Call {
                    callee,
                    type_arguments,
                } => {
                    let callee = self.evaluate(child_key(*callee), memo, visiting, depth + 1);
                    let symbol = callee.symbol.and_then(|id| self.symbol(id));
                    let mut ty = if let Some(symbol) = symbol {
                        if is_type(symbol.kind) {
                            Some(TypeRef::named(symbol.id))
                        } else if matches!(
                            symbol.kind,
                            SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
                        ) {
                            symbol
                                .declared_type_id
                                .or(symbol.inferred_type_id)
                                .and_then(|id| self.project.types.get(&id))
                                .map(|t| t.r#ref.clone())
                        } else if let Some(TypeRef::Function { returns, .. }) = callee.ty {
                            returns.map(|ty| *ty)
                        } else {
                            None
                        }
                    } else if let Some(TypeRef::Function { returns, .. }) = callee.ty {
                        returns.map(|ty| *ty)
                    } else {
                        None
                    };
                    if symbol.is_some_and(|symbol| is_type(symbol.kind))
                        && !type_arguments.is_empty()
                        && let Some(base) = ty.take()
                    {
                        let context = TypeContext {
                            scope: expression.scope_id,
                            file,
                            offset: expression.range.start,
                            exclude: None,
                        };
                        let arguments = type_arguments
                            .iter()
                            .map(|name| {
                                self.parse_type(name, context, 0)
                                    .unwrap_or(TypeRef::Unknown)
                            })
                            .collect();
                        ty = Some(TypeRef::Generic {
                            base: Box::new(base),
                            arguments,
                        });
                    }
                    Evaluation { ty, symbol: None }
                }
                SyntaxExpression::Member {
                    receiver,
                    occurrence_id,
                } => {
                    let receiver = self.evaluate(child_key(*receiver), memo, visiting, depth + 1);
                    let occurrence = self
                        .occurrence_indices
                        .get(occurrence_id)
                        .and_then(|i| self.project.occurrences.get(*i));
                    if let (Some(ty), Some(occurrence)) = (receiver.ty, occurrence) {
                        let members =
                            self.value_members(&ty, &occurrence.text, file, expression.scope_id);
                        let mut ty = None;
                        for id in &members {
                            if let Some(member) = self.symbol(*id)
                                && let Some(value) = member
                                    .declared_type_id
                                    .or(member.inferred_type_id)
                                    .and_then(|id| self.project.types.get(&id))
                            {
                                ty = Some(merge_types(ty.as_ref(), &value.r#ref));
                            }
                        }
                        Evaluation {
                            ty,
                            symbol: if members.len() == 1 {
                                members.first().copied()
                            } else {
                                None
                            },
                        }
                    } else {
                        Evaluation::default()
                    }
                }
                SyntaxExpression::Unknown => Evaluation::default(),
            }
        } else {
            Evaluation::default()
        };
        visiting.remove(&key);
        memo.insert(key, value.clone());
        value
    }
}
/// Queries prepared expression facts and identity-linked fallback records. It
/// never searches unrelated workspace text or parses on the calling thread.
pub struct BestEffortExpressionTypeResolver;
impl BestEffortExpressionTypeResolver {
    pub fn type_of(
        file: FileId,
        range: SourceRange,
        snapshot: &crate::snapshot::SemanticSnapshot,
    ) -> TypeRef {
        if let Some(ty) = snapshot.expression_type(file, range) {
            return ty.clone();
        }
        let occurrence = snapshot
            .occurrences(file)
            .filter(|o| o.range.start >= range.start && o.range.end <= range.end)
            .last()
            .or_else(|| snapshot.occurrence_at(file, range.start));
        let Some(occurrence) = occurrence else {
            return TypeRef::Unknown;
        };
        let Some(symbol) = occurrence
            .resolved_symbol_id
            .and_then(|id| snapshot.symbol(id))
        else {
            return TypeRef::Unknown;
        };
        if is_type(symbol.kind)
            && (matches!(
                occurrence.kind,
                OccurrenceKind::Call | OccurrenceKind::TypeReference
            ) || matches!(occurrence.text.as_str(), "this" | "super"))
        {
            return TypeRef::named(symbol.id);
        }
        symbol
            .declared_type_id
            .or(symbol.inferred_type_id)
            .and_then(|id| snapshot.type_record(id))
            .map_or(TypeRef::Unknown, |record| record.r#ref.clone())
    }
}
