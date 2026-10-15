use super::*;
use crate::{
    ids::ScopeId,
    model::{ScopeKind, SymbolRecord, TypeRef},
    resolver::BestEffortExpressionTypeResolver,
};
use std::collections::{BTreeMap, BTreeSet};

pub struct LocalScopeCompletionProvider;
pub struct MemberCompletionProvider;
pub struct TypeCompletionProvider;
pub struct ImportCompletionProvider;
pub struct KeywordCompletionProvider(pub Arc<KeywordProvider>);
pub struct WorkspaceSymbolCompletionProvider;
pub struct SnippetCompletionProvider;
type Emit<'a> = &'a mut dyn FnMut(CompletionCandidate) -> Result<()>;

fn symbol_candidate(
    symbol: &SymbolRecord,
    context: &CompletionContext,
    snapshot: &SemanticSnapshot,
    provider: ProviderKind,
    distance: u32,
    detail: &str,
) -> CompletionCandidate {
    let mut same_file_count = 0;
    let mut workspace_frequency = 0;
    for occurrence in snapshot.references(symbol.id) {
        workspace_frequency += 1;
        same_file_count += usize::from(occurrence.file_id == context.file_id);
    }
    let qualified = symbol.qualified_name.as_deref().unwrap_or(&symbol.name);
    CompletionCandidate {
        label: symbol.name.clone(),
        insert_text: if context.import_context {
            qualified.into()
        } else {
            symbol.name.clone()
        },
        detail: if context.import_context {
            qualified.into()
        } else {
            detail.into()
        },
        kind: symbol.kind,
        symbol_id: Some(symbol.id),
        scope_distance: Some(distance),
        same_file_count,
        workspace_frequency,
        member: provider == ProviderKind::Member,
        provider,
        score: CompletionScore::default(),
    }
}
fn global(symbol: &SymbolRecord, snapshot: &SemanticSnapshot) -> bool {
    snapshot.scope(symbol.scope_id).is_some_and(|s| {
        matches!(
            s.kind,
            ScopeKind::File | ScopeKind::Package | ScopeKind::Module | ScopeKind::Namespace
        )
    })
}
fn type_kind(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::TypeAlias
    )
}
fn distances(context: &CompletionContext, snapshot: &SemanticSnapshot) -> BTreeMap<ScopeId, u32> {
    let mut current = snapshot
        .scopes(context.file_id)
        .filter(|s| s.range.contains(context.offset) || s.range.end == context.offset)
        .min_by_key(|s| s.range.len())
        .map(|s| s.id);
    let mut result = BTreeMap::new();
    let mut depth = 0;
    while let Some(id) = current {
        if result.insert(id, depth).is_some() {
            break;
        }
        depth += 1;
        current = snapshot.scope(id).and_then(|s| s.parent_id);
    }
    result
}
impl CompletionProvider for LocalScopeCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        !c.member_access && !c.import_context
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        s: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        let distances = distances(c, s);
        for symbol in s.visible_symbols(c.file_id, c.offset) {
            if global(symbol, s)
                && (type_kind(symbol.kind)
                    || matches!(
                        symbol.kind,
                        SymbolKind::Function
                            | SymbolKind::Constant
                            | SymbolKind::Package
                            | SymbolKind::Module
                            | SymbolKind::Namespace
                    ))
            {
                continue;
            }
            if !s.is_accessible(symbol, c.file_id, c.offset) {
                continue;
            }
            if let Some(distance) = distances.get(&symbol.scope_id) {
                emit(symbol_candidate(
                    symbol,
                    c,
                    s,
                    ProviderKind::Local,
                    *distance,
                    "visible symbol",
                ))?;
            } else if matches!(
                symbol.kind,
                SymbolKind::Field | SymbolKind::Property | SymbolKind::Method
            ) {
                emit(symbol_candidate(
                    symbol,
                    c,
                    s,
                    ProviderKind::Local,
                    1,
                    "inherited member",
                ))?;
            }
        }
        Ok(())
    }
}
impl CompletionProvider for MemberCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        c.member_access && c.receiver_range.is_some()
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        s: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        let Some(receiver) = c.receiver_range else {
            return Ok(());
        };
        let ty = BestEffortExpressionTypeResolver::type_of(c.file_id, receiver, s);
        for records in accessible_members(&ty, c, s).into_values() {
            if union(&ty) {
                let mut candidate =
                    symbol_candidate(records[0], c, s, ProviderKind::Member, 0, "member");
                let identities: BTreeSet<_> = records.iter().map(|s| s.id).collect();
                if identities.len() != 1 {
                    candidate.symbol_id = None;
                }
                emit(candidate)?;
            } else {
                for symbol in records {
                    emit(symbol_candidate(
                        symbol,
                        c,
                        s,
                        ProviderKind::Member,
                        0,
                        "member",
                    ))?;
                }
            }
        }
        Ok(())
    }
}
fn union(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Union { .. } => true,
        TypeRef::Generic { base, .. } | TypeRef::Nullable { inner: base } => union(base),
        _ => false,
    }
}
fn accessible_members<'a>(
    ty: &TypeRef,
    c: &CompletionContext,
    s: &'a SemanticSnapshot,
) -> BTreeMap<(String, SymbolKind), Vec<&'a SymbolRecord>> {
    match ty {
        TypeRef::Generic { base, .. } | TypeRef::Nullable { inner: base } => {
            accessible_members(base, c, s)
        }
        TypeRef::Union { alternatives } => {
            let mut alternatives = alternatives.iter();
            let Some(first) = alternatives.next() else {
                return BTreeMap::new();
            };
            let mut result = accessible_members(first, c, s);
            for next in alternatives {
                let mut members = accessible_members(next, c, s);
                result.retain(|key, records| {
                    if let Some(mut shared) = members.remove(key) {
                        records.append(&mut shared);
                        true
                    } else {
                        false
                    }
                });
            }
            result
        }
        _ => {
            let mut result: BTreeMap<_, Vec<_>> = BTreeMap::new();
            for member in s
                .members(ty)
                .into_iter()
                .filter(|symbol| s.is_accessible(symbol, c.file_id, c.offset))
            {
                result
                    .entry((member.name.clone(), member.kind))
                    .or_default()
                    .push(member);
            }
            result
        }
    }
}
impl CompletionProvider for TypeCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        !c.member_access
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        s: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        let visible: BTreeSet<_> = s
            .visible_symbols(c.file_id, c.offset)
            .iter()
            .map(|s| s.id)
            .collect();
        for symbol in s.workspace_symbols().filter(|symbol| {
            type_kind(symbol.kind)
                && (global(symbol, s) || (!c.import_context && visible.contains(&symbol.id)))
                && s.is_accessible(symbol, c.file_id, c.offset)
        }) {
            emit(symbol_candidate(
                symbol,
                c,
                s,
                ProviderKind::Type,
                4,
                symbol.qualified_name.as_deref().unwrap_or("type"),
            ))?;
        }
        Ok(())
    }
}
impl CompletionProvider for ImportCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        c.import_context
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        s: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        for symbol in s.workspace_symbols().filter(|symbol| {
            global(symbol, s)
                && symbol.qualified_name.is_some()
                && s.is_accessible(symbol, c.file_id, c.offset)
        }) {
            emit(symbol_candidate(
                symbol,
                c,
                s,
                ProviderKind::Import,
                8,
                symbol.qualified_name.as_deref().unwrap_or(&symbol.name),
            ))?;
        }
        Ok(())
    }
}
impl CompletionProvider for KeywordCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        !c.member_access && !c.import_context
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        _: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        let language = if c.language_id == "tsx" {
            "typescript"
        } else {
            &c.language_id
        };
        for keyword in self.0.keywords(language) {
            emit(text_candidate(keyword, "keyword", ProviderKind::Keyword))?;
        }
        Ok(())
    }
}
impl CompletionProvider for WorkspaceSymbolCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        !c.member_access && !c.import_context
    }
    fn candidates(
        &self,
        c: &CompletionContext,
        s: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        for symbol in s
            .workspace_symbols()
            .filter(|symbol| global(symbol, s) && s.is_accessible(symbol, c.file_id, c.offset))
        {
            emit(symbol_candidate(
                symbol,
                c,
                s,
                ProviderKind::Workspace,
                10,
                "workspace symbol",
            ))?;
        }
        Ok(())
    }
}
impl CompletionProvider for SnippetCompletionProvider {
    fn supports(&self, c: &CompletionContext) -> bool {
        !c.member_access && !c.import_context && c.language_id == "kotlin"
    }
    fn candidates(
        &self,
        _: &CompletionContext,
        _: &SemanticSnapshot,
        emit: Emit<'_>,
    ) -> Result<()> {
        // The source offers declaration starters, without template expansion.
        emit(text_candidate(
            "fun",
            "function declaration",
            ProviderKind::Snippet,
        ))?;
        emit(text_candidate(
            "when",
            "when expression",
            ProviderKind::Snippet,
        ))
    }
}
fn text_candidate(label: &str, detail: &str, provider: ProviderKind) -> CompletionCandidate {
    CompletionCandidate {
        label: label.into(),
        insert_text: label.into(),
        detail: detail.into(),
        kind: SymbolKind::Unknown,
        symbol_id: None,
        scope_distance: None,
        same_file_count: 0,
        workspace_frequency: 0,
        member: false,
        provider,
        score: CompletionScore::default(),
    }
}
