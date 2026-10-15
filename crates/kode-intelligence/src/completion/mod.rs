//! Pure completion over prepared snapshots. UI insertion needs no AST access.
mod context;
pub mod providers;
mod ranking;
use crate::{ids::SymbolId, model::SymbolKind, snapshot::SemanticSnapshot};
pub use context::CompletionContext;
use kode_core::{
    DomainError, Result,
    intelligence::{CompletionItem, SymbolKind as EditorKind},
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::keywords::KeywordProvider;
pub use ranking::{CompletionRanker, CompletionScore, HeuristicCompletionRanker};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderKind {
    Local,
    Member,
    Type,
    Import,
    Keyword,
    Workspace,
    Snippet,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionCandidate {
    pub label: String,
    pub insert_text: String,
    pub detail: String,
    pub kind: SymbolKind,
    pub symbol_id: Option<SymbolId>,
    pub scope_distance: Option<u32>,
    pub same_file_count: usize,
    pub workspace_frequency: usize,
    pub member: bool,
    pub provider: ProviderKind,
    pub score: CompletionScore,
}
impl CompletionCandidate {
    pub fn item(&self, context: &CompletionContext) -> CompletionItem {
        CompletionItem {
            label: self.label.clone(),
            insert_text: self.insert_text.clone(),
            detail: Some(self.detail.clone()),
            kind: Some(
                if self.provider == ProviderKind::Keyword || self.provider == ProviderKind::Snippet
                {
                    EditorKind::Keyword
                } else {
                    editor_kind(self.kind)
                },
            ),
            replacement_range: context.replacement_range,
            source: format!("native:{:?}", self.provider).to_lowercase(),
        }
    }
}
pub trait CompletionProvider: Send + Sync {
    fn supports(&self, context: &CompletionContext) -> bool;
    fn candidates(
        &self,
        context: &CompletionContext,
        snapshot: &SemanticSnapshot,
        emit: &mut dyn FnMut(CompletionCandidate) -> Result<()>,
    ) -> Result<()>;
}
pub struct CompletionEngine {
    providers: Vec<Box<dyn CompletionProvider>>,
    ranker: Box<dyn CompletionRanker>,
}
impl CompletionEngine {
    pub fn native(keywords: Arc<KeywordProvider>) -> Self {
        use providers::*;
        Self::new(
            vec![
                Box::new(LocalScopeCompletionProvider),
                Box::new(MemberCompletionProvider),
                Box::new(TypeCompletionProvider),
                Box::new(ImportCompletionProvider),
                Box::new(KeywordCompletionProvider(keywords)),
                Box::new(WorkspaceSymbolCompletionProvider),
                Box::new(SnippetCompletionProvider),
            ],
            Box::new(HeuristicCompletionRanker),
        )
    }
    pub fn new(
        providers: Vec<Box<dyn CompletionProvider>>,
        ranker: Box<dyn CompletionRanker>,
    ) -> Self {
        Self { providers, ranker }
    }
    pub fn complete(
        &self,
        context: &CompletionContext,
        snapshot: &SemanticSnapshot,
        limit: usize,
        cancel: &CancellationToken,
    ) -> Result<Vec<CompletionCandidate>> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if !snapshot
            .file(context.file_id)
            .is_some_and(|f| f.content_hash == context.content_hash)
        {
            return Err(DomainError::Unsupported(
                "completion snapshot does not match source content".into(),
            ));
        }
        if limit == 0 {
            return Ok(vec![]);
        }
        let started = Instant::now();
        let mut count = 0usize;
        let mut candidates = BTreeMap::new();
        let prefix = context.prefix.to_lowercase();
        let qualified = context.qualified_prefix.as_ref().map(|s| s.to_lowercase());
        for provider in &self.providers {
            if !provider.supports(context) {
                continue;
            }
            provider.candidates(context, snapshot, &mut |mut candidate| {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                count += 1;
                if count > 100_000 || started.elapsed() > Duration::from_secs(2) {
                    return Err(DomainError::Unsupported(
                        "completion work limit exceeded".into(),
                    ));
                }
                let matches = if context.import_context
                    && let Some(prefix) = &qualified
                {
                    candidate.insert_text.to_lowercase().starts_with(prefix)
                } else {
                    candidate.label.to_lowercase().starts_with(&prefix)
                };
                if candidate.label.is_empty() || !matches {
                    return Ok(());
                }
                candidate.score = self.ranker.score(context, &candidate);
                if !candidate.score.valid() {
                    return Err(DomainError::Unsupported(
                        "completion ranker returned an invalid score".into(),
                    ));
                }
                let key = candidate.symbol_id.map_or_else(
                    || CandidateKey::Text(candidate.kind, candidate.label.clone()),
                    CandidateKey::Symbol,
                );
                let replace = candidates
                    .get(&key)
                    .is_none_or(|old| compare(&candidate, old).is_lt());
                if replace {
                    candidates.insert(key, candidate);
                }
                Ok(())
            })?;
        }
        let mut values: Vec<_> = candidates.into_values().collect();
        values.sort_by(compare);
        values.truncate(limit.min(1000));
        Ok(values)
    }
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum CandidateKey {
    Symbol(SymbolId),
    Text(SymbolKind, String),
}
fn compare(a: &CompletionCandidate, b: &CompletionCandidate) -> std::cmp::Ordering {
    b.score
        .total()
        .total_cmp(&a.score.total())
        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        .then_with(|| a.kind.wire_name().cmp(b.kind.wire_name()))
        .then_with(|| a.symbol_id.cmp(&b.symbol_id))
        .then_with(|| a.provider.cmp(&b.provider))
        .then_with(|| a.label.cmp(&b.label))
        .then_with(|| a.detail.cmp(&b.detail))
        .then_with(|| a.insert_text.cmp(&b.insert_text))
}
fn editor_kind(kind: SymbolKind) -> EditorKind {
    match kind {
        SymbolKind::Class | SymbolKind::Struct => EditorKind::Class,
        SymbolKind::Interface => EditorKind::Interface,
        SymbolKind::Function | SymbolKind::Constructor => EditorKind::Function,
        SymbolKind::Method => EditorKind::Method,
        SymbolKind::Field | SymbolKind::Property | SymbolKind::EnumMember => EditorKind::Field,
        SymbolKind::Enum => EditorKind::Enum,
        SymbolKind::Module | SymbolKind::Namespace => EditorKind::Module,
        SymbolKind::Package => EditorKind::Package,
        SymbolKind::Parameter => EditorKind::Parameter,
        SymbolKind::TypeAlias => EditorKind::TypeAlias,
        _ => EditorKind::Variable,
    }
}
