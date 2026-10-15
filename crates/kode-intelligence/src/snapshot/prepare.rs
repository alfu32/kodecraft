use crate::model::*;
use kode_core::{DomainError, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    sync::Arc,
};
pub(super) fn invalid(message: impl ToString) -> DomainError {
    DomainError::Parse {
        format: "semantic snapshot",
        message: message.to_string(),
    }
}
fn unique<T: PartialEq, K: Copy + Ord + Debug>(
    records: &mut Vec<T>,
    key: impl Fn(&T) -> K,
    label: &str,
) -> Result<()> {
    let mut indices = BTreeMap::<K, usize>::new();
    let mut result = Vec::with_capacity(records.len());
    for record in std::mem::take(records) {
        let id = key(&record);
        if let Some(index) = indices.get(&id) {
            if result[*index] != record {
                return Err(invalid(format!(
                    "conflicting duplicate {label} identity {id:?}"
                )));
            }
        } else {
            indices.insert(id, result.len());
            result.push(record);
        }
    }
    *records = result;
    Ok(())
}
impl ResolvedSemanticProject {
    /// Build unresolved query facts only. Parsing/type resolution belongs to its
    /// explicit frontend/resolver; this does not fabricate inferred information.
    pub fn from_deltas(deltas: impl IntoIterator<Item = FileSemanticDelta>) -> Result<Self> {
        let mut project = Self::default();
        for mut delta in deltas {
            unique(&mut delta.scopes, |r| r.id, "scope")?;
            unique(&mut delta.symbols, |r| r.id, "symbol")?;
            unique(&mut delta.occurrences, |r| r.id, "occurrence")?;
            if let Some(old) = project.deltas.get(&delta.file.id) {
                if old.as_ref() != &delta {
                    return Err(invalid("conflicting duplicate file identity"));
                }
                continue;
            }
            project.scopes.extend(delta.scopes.iter().cloned());
            project.symbols.extend(delta.symbols.iter().cloned());
            project
                .occurrences
                .extend(delta.occurrences.iter().cloned());
            project.relations.extend(delta.relations.iter().cloned());
            project
                .expression_types
                .extend(delta.expression_types.iter().cloned());
            project
                .resolution_generations
                .insert(delta.file.id, delta.file.dependency_generation);
            project.deltas.insert(delta.file.id, Arc::new(delta));
        }
        normalize(&mut project)?;
        Ok(project)
    }
}
pub(super) fn normalize(project: &mut ResolvedSemanticProject) -> Result<()> {
    unique(&mut project.symbols, |r| r.id, "symbol")?;
    unique(&mut project.scopes, |r| r.id, "scope")?;
    unique(&mut project.occurrences, |r| r.id, "occurrence")?;
    let mut paths = BTreeMap::new();
    for (id, delta) in &project.deltas {
        if id != &delta.file.id || *id != crate::ids::SemanticIds::file(&delta.file.path) {
            return Err(invalid("file map key does not match its record"));
        }
        if paths.insert(&delta.file.path, *id).is_some() {
            return Err(invalid(
                "multiple identities use the same semantic file path",
            ));
        }
        if delta.scopes.iter().any(|r| r.file_id != *id)
            || delta.symbols.iter().any(|r| r.file_id != *id)
            || delta.occurrences.iter().any(|r| r.file_id != *id)
            || delta.lexical_tokens.iter().any(|r| r.file_id != *id)
            || delta.expression_types.iter().any(|r| r.file_id != *id)
            || delta.imports.iter().any(|r| r.file_id != *id)
        {
            return Err(invalid("file delta contains foreign records"));
        }
        for range in delta.ranges() {
            SourceRange::new(range.start, range.end)?;
        }
    }
    let scopes: BTreeMap<_, _> = project.scopes.iter().map(|r| (r.id, r)).collect();
    let symbols: BTreeMap<_, _> = project.symbols.iter().map(|r| (r.id, r)).collect();
    let occurrences: BTreeSet<_> = project.occurrences.iter().map(|r| r.id).collect();
    let file_exists = |id| project.deltas.contains_key(&id);
    let mut done = BTreeSet::new();
    for scope in &project.scopes {
        if !file_exists(scope.file_id) {
            return Err(invalid("scope file is absent"));
        }
        SourceRange::new(scope.range.start, scope.range.end)?;
        if scope
            .owner_symbol_id
            .is_some_and(|id| !symbols.contains_key(&id))
        {
            return Err(invalid("scope owner is absent"));
        }
        if let Some(parent) = scope.parent_id {
            let parent = scopes
                .get(&parent)
                .ok_or_else(|| invalid("scope parent is absent"))?;
            if parent.file_id != scope.file_id
                || parent.range.start > scope.range.start
                || parent.range.end < scope.range.end
            {
                return Err(invalid("scope extends outside its parent"));
            }
        }
        let mut chain = BTreeSet::new();
        let mut cursor = Some(scope.id);
        while let Some(id) = cursor {
            if done.contains(&id) {
                break;
            }
            if !chain.insert(id) {
                return Err(invalid("scope parents contain a cycle"));
            }
            cursor = scopes.get(&id).and_then(|r| r.parent_id);
        }
        done.extend(chain);
    }
    for symbol in &project.symbols {
        let scope = scopes
            .get(&symbol.scope_id)
            .ok_or_else(|| invalid("symbol scope is absent"))?;
        if scope.file_id != symbol.file_id {
            return Err(invalid("symbol scope belongs to another file"));
        }
        SourceRange::new(symbol.declaration_range.start, symbol.declaration_range.end)?;
        SourceRange::new(symbol.name_range.start, symbol.name_range.end)?;
        if symbol.name_range.start < symbol.declaration_range.start
            || symbol.name_range.end > symbol.declaration_range.end
        {
            return Err(invalid("symbol name extends outside its declaration"));
        }
        if symbol
            .owner_symbol_id
            .is_some_and(|id| !symbols.contains_key(&id))
        {
            return Err(invalid("symbol owner is absent"));
        }
        if [symbol.declared_type_id, symbol.inferred_type_id]
            .into_iter()
            .flatten()
            .any(|id| !project.types.contains_key(&id))
        {
            return Err(invalid("symbol refers to an absent type record"));
        }
    }
    for occurrence in &project.occurrences {
        if !scopes
            .get(&occurrence.scope_id)
            .is_some_and(|scope| scope.file_id == occurrence.file_id)
        {
            return Err(invalid("occurrence scope is absent or foreign"));
        }
        SourceRange::new(occurrence.range.start, occurrence.range.end)?;
        if occurrence
            .resolved_symbol_id
            .is_some_and(|id| !symbols.contains_key(&id))
        {
            return Err(invalid("occurrence resolves to an absent symbol"));
        }
        if occurrence
            .receiver_occurrence_id
            .is_some_and(|id| !occurrences.contains(&id))
        {
            return Err(invalid("receiver occurrence is absent"));
        }
    }
    for relation in &project.relations {
        if !symbols.contains_key(&relation.from) || !symbols.contains_key(&relation.to) {
            return Err(invalid("relation symbol is absent"));
        }
    }
    for (id, ty) in &project.types {
        if id != &ty.id {
            return Err(invalid("type map key does not match its record"));
        }
        validate_type(&ty.r#ref, &symbols)?;
    }
    for delta in project.deltas.values() {
        for import in &delta.imports {
            if !scopes
                .get(&import.scope_id)
                .is_some_and(|scope| scope.file_id == delta.file.id)
            {
                return Err(invalid("import scope is absent or foreign"));
            }
        }
        for ty in &delta.unresolved_types {
            if !symbols
                .get(&ty.owner_symbol_id)
                .is_some_and(|symbol| symbol.file_id == delta.file.id)
                || !scopes
                    .get(&ty.scope_id)
                    .is_some_and(|scope| scope.file_id == delta.file.id)
            {
                return Err(invalid("unresolved type owner/scope is absent or foreign"));
            }
        }
        for hint in &delta.type_hints {
            if !symbols
                .get(&hint.target_symbol_id)
                .is_some_and(|symbol| symbol.file_id == delta.file.id)
                || !scopes
                    .get(&hint.scope_id)
                    .is_some_and(|scope| scope.file_id == delta.file.id)
            {
                return Err(invalid("type hint target/scope is absent or foreign"));
            }
        }
    }
    if project
        .resolution_generations
        .keys()
        .any(|id| !file_exists(*id))
    {
        return Err(invalid("resolution generation file is absent"));
    }
    for dependency in &project.dependencies {
        if !file_exists(dependency.from_file_id) || !file_exists(dependency.to_file_id) {
            return Err(invalid("dependency file is absent"));
        }
    }
    // Best confidence wins duplicate expression ranges; empty expressions cannot
    // describe source text and must not displace a real inferred value.
    let mut expressions = BTreeMap::<_, ExpressionTypeRecord>::new();
    for expression in std::mem::take(&mut project.expression_types) {
        if !file_exists(expression.file_id) {
            return Err(invalid("expression file is absent"));
        }
        validate_type(&expression.r#type, &symbols)?;
        SourceRange::new(expression.range.start, expression.range.end)?;
        if expression.range.is_empty() {
            continue;
        }
        let key = (
            expression.file_id,
            expression.range.start,
            expression.range.end,
        );
        if expressions
            .get(&key)
            .is_none_or(|old| old.confidence.value() < expression.confidence.value())
        {
            expressions.insert(key, expression);
        }
    }
    project.expression_types = expressions.into_values().collect();
    let mut relation_indices = BTreeMap::<_, usize>::new();
    let mut relations = Vec::<RelationRecord>::new();
    for relation in std::mem::take(&mut project.relations) {
        let key = (relation.from, relation.to, relation.kind);
        if let Some(index) = relation_indices.get(&key) {
            if relations[*index].confidence.value() < relation.confidence.value() {
                relations[*index] = relation;
            }
        } else {
            relation_indices.insert(key, relations.len());
            relations.push(relation);
        }
    }
    project.relations = relations;
    project
        .dependencies
        .sort_by_key(|r| (r.from_file_id, r.to_file_id, r.kind, r.generation));
    project.dependencies.dedup();
    Ok(())
}
fn validate_type(
    ty: &TypeRef,
    symbols: &BTreeMap<crate::ids::SymbolId, &SymbolRecord>,
) -> Result<()> {
    let mut pending = vec![(ty, 0)];
    let mut visited = 0;
    while let Some((ty, depth)) = pending.pop() {
        visited += 1;
        if depth > 64 || visited > 100_000 {
            return Err(invalid("semantic type shape exceeds work limits"));
        }
        match ty {
            TypeRef::Named { symbol_id } if !symbols.contains_key(symbol_id) => {
                return Err(invalid("named type symbol is absent"));
            }
            TypeRef::Generic { base, arguments } => {
                pending.push((base, depth + 1));
                pending.extend(arguments.iter().map(|ty| (ty, depth + 1)));
            }
            TypeRef::Nullable { inner } => pending.push((inner, depth + 1)),
            TypeRef::Union { alternatives } => {
                pending.extend(alternatives.iter().map(|ty| (ty, depth + 1)))
            }
            TypeRef::Function {
                parameters,
                returns,
            } => {
                pending.extend(parameters.iter().map(|ty| (ty, depth + 1)));
                if let Some(returns) = returns {
                    pending.push((returns, depth + 1));
                }
            }
            _ => {}
        }
    }
    Ok(())
}
