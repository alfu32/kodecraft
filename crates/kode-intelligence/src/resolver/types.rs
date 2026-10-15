use super::*;
use super::{
    declared_types::{TypeContext, named_types},
    expressions::{Evaluation, Key},
};
pub(super) fn merge_types(existing: Option<&TypeRef>, inferred: &TypeRef) -> TypeRef {
    if existing == Some(&TypeRef::Unknown) {
        return TypeRef::Unknown;
    }
    let mut alternatives = BTreeMap::new();
    let mut pending = vec![inferred];
    if let Some(existing) = existing {
        pending.push(existing);
    }
    while let Some(ty) = pending.pop() {
        if let TypeRef::Union {
            alternatives: items,
        } = ty
        {
            pending.extend(items);
        } else {
            alternatives.insert(SemanticIds::type_ref(ty), ty.clone());
        }
    }
    if alternatives.len() > 64 {
        return TypeRef::Unknown;
    }
    if alternatives.len() == 1 {
        alternatives.into_values().next().expect("one alternative")
    } else {
        TypeRef::Union {
            alternatives: alternatives.into_values().collect(),
        }
    }
}
impl State<'_> {
    fn inferred_symbol(
        &mut self,
        target: SymbolId,
        ty: &TypeRef,
        confidence: Confidence,
    ) -> Result<bool> {
        let Some(symbol) = self.symbol(target) else {
            return Ok(false);
        };
        if symbol.declared_type_id.is_some() {
            return Ok(false);
        }
        let existing = symbol
            .inferred_type_id
            .and_then(|id| self.project.types.get(&id))
            .map(|t| &t.r#ref);
        let merged = merge_types(existing, ty);
        let id = SemanticIds::type_ref(&merged);
        if symbol.inferred_type_id == Some(id) {
            return Ok(false);
        }
        if merged == TypeRef::Unknown && existing != Some(&TypeRef::Unknown) {
            self.notes.push(
                "An inferred union exceeded 64 alternatives; its type remains unknown".into(),
            );
        }
        let record = self.project.types.entry(id).or_insert(TypeRecord {
            id,
            r#ref: merged.clone(),
            confidence,
            knowledge_level: TypeKnowledgeLevel::LocallyInferred,
        });
        if confidence.value() > record.confidence.value() {
            record.confidence = confidence;
        }
        let index = self.symbols[&target];
        self.project.symbols[index].inferred_type_id = Some(id);
        self.project.symbols[index].confidence = confidence;
        for named in named_types(&merged) {
            self.relation(target, named, RelationKind::TypeOf, confidence);
        }
        Ok(true)
    }
    pub fn infer_types(&mut self) -> Result<()> {
        let hints: Vec<_> = self
            .project
            .deltas
            .values()
            .filter(|d| self.active_files.contains(&d.file.id))
            .flat_map(|d| d.type_hints.iter().map(|h| (d.file.id, h.clone())))
            .collect();
        let bindings: Vec<_> = self
            .expression_syntax
            .iter()
            .filter(|(key, _)| self.active_files.contains(&key.0))
            .filter(|(_, e)| !e.assigned_to.is_empty())
            .map(|(key, e)| (*key, e.assigned_to.clone()))
            .collect();
        for pass in 0..64 {
            self.check()?;
            let mut changed = false;
            for (key, targets) in &bindings {
                self.check()?;
                if targets.iter().all(|target| {
                    self.symbol(*target)
                        .is_some_and(|symbol| symbol.declared_type_id.is_some())
                }) {
                    continue;
                }
                if let Some(ty) = self.evaluate_one(*key).ty {
                    for target in targets {
                        changed |=
                            self.inferred_symbol(*target, &ty, Confidence::inferred(0.85)?)?;
                    }
                }
            }
            for (file, hint) in &hints {
                self.check()?;
                if self
                    .symbol(hint.target_symbol_id)
                    .is_some_and(|symbol| symbol.declared_type_id.is_some())
                {
                    continue;
                }
                let key = (
                    *file,
                    hint.expression_range.start,
                    hint.expression_range.end,
                );
                let ty = if self.expression_syntax.contains_key(&key) {
                    self.evaluate_one(key).ty
                } else {
                    self.hint_type(*file, hint)
                };
                if let Some(ty) = ty {
                    changed |= self.inferred_symbol(hint.target_symbol_id, &ty, hint.confidence)?;
                }
            }
            if !changed {
                break;
            }
            if pass == 63 {
                self.notes.push(
                    "Type inference reached its 64-pass limit; partial facts were retained".into(),
                );
            }
        }
        Ok(())
    }
    fn hint_type(&self, file: FileId, hint: &TypeHint) -> Option<TypeRef> {
        let context = TypeContext {
            scope: hint.scope_id,
            file,
            offset: hint.expression_range.start,
            exclude: None,
        };
        match hint.kind {
            TypeHintKind::Cast => self.parse_type(&hint.referenced_name, context, 0),
            TypeHintKind::ConstructorCall => {
                let target = self
                    .find_name(
                        hint.referenced_name.split('<').next().unwrap_or_default(),
                        hint.scope_id,
                        file,
                        hint.expression_range.start,
                        NameKind::Callable,
                        None,
                    )
                    .and_then(|id| self.symbol(id))?;
                if is_type(target.kind) {
                    self.parse_type(&hint.referenced_name, context, 0)
                } else {
                    None
                }
            }
            TypeHintKind::Literal => Some(TypeRef::primitive(&hint.referenced_name)),
            TypeHintKind::InitializerCall => self
                .find_name(
                    &hint.referenced_name,
                    hint.scope_id,
                    file,
                    hint.expression_range.start,
                    NameKind::Callable,
                    None,
                )
                .and_then(|id| self.symbol(id))
                .and_then(|symbol| {
                    let ty = symbol
                        .declared_type_id
                        .or(symbol.inferred_type_id)
                        .and_then(|id| self.project.types.get(&id))?;
                    if matches!(
                        symbol.kind,
                        SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
                    ) {
                        Some(ty.r#ref.clone())
                    } else if let TypeRef::Function { returns, .. } = &ty.r#ref {
                        returns.as_ref().map(|ty| ty.as_ref().clone())
                    } else {
                        None
                    }
                }),
            TypeHintKind::Assignment => {
                if hint.referenced_name == "this" {
                    return self.owner_type(hint.scope_id).map(TypeRef::named);
                }
                if hint.referenced_name == "super" {
                    return self
                        .owner_type(hint.scope_id)
                        .and_then(|owner| self.base_type(owner))
                        .map(TypeRef::named);
                }
                let symbol = self
                    .find_name(
                        &hint.referenced_name,
                        hint.scope_id,
                        file,
                        hint.expression_range.start,
                        NameKind::Any,
                        None,
                    )
                    .and_then(|id| self.symbol(id))?;
                symbol
                    .declared_type_id
                    .or(symbol.inferred_type_id)
                    .and_then(|id| self.project.types.get(&id))
                    .map(|t| t.r#ref.clone())
            }
        }
    }
    pub fn publish_expression_types(&mut self) -> Result<()> {
        let memo = self.evaluate_all()?;
        for (key, value) in &memo {
            self.check()?;
            if let Some(expression) = self.expression_syntax.get(key)
                && let SyntaxExpression::Member { occurrence_id, .. } = &expression.expression
                && let Some(index) = self.occurrence_indices.get(occurrence_id)
            {
                self.project.occurrences[*index].resolved_symbol_id = value.symbol;
            }
        }
        // Re-evaluate token references after receiver-aware identities publish.
        let memo = self.evaluate_all()?;
        let mut values: BTreeMap<Key, ExpressionTypeRecord> = self
            .project
            .expression_types
            .drain(..)
            .map(|record| {
                (
                    (record.file_id, record.range.start, record.range.end),
                    record,
                )
            })
            .collect();
        for ((file, start, end), Evaluation { ty, .. }) in memo {
            let ty = ty.unwrap_or(TypeRef::Unknown);
            if start == end {
                continue;
            }
            values
                .entry((file, start, end))
                .or_insert(ExpressionTypeRecord {
                    file_id: file,
                    range: SourceRange::new(start, end)?,
                    r#type: ty,
                    confidence: Confidence::inferred(0.85)?,
                });
        }
        self.project.expression_types = values.into_values().collect();
        self.notes.sort();
        self.notes.dedup();
        Ok(())
    }
}
