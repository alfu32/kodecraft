//! Query tables are prepared before publication; readers share immutable state.
mod prepare;
use crate::{ids::*, model::*};
use kode_core::{ByteOffset, DomainError, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

#[derive(Debug, Default)]
struct RangeIndex {
    indices: Vec<usize>,
    prefix_end: Vec<ByteOffset>,
}
impl RangeIndex {
    fn new(mut indices: Vec<usize>, range: impl Fn(usize) -> SourceRange) -> Self {
        indices.sort_by_key(|index| {
            let r = range(*index);
            (r.start, r.end, *index)
        });
        let mut maximum = ByteOffset(0);
        let prefix_end = indices
            .iter()
            .map(|index| {
                maximum = maximum.max(range(*index).end);
                maximum
            })
            .collect();
        Self {
            indices,
            prefix_end,
        }
    }
    fn overlapping<'a>(
        &'a self,
        query: SourceRange,
        range: impl Fn(usize) -> SourceRange + Copy + 'a,
    ) -> impl Iterator<Item = usize> + 'a {
        let first = self.prefix_end.partition_point(|end| *end <= query.start);
        self.indices[first..]
            .iter()
            .copied()
            .take_while(move |index| range(*index).start < query.end)
            .filter(move |index| !query.is_empty() && range(*index).end > query.start)
    }
}
#[derive(Debug)]
struct SnapshotData {
    version: u64,
    project: ResolvedSemanticProject,
    files_by_path: BTreeMap<String, FileId>,
    symbols_by_id: BTreeMap<SymbolId, usize>,
    scopes_by_id: BTreeMap<ScopeId, usize>,
    symbols_by_file: BTreeMap<FileId, Vec<usize>>,
    symbols_by_scope: BTreeMap<ScopeId, Vec<usize>>,
    scopes_by_file: BTreeMap<FileId, Vec<usize>>,
    occurrences_by_file: BTreeMap<FileId, RangeIndex>,
    occurrences_by_symbol: BTreeMap<SymbolId, Vec<usize>>,
    lexical: Vec<LexicalTokenRecord>,
    lexical_by_file: BTreeMap<FileId, RangeIndex>,
    expressions_by_file: BTreeMap<FileId, Vec<usize>>,
    relations_from: BTreeMap<SymbolId, Vec<usize>>,
    dependencies_from: BTreeMap<FileId, Vec<usize>>,
    dependencies_to: BTreeMap<FileId, Vec<usize>>,
}
#[derive(Debug, Clone)]
pub struct SemanticSnapshot(Arc<SnapshotData>);
impl SemanticSnapshot {
    /// Call on an indexing worker. No reader constructs query tables or resolves
    /// facts, and callers cannot mutate a published project through this API.
    pub fn new(version: u64, mut project: ResolvedSemanticProject) -> Result<Self> {
        prepare::normalize(&mut project)?;
        let mut data = SnapshotData {
            version,
            project,
            files_by_path: Default::default(),
            symbols_by_id: Default::default(),
            scopes_by_id: Default::default(),
            symbols_by_file: Default::default(),
            symbols_by_scope: Default::default(),
            scopes_by_file: Default::default(),
            occurrences_by_file: Default::default(),
            occurrences_by_symbol: Default::default(),
            lexical: vec![],
            lexical_by_file: Default::default(),
            expressions_by_file: Default::default(),
            relations_from: Default::default(),
            dependencies_from: Default::default(),
            dependencies_to: Default::default(),
        };
        for (file, delta) in &data.project.deltas {
            data.files_by_path.insert(delta.file.path.clone(), *file);
            data.lexical.extend(delta.lexical_tokens.iter().cloned());
        }
        for (index, symbol) in data.project.symbols.iter().enumerate() {
            data.symbols_by_id.insert(symbol.id, index);
            data.symbols_by_file
                .entry(symbol.file_id)
                .or_default()
                .push(index);
            data.symbols_by_scope
                .entry(symbol.scope_id)
                .or_default()
                .push(index);
        }
        for (index, scope) in data.project.scopes.iter().enumerate() {
            data.scopes_by_id.insert(scope.id, index);
            data.scopes_by_file
                .entry(scope.file_id)
                .or_default()
                .push(index);
        }
        let mut occurrence_groups = BTreeMap::<_, Vec<usize>>::new();
        for (index, occurrence) in data.project.occurrences.iter().enumerate() {
            occurrence_groups
                .entry(occurrence.file_id)
                .or_default()
                .push(index);
            if let Some(symbol) = occurrence.resolved_symbol_id {
                data.occurrences_by_symbol
                    .entry(symbol)
                    .or_default()
                    .push(index);
            }
        }
        data.occurrences_by_file = occurrence_groups
            .into_iter()
            .map(|(file, indices)| {
                (
                    file,
                    RangeIndex::new(indices, |index| data.project.occurrences[index].range),
                )
            })
            .collect();
        let mut lexical_groups = BTreeMap::<_, Vec<usize>>::new();
        for (index, token) in data.lexical.iter().enumerate() {
            lexical_groups.entry(token.file_id).or_default().push(index);
        }
        data.lexical_by_file = lexical_groups
            .into_iter()
            .map(|(file, indices)| {
                (
                    file,
                    RangeIndex::new(indices, |index| data.lexical[index].range),
                )
            })
            .collect();
        for (index, expression) in data.project.expression_types.iter().enumerate() {
            data.expressions_by_file
                .entry(expression.file_id)
                .or_default()
                .push(index);
        }
        for (index, relation) in data.project.relations.iter().enumerate() {
            data.relations_from
                .entry(relation.from)
                .or_default()
                .push(index);
        }
        for (index, dependency) in data.project.dependencies.iter().enumerate() {
            data.dependencies_from
                .entry(dependency.from_file_id)
                .or_default()
                .push(index);
            data.dependencies_to
                .entry(dependency.to_file_id)
                .or_default()
                .push(index);
        }
        Ok(Self(Arc::new(data)))
    }
    pub fn empty() -> Self {
        Self::new(0, ResolvedSemanticProject::default()).expect("empty facts are valid")
    }
    pub fn version(&self) -> u64 {
        self.0.version
    }
    pub fn project(&self) -> &ResolvedSemanticProject {
        &self.0.project
    }
    pub fn file(&self, id: FileId) -> Option<&FileRecord> {
        self.0.project.deltas.get(&id).map(|delta| &delta.file)
    }
    pub fn file_at_path(&self, path: &str) -> Option<&FileRecord> {
        self.file(*self.0.files_by_path.get(path)?)
    }
    pub fn symbol(&self, id: SymbolId) -> Option<&SymbolRecord> {
        Some(&self.0.project.symbols[*self.0.symbols_by_id.get(&id)?])
    }
    pub fn scope(&self, id: ScopeId) -> Option<&ScopeRecord> {
        Some(&self.0.project.scopes[*self.0.scopes_by_id.get(&id)?])
    }
    pub fn symbol_at(&self, file: FileId, offset: ByteOffset) -> Option<&SymbolRecord> {
        if let Some(symbol) = self
            .occurrence_at(file, offset)
            .and_then(|occurrence| occurrence.resolved_symbol_id)
            .and_then(|id| self.symbol(id))
        {
            return Some(symbol);
        }
        self.symbols(file)
            .find(|symbol| symbol.name_range.contains(offset))
    }
    pub fn occurrence_at(&self, file: FileId, offset: ByteOffset) -> Option<&OccurrenceRecord> {
        // A caret immediately after an identifier still queries that occurrence.
        let index = self.0.occurrences_by_file.get(&file)?;
        let first = index.prefix_end.partition_point(|end| *end < offset);
        index.indices[first..]
            .iter()
            .copied()
            .take_while(|i| self.0.project.occurrences[*i].range.start <= offset)
            .map(|i| &self.0.project.occurrences[i])
            .find(|occurrence| occurrence.range.contains(offset) || occurrence.range.end == offset)
    }
    pub fn occurrences(&self, file: FileId) -> impl Iterator<Item = &OccurrenceRecord> {
        self.0
            .occurrences_by_file
            .get(&file)
            .into_iter()
            .flat_map(|index| &index.indices)
            .map(|index| &self.0.project.occurrences[*index])
    }
    pub fn references(&self, symbol: SymbolId) -> impl Iterator<Item = &OccurrenceRecord> {
        self.0
            .occurrences_by_symbol
            .get(&symbol)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.occurrences[*index])
    }
    pub fn occurrences_in(
        &self,
        file: FileId,
        range: SourceRange,
    ) -> impl Iterator<Item = &OccurrenceRecord> {
        self.0
            .occurrences_by_file
            .get(&file)
            .into_iter()
            .flat_map(move |index| {
                index.overlapping(range, |index| self.0.project.occurrences[index].range)
            })
            .map(|index| &self.0.project.occurrences[index])
    }
    pub fn lexical_tokens(&self, file: FileId) -> impl Iterator<Item = &LexicalTokenRecord> {
        self.0
            .lexical_by_file
            .get(&file)
            .into_iter()
            .flat_map(|index| &index.indices)
            .map(|index| &self.0.lexical[*index])
    }
    pub fn lexical_tokens_in(
        &self,
        file: FileId,
        range: SourceRange,
    ) -> impl Iterator<Item = &LexicalTokenRecord> {
        self.0
            .lexical_by_file
            .get(&file)
            .into_iter()
            .flat_map(move |index| index.overlapping(range, |index| self.0.lexical[index].range))
            .map(|index| &self.0.lexical[index])
    }
    pub fn expression_type(&self, file: FileId, range: SourceRange) -> Option<&TypeRef> {
        self.0
            .expressions_by_file
            .get(&file)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.expression_types[*index])
            .filter(|expression| {
                expression.range.start >= range.start && expression.range.end <= range.end
            })
            .max_by_key(|expression| expression.range.len())
            .map(|expression| &expression.r#type)
    }
    pub fn symbols(&self, file: FileId) -> impl Iterator<Item = &SymbolRecord> {
        self.0
            .symbols_by_file
            .get(&file)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.symbols[*index])
    }
    pub fn workspace_symbols(&self) -> impl Iterator<Item = &SymbolRecord> {
        self.0.project.symbols.iter()
    }
    pub fn scopes(&self, file: FileId) -> impl Iterator<Item = &ScopeRecord> {
        self.0
            .scopes_by_file
            .get(&file)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.scopes[*index])
    }
    pub fn relations(
        &self,
        from: SymbolId,
        kind: Option<RelationKind>,
    ) -> impl Iterator<Item = &RelationRecord> {
        self.0
            .relations_from
            .get(&from)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.relations[*index])
            .filter(move |relation| kind.is_none_or(|kind| kind == relation.kind))
    }
    pub fn dependencies(&self, file: FileId) -> impl Iterator<Item = &FileDependencyRecord> {
        self.0
            .dependencies_from
            .get(&file)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.dependencies[*index])
    }
    pub fn dependents(&self, file: FileId) -> impl Iterator<Item = &FileDependencyRecord> {
        self.0
            .dependencies_to
            .get(&file)
            .into_iter()
            .flatten()
            .map(|index| &self.0.project.dependencies[*index])
    }
    pub fn resolution_generation(&self, file: FileId) -> u64 {
        self.0
            .project
            .resolution_generations
            .get(&file)
            .copied()
            .unwrap_or(0)
    }
    pub fn type_record(&self, id: TypeId) -> Option<&TypeRecord> {
        self.0.project.types.get(&id)
    }
    pub fn members(&self, ty: &TypeRef) -> Vec<&SymbolRecord> {
        match ty {
            TypeRef::Named { symbol_id } => self.named_members(*symbol_id),
            TypeRef::Generic { base, .. } | TypeRef::Nullable { inner: base } => self.members(base),
            TypeRef::Union { alternatives } => {
                let Some(first) = alternatives.first() else {
                    return vec![];
                };
                let mut members = self.members(first);
                for other in alternatives.iter().skip(1) {
                    let shared: BTreeSet<_> = self
                        .members(other)
                        .into_iter()
                        .map(|symbol| (&symbol.name, symbol.kind))
                        .collect();
                    members.retain(|symbol| shared.contains(&(&symbol.name, symbol.kind)));
                }
                members
            }
            _ => vec![],
        }
    }
    fn named_members(&self, owner: SymbolId) -> Vec<&SymbolRecord> {
        let mut visited = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut result = vec![];
        let mut pending = vec![owner];
        while let Some(owner) = pending.pop() {
            if !visited.insert(owner) {
                continue;
            }
            for relation in self.relations(owner, Some(RelationKind::Contains)) {
                if let Some(symbol) = self.symbol(relation.to)
                    && names.insert((&symbol.name, symbol.kind))
                {
                    result.push(symbol);
                }
            }
            let bases: Vec<_> = self
                .relations(owner, None)
                .filter(|relation| {
                    matches!(
                        relation.kind,
                        RelationKind::Extends | RelationKind::Implements | RelationKind::AliasOf
                    )
                })
                .map(|relation| relation.to)
                .collect();
            pending.extend(bases.into_iter().rev());
        }
        result
    }
    fn scope_chain(&self, file: FileId, offset: ByteOffset) -> Vec<ScopeId> {
        let mut cursor = self
            .scopes(file)
            .filter(|scope| scope.range.contains(offset) || scope.range.end == offset)
            .min_by_key(|scope| scope.range.len())
            .map(|scope| scope.id);
        let mut chain = vec![];
        let mut visited = BTreeSet::new();
        while let Some(id) = cursor {
            if !visited.insert(id) {
                break;
            }
            chain.push(id);
            cursor = self.scope(id).and_then(|scope| scope.parent_id);
        }
        chain
    }
    fn owner_type_at(&self, file: FileId, offset: ByteOffset) -> Option<&SymbolRecord> {
        self.scope_chain(file, offset)
            .into_iter()
            .filter_map(|id| self.scope(id))
            .filter_map(|scope| scope.owner_symbol_id)
            .filter_map(|id| self.symbol(id))
            .find(|symbol| is_type(symbol.kind))
    }
    fn inherits_from(&self, ty: SymbolId, base: SymbolId) -> bool {
        let mut pending = vec![ty];
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            for relation in self.relations(id, None).filter(|relation| {
                matches!(
                    relation.kind,
                    RelationKind::Extends | RelationKind::Implements
                )
            }) {
                if relation.to == base {
                    return true;
                }
                pending.push(relation.to);
            }
        }
        false
    }
    pub fn is_accessible(&self, symbol: &SymbolRecord, file: FileId, offset: ByteOffset) -> bool {
        let visibility = symbol.flags & SymbolFlags::VISIBILITY_MASK;
        if matches!(visibility, 0 | SymbolFlags::PUBLIC | SymbolFlags::INTERNAL) {
            return true;
        }
        let Some(owner) = symbol.owner_symbol_id else {
            return symbol.file_id == file;
        };
        let Some(current) = self.owner_type_at(file, offset) else {
            return false;
        };
        if visibility == SymbolFlags::PRIVATE {
            return current.id == owner;
        }
        if visibility == SymbolFlags::PROTECTED {
            return current.id == owner || self.inherits_from(current.id, owner);
        }
        true
    }
    pub fn visible_symbols(&self, file: FileId, offset: ByteOffset) -> Vec<&SymbolRecord> {
        let chain = self.scope_chain(file, offset);
        let mut result = vec![];
        let mut names = BTreeSet::new();
        for scope in &chain {
            for index in self.0.symbols_by_scope.get(scope).into_iter().flatten() {
                let symbol = &self.0.project.symbols[*index];
                if (symbol.file_id != file
                    || !matches!(symbol.kind, SymbolKind::Variable | SymbolKind::Parameter)
                    || symbol.name_range.start <= offset)
                    && names.insert(&symbol.name)
                {
                    result.push(symbol);
                }
            }
        }
        let owning_type = self.owner_type_at(file, offset).map(|owner| owner.id);
        if let Some(owner) = owning_type {
            for member in self.members(&TypeRef::named(owner)) {
                if self.is_accessible(member, file, offset) && names.insert(&member.name) {
                    result.push(member);
                }
            }
        }
        for symbol in self.workspace_symbols().filter(|symbol| {
            self.scope(symbol.scope_id).is_some_and(|scope| {
                matches!(
                    scope.kind,
                    ScopeKind::File | ScopeKind::Package | ScopeKind::Module | ScopeKind::Namespace
                )
            }) && (is_type(symbol.kind)
                || matches!(
                    symbol.kind,
                    SymbolKind::Package
                        | SymbolKind::Module
                        | SymbolKind::Namespace
                        | SymbolKind::Function
                        | SymbolKind::Constant
                ))
        }) {
            if self.is_accessible(symbol, file, offset) && names.insert(&symbol.name) {
                result.push(symbol);
            }
        }
        result
    }
}
fn is_type(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::TypeAlias
    )
}

/// The coordinator calls publish only after its matching persistence transaction
/// succeeds. Prepared snapshots are swapped under a short lock; old tables can
/// be released by the indexing worker after the lock is gone.
pub struct SnapshotPublisher {
    current: RwLock<SemanticSnapshot>,
}
impl Default for SnapshotPublisher {
    fn default() -> Self {
        Self {
            current: RwLock::new(SemanticSnapshot::empty()),
        }
    }
}
impl SnapshotPublisher {
    pub fn from_snapshot(snapshot: SemanticSnapshot) -> Self {
        Self {
            current: RwLock::new(snapshot),
        }
    }
    pub fn load(&self) -> Result<SemanticSnapshot> {
        self.current
            .read()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| DomainError::Unsupported("semantic publication lock poisoned".into()))
    }
    pub fn publish(&self, next: SemanticSnapshot) -> Result<SemanticSnapshot> {
        let mut current = self
            .current
            .write()
            .map_err(|_| DomainError::Unsupported("semantic publication lock poisoned".into()))?;
        if next.version() <= current.version() {
            return Err(DomainError::Unsupported(
                "semantic publication revision must advance".into(),
            ));
        }
        let previous = std::mem::replace(&mut *current, next);
        drop(current);
        Ok(previous)
    }
}
