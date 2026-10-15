//! Worker-side incremental preparation. L05 commits before L09 notifies readers.
pub mod dependencies;
pub mod invalidation;
use crate::{
    ids::*,
    model::*,
    resolver::{ResolutionReport, SemanticResolver},
};
use invalidation::{SemanticInvalidationPlan, SemanticInvalidationPlanner};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Use with document freshness and snapshot revision gates in the coordinator.
/// A dependent's generation changes even when its document version stays fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCacheKey {
    pub file: FileId,
    pub content_hash: String,
    pub parse_version: u64,
    pub resolution_generation: u64,
}
impl SemanticCacheKey {
    pub fn for_file(project: &ResolvedSemanticProject, file: FileId) -> Option<Self> {
        let delta = project.deltas.get(&file)?;
        Some(Self {
            file,
            content_hash: delta.file.content_hash.clone(),
            parse_version: delta.file.parse_version,
            resolution_generation: *project.resolution_generations.get(&file)?,
        })
    }
}
pub struct IncrementalResolution {
    pub report: ResolutionReport,
    /// Deliver to token/completion consumers only after persistence succeeds.
    pub invalidation: SemanticInvalidationPlan,
}
pub struct IncrementalSemanticIndex;
impl IncrementalSemanticIndex {
    pub fn prepare(
        previous: &ResolvedSemanticProject,
        updates: impl IntoIterator<Item = FileSemanticDelta>,
        removed: impl IntoIterator<Item = FileId>,
        cancel: &CancellationToken,
    ) -> Result<IncrementalResolution> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let mut unique = BTreeMap::new();
        for delta in updates {
            if let Some(old) = unique.insert(delta.file.id, delta.clone())
                && old != delta
            {
                return Err(invalid("conflicting updates for one file"));
            }
        }
        let removed: BTreeSet<_> = removed.into_iter().collect();
        if removed.iter().any(|id| unique.contains_key(id)) {
            return Err(invalid("cannot update and remove the same file"));
        }
        let invalidation = SemanticInvalidationPlanner::plan(previous, &unique, &removed);
        let mut deltas = previous.deltas.clone();
        for id in &removed {
            deltas.remove(id);
        }
        for (id, delta) in unique {
            // Retain extraction Arc identity for untouched and exact no-op facts.
            if deltas.get(&id).is_none_or(|old| old.as_ref() != &delta) {
                deltas.insert(id, Arc::new(delta));
            }
        }
        let mut project =
            ResolvedSemanticProject::from_deltas(deltas.values().map(|d| d.as_ref().clone()))?;
        project.deltas = deltas;
        let active: BTreeSet<_> = invalidation
            .affected_files
            .iter()
            .filter(|id| project.deltas.contains_key(id))
            .copied()
            .collect();
        let old_symbols: BTreeMap<_, _> = previous.symbols.iter().map(|s| (s.id, s)).collect();
        for symbol in &mut project.symbols {
            if !active.contains(&symbol.file_id)
                && let Some(old) = old_symbols.get(&symbol.id)
            {
                *symbol = (*old).clone();
            }
        }
        let old_occurrences: BTreeMap<_, _> =
            previous.occurrences.iter().map(|o| (o.id, o)).collect();
        for occurrence in &mut project.occurrences {
            if !active.contains(&occurrence.file_id)
                && let Some(old) = old_occurrences.get(&occurrence.id)
            {
                *occurrence = (*old).clone();
            }
        }
        let ids: BTreeSet<_> = project.symbols.iter().map(|s| s.id).collect();
        let file_by_symbol: BTreeMap<_, _> =
            project.symbols.iter().map(|s| (s.id, s.file_id)).collect();
        project
            .relations
            .retain(|r| active.contains(&file_by_symbol[&r.from]));
        project.relations.extend(
            previous
                .relations
                .iter()
                .filter(|r| {
                    file_by_symbol
                        .get(&r.from)
                        .is_some_and(|file| !active.contains(file))
                        && ids.contains(&r.to)
                })
                .cloned(),
        );
        project
            .expression_types
            .retain(|e| active.contains(&e.file_id));
        project.expression_types.extend(
            previous
                .expression_types
                .iter()
                .filter(|e| project.deltas.contains_key(&e.file_id) && !active.contains(&e.file_id))
                .cloned(),
        );
        project.types = previous
            .types
            .iter()
            .filter(|(_, t)| valid_type(&t.r#ref, &ids))
            .map(|(id, t)| (*id, t.clone()))
            .collect();
        for id in project.deltas.keys() {
            let generation = previous
                .resolution_generations
                .get(id)
                .copied()
                .unwrap_or(0);
            let generation = if active.contains(id) {
                generation
                    .checked_add(1)
                    .ok_or_else(|| invalid("resolution generation exhausted"))?
            } else {
                generation
            };
            project.resolution_generations.insert(*id, generation);
        }
        let report = SemanticResolver::resolve_affected(project, active, cancel)?;
        Ok(IncrementalResolution {
            report,
            invalidation,
        })
    }
}
fn valid_type(ty: &TypeRef, ids: &BTreeSet<SymbolId>) -> bool {
    match ty {
        TypeRef::Named { symbol_id } => ids.contains(symbol_id),
        TypeRef::Generic { base, arguments } => {
            valid_type(base, ids) && arguments.iter().all(|t| valid_type(t, ids))
        }
        TypeRef::Function {
            parameters,
            returns,
        } => {
            parameters.iter().all(|t| valid_type(t, ids))
                && returns.as_ref().is_none_or(|t| valid_type(t, ids))
        }
        TypeRef::Union { alternatives } => alternatives.iter().all(|t| valid_type(t, ids)),
        TypeRef::Nullable { inner } => valid_type(inner, ids),
        TypeRef::Primitive { .. } | TypeRef::Unknown => true,
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "incremental semantic index",
        message: message.into(),
    }
}
