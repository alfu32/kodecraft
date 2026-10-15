use super::dependencies::DependencyGraph;
use crate::{ids::FileId, model::*};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SemanticInvalidationPlan {
    /// Includes removed files so consumers can discard their overlays.
    pub affected_files: BTreeSet<FileId>,
    pub content_changed_files: BTreeSet<FileId>,
    pub surface_changed_files: BTreeSet<FileId>,
    pub removed_files: BTreeSet<FileId>,
}
pub struct SemanticInvalidationPlanner;
impl SemanticInvalidationPlanner {
    pub fn plan(
        previous: &ResolvedSemanticProject,
        updates: &BTreeMap<FileId, FileSemanticDelta>,
        removed: &BTreeSet<FileId>,
    ) -> SemanticInvalidationPlan {
        let mut plan = SemanticInvalidationPlan::default();
        for (id, update) in updates {
            let old = previous.deltas.get(id);
            if old.is_none_or(|old| {
                old.file.content_hash != update.file.content_hash
                    || old.file.language_id != update.file.language_id
                    || old.exported_surface_hash != update.exported_surface_hash
            }) {
                plan.content_changed_files.insert(*id);
            }
            if old.is_none_or(|old| {
                old.exported_surface_hash != update.exported_surface_hash
                    || old.file.language_id != update.file.language_id
            }) {
                plan.surface_changed_files.insert(*id);
            }
        }
        for id in removed.iter().filter(|id| previous.deltas.contains_key(id)) {
            plan.removed_files.insert(*id);
            plan.surface_changed_files.insert(*id);
        }
        let graph = DependencyGraph::new(&previous.dependencies);
        plan.affected_files = graph.dependent_closure(plan.surface_changed_files.iter().copied());
        plan.affected_files.extend(&plan.content_changed_files);
        let mut names = BTreeSet::new();
        for id in &plan.surface_changed_files {
            if let Some(old) = previous.deltas.get(id) {
                exported_names(old, &mut names);
            }
            if let Some(update) = updates.get(id) {
                exported_names(update, &mut names);
            }
        }
        // Raw facts deliberately include previously resolved references: a newly
        // visible duplicate declaration may make an old binding ambiguous too.
        if !names.is_empty() {
            for (id, delta) in &previous.deltas {
                if !plan.affected_files.contains(id) && may_reference(delta, &names) {
                    plan.affected_files.insert(*id);
                }
            }
            plan.affected_files = graph.dependent_closure(plan.affected_files.iter().copied());
        }
        plan
    }
}
fn exported_names(delta: &FileSemanticDelta, names: &mut BTreeSet<String>) {
    let symbols: BTreeMap<_, _> = delta.symbols.iter().map(|s| (s.id, s)).collect();
    for symbol in &delta.symbols {
        if symbol.owner_symbol_id.is_none_or(|id| {
            symbols.get(&id).is_some_and(|s| {
                matches!(
                    s.kind,
                    SymbolKind::Class
                        | SymbolKind::Interface
                        | SymbolKind::Struct
                        | SymbolKind::Enum
                        | SymbolKind::TypeAlias
                )
            })
        }) {
            names.insert(symbol.name.clone());
            if let Some(name) = &symbol.qualified_name {
                names.insert(name.clone());
            }
        }
    }
}
fn may_reference(delta: &FileSemanticDelta, names: &BTreeSet<String>) -> bool {
    let matches = |name: &str| {
        names.contains(name) || names.contains(name.rsplit('.').next().unwrap_or(name))
    };
    delta.unresolved_types.iter().any(|t| matches(&t.name))
        || delta.type_hints.iter().any(|h| matches(&h.referenced_name))
        || delta
            .occurrences
            .iter()
            .any(|o| o.kind != OccurrenceKind::Declaration && matches(&o.text))
        || delta.imports.iter().any(|i| matches(&i.path))
}
