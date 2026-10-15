//! Deterministic native resolution. Parsing and publication remain separate.
mod declared_types;
mod expressions;
pub use expressions::BestEffortExpressionTypeResolver;
mod imports;
mod members;
mod names;
mod relations;
mod scopes;
mod types;
use crate::{ids::*, model::*};
use kode_core::{ByteOffset, DomainError, Result};
use kode_platform::jobs::CancellationToken;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedName {
    pub file_id: FileId,
    pub scope_id: ScopeId,
    pub name: String,
    pub range: SourceRange,
    pub kind: OccurrenceKind,
}
pub struct ResolutionReport {
    pub project: ResolvedSemanticProject,
    pub unresolved: Vec<UnresolvedName>,
    pub notes: Vec<String>,
}
pub struct SemanticResolver;
impl SemanticResolver {
    pub fn resolve(
        deltas: impl IntoIterator<Item = FileSemanticDelta>,
        cancel: &CancellationToken,
    ) -> Result<ResolutionReport> {
        Self::run(State::new(deltas, cancel)?)
    }
    pub(crate) fn resolve_affected(
        project: ResolvedSemanticProject,
        affected: BTreeSet<FileId>,
        cancel: &CancellationToken,
    ) -> Result<ResolutionReport> {
        Self::run(State::from_project(project, affected, cancel)?)
    }
    fn run(mut state: State<'_>) -> Result<ResolutionReport> {
        state.resolve_declared_types()?;
        state.resolve_occurrences()?;
        state.infer_types()?;
        // Member identities are recomputed after inference and union merging.
        for occurrence in &mut state.project.occurrences {
            if state.active_files.contains(&occurrence.file_id)
                && occurrence.kind == OccurrenceKind::MemberReference
            {
                occurrence.resolved_symbol_id = None;
            }
        }
        state.resolve_occurrences()?;
        state.publish_expression_types()?;
        state.derive_dependencies()?;
        state.finish()
    }
}
pub(super) struct State<'a> {
    pub project: ResolvedSemanticProject,
    pub active_files: BTreeSet<FileId>,
    pub symbols: BTreeMap<SymbolId, usize>,
    pub scopes: BTreeMap<ScopeId, usize>,
    pub by_name: BTreeMap<String, Vec<SymbolId>>,
    pub qualified: BTreeMap<String, Vec<SymbolId>>,
    pub scoped: BTreeMap<ScopeId, BTreeMap<String, Vec<SymbolId>>>,
    member_names: BTreeMap<SymbolId, BTreeMap<String, Vec<SymbolId>>>,
    base_edges: BTreeMap<SymbolId, Vec<(SymbolId, RelationKind)>>,
    c_type_contexts: BTreeMap<FileId, names::CTypeContexts>,
    pub outgoing: BTreeMap<SymbolId, Vec<usize>>,
    pub occurrence_indices: BTreeMap<OccurrenceId, usize>,
    pub expression_syntax: BTreeMap<expressions::Key, ExpressionSyntaxRecord>,
    pub notes: Vec<String>,
    pub cancel: &'a CancellationToken,
    started: Instant,
    operations: usize,
}
impl<'a> State<'a> {
    fn new(
        deltas: impl IntoIterator<Item = FileSemanticDelta>,
        cancel: &'a CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let mut deltas: Vec<_> = deltas.into_iter().collect();
        deltas.sort_by_key(|d| d.file.id);
        let project = ResolvedSemanticProject::from_deltas(deltas)?;
        let active = project.deltas.keys().copied().collect();
        Self::from_project(project, active, cancel)
    }
    fn from_project(
        mut project: ResolvedSemanticProject,
        active_files: BTreeSet<FileId>,
        cancel: &'a CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        project.symbols.sort_by_key(|s| s.id);
        project.scopes.sort_by_key(|s| s.id);
        project
            .occurrences
            .sort_by_key(|o| (o.file_id, o.range.start, o.range.end));
        let symbols = project
            .symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        let scopes = project
            .scopes
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        let mut state = Self {
            project,
            active_files,
            symbols,
            scopes,
            by_name: BTreeMap::new(),
            qualified: BTreeMap::new(),
            scoped: BTreeMap::new(),
            member_names: BTreeMap::new(),
            base_edges: BTreeMap::new(),
            c_type_contexts: BTreeMap::new(),
            outgoing: BTreeMap::new(),
            occurrence_indices: BTreeMap::new(),
            expression_syntax: BTreeMap::new(),
            notes: vec![],
            cancel,
            started: Instant::now(),
            operations: 0,
        };
        for symbol in &state.project.symbols {
            state
                .by_name
                .entry(symbol.name.clone())
                .or_default()
                .push(symbol.id);
            if let Some(name) = &symbol.qualified_name {
                state
                    .qualified
                    .entry(name.clone())
                    .or_default()
                    .push(symbol.id);
            }
            state
                .scoped
                .entry(symbol.scope_id)
                .or_default()
                .entry(symbol.name.clone())
                .or_default()
                .push(symbol.id);
        }
        for (i, relation) in state.project.relations.iter().enumerate() {
            state.outgoing.entry(relation.from).or_default().push(i);
            if relation.kind == RelationKind::Contains
                && let Some(symbol) = state.symbol(relation.to)
            {
                let name = symbol.name.clone();
                state
                    .member_names
                    .entry(relation.from)
                    .or_default()
                    .entry(name)
                    .or_default()
                    .push(relation.to);
            }
            if matches!(
                relation.kind,
                RelationKind::Extends | RelationKind::Implements | RelationKind::AliasOf
            ) {
                state
                    .base_edges
                    .entry(relation.from)
                    .or_default()
                    .push((relation.to, relation.kind));
            }
        }
        for (i, occurrence) in state.project.occurrences.iter().enumerate() {
            state.occurrence_indices.insert(occurrence.id, i);
        }
        for delta in state.project.deltas.values() {
            if delta.file.language_id == "c" {
                state
                    .c_type_contexts
                    .insert(delta.file.id, names::CTypeContexts::new(delta));
            }
            for expression in &delta.syntax_expressions {
                state.expression_syntax.insert(
                    (delta.file.id, expression.range.start, expression.range.end),
                    expression.clone(),
                );
            }
        }
        state.check()?;
        Ok(state)
    }
    pub fn check(&mut self) -> Result<()> {
        self.operations += 1;
        if self.cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if self.operations > 2_000_000 || self.started.elapsed() > Duration::from_secs(5) {
            return Err(DomainError::Unsupported(
                "semantic resolution work limit exceeded".into(),
            ));
        }
        Ok(())
    }
    pub fn symbol(&self, id: SymbolId) -> Option<&SymbolRecord> {
        self.symbols.get(&id).map(|i| &self.project.symbols[*i])
    }
    pub fn scope(&self, id: ScopeId) -> Option<&ScopeRecord> {
        self.scopes.get(&id).map(|i| &self.project.scopes[*i])
    }
    pub fn language(&self, file: FileId) -> &str {
        self.project
            .deltas
            .get(&file)
            .map_or("", |d| d.file.language_id.as_str())
    }
    pub fn is_global(&self, symbol: &SymbolRecord) -> bool {
        self.scope(symbol.scope_id).is_some_and(|s| {
            matches!(
                s.kind,
                ScopeKind::File | ScopeKind::Package | ScopeKind::Module | ScopeKind::Namespace
            )
        })
    }
    pub fn relation(
        &mut self,
        from: SymbolId,
        to: SymbolId,
        kind: RelationKind,
        confidence: Confidence,
    ) {
        if self.outgoing.get(&from).into_iter().flatten().any(|i| {
            let r = &self.project.relations[*i];
            r.to == to && r.kind == kind
        }) {
            return;
        }
        self.outgoing
            .entry(from)
            .or_default()
            .push(self.project.relations.len());
        self.project.relations.push(RelationRecord {
            from,
            to,
            kind,
            confidence,
        });
        if kind == RelationKind::Contains
            && let Some(symbol) = self.symbol(to)
        {
            let name = symbol.name.clone();
            self.member_names
                .entry(from)
                .or_default()
                .entry(name)
                .or_default()
                .push(to);
        }
        if matches!(
            kind,
            RelationKind::Extends | RelationKind::Implements | RelationKind::AliasOf
        ) {
            self.base_edges.entry(from).or_default().push((to, kind));
        }
    }
    fn finish(mut self) -> Result<ResolutionReport> {
        self.check()?;
        let mut unresolved = vec![];
        for occurrence in &self.project.occurrences {
            if occurrence.resolved_symbol_id.is_none()
                && !declared_types::is_primitive(
                    &occurrence.text,
                    self.language(occurrence.file_id),
                )
            {
                unresolved.push(UnresolvedName {
                    file_id: occurrence.file_id,
                    scope_id: occurrence.scope_id,
                    name: occurrence.text.clone(),
                    range: occurrence.range,
                    kind: occurrence.kind,
                });
            }
        }
        self.project
            .relations
            .sort_by_key(|r| (r.from, r.to, r.kind));
        self.project
            .dependencies
            .sort_by_key(|d| (d.from_file_id, d.to_file_id, d.kind));
        // Validate the enriched representation using exactly the query/store gate.
        let project = crate::snapshot::SemanticSnapshot::new(0, self.project)?
            .project()
            .clone();
        Ok(ResolutionReport {
            project,
            unresolved,
            notes: self.notes,
        })
    }
    fn resolve_occurrences(&mut self) -> Result<()> {
        let mut resolved = BTreeMap::<OccurrenceId, OccurrenceRecord>::new();
        for mut occurrence in self.project.occurrences.clone() {
            if !self.active_files.contains(&occurrence.file_id) {
                resolved.insert(occurrence.id, occurrence);
                continue;
            }
            self.check()?;
            let target = occurrence.resolved_symbol_id.or_else(|| {
                if occurrence.text == "this" {
                    self.owner_type(occurrence.scope_id)
                } else if occurrence.text == "super" {
                    self.owner_type(occurrence.scope_id)
                        .and_then(|owner| self.base_type(owner))
                } else if occurrence.kind == OccurrenceKind::MemberReference {
                    let receiver = occurrence
                        .receiver_occurrence_id
                        .and_then(|id| resolved.get(&id));
                    let symbol = receiver
                        .and_then(|o| o.resolved_symbol_id)
                        .and_then(|id| self.symbol(id));
                    let ty = symbol.and_then(|s| {
                        self.occurrence_type(receiver.expect("receiver accompanies symbol"), s)
                    });
                    ty.as_ref().and_then(primary_named).and_then(|owner| {
                        self.named_members(owner, &occurrence.text)
                            .into_iter()
                            .find(|id| {
                                self.symbol(*id).is_some_and(|s| {
                                    s.name == occurrence.text
                                        && self.accessible(
                                            s,
                                            occurrence.file_id,
                                            occurrence.scope_id,
                                        )
                                })
                            })
                    })
                } else if occurrence.kind == OccurrenceKind::Import {
                    self.import_occurrence(&occurrence)
                } else {
                    let kind = self.occurrence_kind(&occurrence);
                    self.find_name(
                        &occurrence.text,
                        occurrence.scope_id,
                        occurrence.file_id,
                        occurrence.range.start,
                        kind,
                        None,
                    )
                }
            });
            if let Some(id) = target {
                occurrence.resolved_symbol_id = Some(id);
                occurrence.confidence = Confidence::inferred(0.95)?;
                if occurrence.kind == OccurrenceKind::Call
                    && let Some(owner) = self.owner_symbol(occurrence.scope_id)
                {
                    self.relation(owner, id, RelationKind::Calls, Confidence::inferred(0.95)?);
                }
            }
            resolved.insert(occurrence.id, occurrence);
        }
        self.project.occurrences = resolved.into_values().collect();
        self.project
            .occurrences
            .sort_by_key(|o| (o.file_id, o.range.start, o.range.end));
        Ok(())
    }
    pub fn occurrence_type(
        &self,
        occurrence: &OccurrenceRecord,
        symbol: &SymbolRecord,
    ) -> Option<TypeRef> {
        if is_type(symbol.kind)
            && (matches!(
                occurrence.kind,
                OccurrenceKind::Call | OccurrenceKind::TypeReference
            ) || matches!(occurrence.text.as_str(), "this" | "super"))
        {
            return Some(TypeRef::named(symbol.id));
        }
        symbol
            .declared_type_id
            .or(symbol.inferred_type_id)
            .and_then(|id| self.project.types.get(&id))
            .map(|t| t.r#ref.clone())
    }
}
#[derive(Clone, Copy)]
pub(super) enum NameKind {
    Any,
    Type,
    Callable,
    Tag(SymbolKind),
}
pub(super) fn is_type(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::TypeAlias
    )
}
pub(super) fn primary_named(ty: &TypeRef) -> Option<SymbolId> {
    match ty {
        TypeRef::Named { symbol_id } => Some(*symbol_id),
        TypeRef::Generic { base, .. } | TypeRef::Nullable { inner: base } => primary_named(base),
        _ => None,
    }
}
