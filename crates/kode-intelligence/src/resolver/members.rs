use super::*;
impl State<'_> {
    /// All union alternatives must expose the member. Distinct declarations
    /// retain distinct identities even when their visible spelling is shared.
    pub fn value_members(
        &self,
        ty: &TypeRef,
        name: &str,
        file: FileId,
        scope: ScopeId,
    ) -> Vec<SymbolId> {
        match ty {
            TypeRef::Named { symbol_id } => self
                .named_members(*symbol_id, name)
                .into_iter()
                .filter(|id| {
                    self.symbol(*id)
                        .is_some_and(|s| s.name == name && self.accessible(s, file, scope))
                })
                .collect(),
            TypeRef::Nullable { inner } | TypeRef::Generic { base: inner, .. } => {
                self.value_members(inner, name, file, scope)
            }
            TypeRef::Union { alternatives } => {
                let mut combined = BTreeSet::new();
                let mut kinds: Option<BTreeSet<SymbolKind>> = None;
                for ty in alternatives {
                    let members = self.value_members(ty, name, file, scope);
                    if members.is_empty() {
                        return vec![];
                    }
                    let current: BTreeSet<_> = members
                        .iter()
                        .filter_map(|id| self.symbol(*id).map(|s| s.kind))
                        .collect();
                    kinds = Some(kinds.map_or(current.clone(), |previous| {
                        previous.intersection(&current).copied().collect()
                    }));
                    combined.extend(members);
                }
                combined
                    .into_iter()
                    .filter(|id| {
                        self.symbol(*id)
                            .is_some_and(|s| kinds.as_ref().is_some_and(|k| k.contains(&s.kind)))
                    })
                    .collect()
            }
            _ => vec![],
        }
    }
}
