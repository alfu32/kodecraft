use super::*;
impl State<'_> {
    pub fn unique(&self, candidates: impl IntoIterator<Item = SymbolId>) -> Option<SymbolId> {
        let candidates: BTreeSet<_> = candidates.into_iter().collect();
        if candidates.len() == 1 {
            candidates.into_iter().next()
        } else {
            None
        }
    }
    fn matches_kind(&self, symbol: &SymbolRecord, kind: NameKind, file: FileId) -> bool {
        match kind {
            NameKind::Tag(kind) => symbol.kind == kind,
            NameKind::Type => {
                is_type(symbol.kind)
                    && (self.language(file) != "c" || symbol.kind == SymbolKind::TypeAlias)
            }
            NameKind::Callable => {
                matches!(
                    symbol.kind,
                    SymbolKind::Function
                        | SymbolKind::Method
                        | SymbolKind::Constructor
                        | SymbolKind::Variable
                        | SymbolKind::Parameter
                        | SymbolKind::Property
                        | SymbolKind::Field
                ) || (self.language(file) != "c" && is_type(symbol.kind))
            }
            NameKind::Any => {
                self.language(file) != "c"
                    || !matches!(symbol.kind, SymbolKind::Struct | SymbolKind::Enum)
            }
        }
    }
    pub fn find_name(
        &self,
        name: &str,
        scope: ScopeId,
        file: FileId,
        offset: ByteOffset,
        kind: NameKind,
        exclude: Option<SymbolId>,
    ) -> Option<SymbolId> {
        let admissible = |id: SymbolId| {
            self.symbol(id).is_some_and(|s| {
                Some(id) != exclude
                    && self.language(s.file_id) == self.language(file)
                    && self.matches_kind(s, kind, file)
                    && self.accessible(s, file, scope)
            })
        };
        if name.contains('.') {
            return self.unique(
                self.qualified
                    .get(name)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|id| admissible(*id)),
            );
        }
        // Only the actual lexical chain participates. A sibling's local variable
        // never becomes an accidental workspace fallback.
        for level in self.scope_chain(scope) {
            let bindings: Vec<_> = self
                .scoped
                .get(&level)
                .and_then(|names| names.get(name))
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    self.symbol(*id).is_some_and(|s| {
                        s.name == name
                            && Some(s.id) != exclude
                            && self.accessible(s, file, scope)
                            && match kind {
                                NameKind::Tag(_) => {
                                    matches!(s.kind, SymbolKind::Struct | SymbolKind::Enum)
                                }
                                NameKind::Type if self.language(file) != "c" => is_type(s.kind),
                                _ => {
                                    self.language(file) != "c"
                                        || !matches!(s.kind, SymbolKind::Struct | SymbolKind::Enum)
                                }
                            }
                            && (!matches!(s.kind, SymbolKind::Variable | SymbolKind::Parameter)
                                || s.name_range.start <= offset)
                    })
                })
                .collect();
            if !bindings.is_empty() {
                let mut candidates: Vec<_> =
                    bindings.into_iter().filter(|id| admissible(*id)).collect();
                if candidates.is_empty() {
                    return None;
                }
                // Later declarations of a local name shadow earlier declarations
                // in the same scope, while distinct nonlocal identities conflict.
                if candidates.iter().all(|id| {
                    self.symbol(*id).is_some_and(|s| {
                        matches!(s.kind, SymbolKind::Variable | SymbolKind::Parameter)
                    })
                }) {
                    candidates.sort_by_key(|id| self.symbol(*id).map(|s| s.name_range.start));
                    return candidates.last().copied();
                }
                return self.unique(candidates);
            }
            if let Some(owner) = self
                .scope(level)
                .filter(|s| s.kind == ScopeKind::Type)
                .and_then(|s| s.owner_symbol_id)
            {
                let candidates: Vec<_> = self
                    .named_members(owner, name)
                    .into_iter()
                    .filter(|id| {
                        admissible(*id) && self.symbol(*id).is_some_and(|s| s.name == name)
                    })
                    .collect();
                if !candidates.is_empty() {
                    return self.unique(candidates);
                }
            }
        }
        let explicit: Vec<_> = self
            .import_candidates(name, file, false)
            .into_iter()
            .filter(|id| admissible(*id))
            .collect();
        if !explicit.is_empty() {
            return self.unique(explicit);
        }
        let candidates: Vec<_> = self
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| admissible(*id))
            .filter(|id| {
                self.symbol(*id).is_some_and(|s| {
                    self.is_global(s)
                        && self.language(s.file_id) == self.language(file)
                        && (s.file_id != file
                            || !matches!(s.kind, SymbolKind::Variable | SymbolKind::Parameter)
                            || s.name_range.start <= offset)
                })
            })
            .collect();
        if self.language(file) == "c" {
            let includes = self.included_files(file);
            let included: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|id| {
                    self.symbol(*id)
                        .is_some_and(|s| includes.contains(&s.file_id))
                })
                .collect();
            if !included.is_empty() {
                return self.unique(included);
            }
            return self.unique(candidates);
        }
        let package = self.package(file);
        let same_package: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|id| {
                self.symbol(*id)
                    .is_some_and(|s| self.package(s.file_id) == package)
            })
            .collect();
        if !same_package.is_empty() {
            return self.unique(same_package);
        }
        self.unique(
            self.import_candidates(name, file, true)
                .into_iter()
                .filter(|id| admissible(*id)),
        )
    }
    pub fn occurrence_kind(&self, occurrence: &OccurrenceRecord) -> NameKind {
        if occurrence.kind == OccurrenceKind::TypeReference
            && self.language(occurrence.file_id) == "c"
            && let Some(kind) = self
                .c_type_contexts
                .get(&occurrence.file_id)
                .and_then(|contexts| contexts.lookup(occurrence.range))
        {
            return kind;
        }
        match occurrence.kind {
            OccurrenceKind::TypeReference => NameKind::Type,
            OccurrenceKind::Call => NameKind::Callable,
            _ => NameKind::Any,
        }
    }
}
/// Segment maximum-end index preserves nested/crossing type-specifier ranges
/// without rescanning every earlier field in a large C declaration.
pub(super) struct CTypeContexts {
    entries: Vec<(SourceRange, NameKind)>,
    maximum: Vec<ByteOffset>,
}
impl CTypeContexts {
    pub(super) fn new(delta: &FileSemanticDelta) -> Self {
        let mut entries: Vec<_> = delta
            .unresolved_types
            .iter()
            .map(|ty| (ty.range, tag_kind(&ty.name)))
            .collect();
        for expression in &delta.syntax_expressions {
            if let SyntaxExpression::Cast { value, type_name } = &expression.expression
                && expression.range.start < value.start
            {
                entries.push((
                    SourceRange::new(expression.range.start, value.start)
                        .expect("cast type precedes value"),
                    tag_kind(type_name),
                ));
            }
        }
        entries.sort_by_key(|(range, _)| (range.start, range.end));
        let mut result = Self {
            maximum: vec![ByteOffset(0); entries.len() * 4 + 1],
            entries,
        };
        if !result.entries.is_empty() {
            result.build(1, 0, result.entries.len());
        }
        result
    }
    fn build(&mut self, index: usize, left: usize, right: usize) -> ByteOffset {
        let end = if right - left == 1 {
            self.entries[left].0.end
        } else {
            let middle = (left + right) / 2;
            self.build(index * 2, left, middle)
                .max(self.build(index * 2 + 1, middle, right))
        };
        self.maximum[index] = end;
        end
    }
    pub(super) fn lookup(&self, range: SourceRange) -> Option<NameKind> {
        if self.entries.is_empty() {
            return None;
        }
        let mut best = None;
        self.query(1, 0, self.entries.len(), range, &mut best);
        best.map(|(_, kind)| kind)
    }
    fn query(
        &self,
        index: usize,
        left: usize,
        right: usize,
        range: SourceRange,
        best: &mut Option<(usize, NameKind)>,
    ) {
        if self.maximum[index] < range.end || self.entries[left].0.start > range.start {
            return;
        }
        if right - left == 1 {
            let (candidate, kind) = self.entries[left];
            if candidate.start <= range.start
                && range.end <= candidate.end
                && best
                    .as_ref()
                    .is_none_or(|(length, _)| candidate.len() < *length)
            {
                *best = Some((candidate.len(), kind));
            }
            return;
        }
        let middle = (left + right) / 2;
        self.query(index * 2, left, middle, range, best);
        self.query(index * 2 + 1, middle, right, range, best);
    }
}
fn tag_kind(name: &str) -> NameKind {
    match name
        .split_whitespace()
        .find(|word| !matches!(*word, "const" | "volatile" | "restrict" | "_Atomic"))
    {
        Some("struct" | "union") => NameKind::Tag(SymbolKind::Struct),
        Some("enum") => NameKind::Tag(SymbolKind::Enum),
        _ => NameKind::Type,
    }
}
