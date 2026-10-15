//! Kotlin extraction against the pinned ast-grep grammar. No cross-file resolution.
mod builder;
use crate::model::FileSemanticDelta;
use kode_core::{Result, WorkspaceGeneration, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{
    parser::{ParsedDocument, ParserSession},
    registry::GrammarRegistry,
};

pub struct KotlinSemanticAdapter {
    parser: ParserSession,
}
impl KotlinSemanticAdapter {
    pub fn new(registry: &GrammarRegistry) -> Result<Self> {
        let grammar = registry.get("kotlin").ok_or_else(|| {
            kode_core::DomainError::Unsupported("native Kotlin grammar unavailable".into())
        })?;
        Ok(Self {
            parser: ParserSession::new(grammar)?,
        })
    }
    /// Own one adapter per document worker. Immutable published deltas retain no
    /// mutable parser state; L09 owns cache eviction and stale-result rejection.
    pub fn extract(
        &mut self,
        path: &str,
        snapshot: &TextSnapshot,
        generation: WorkspaceGeneration,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        let parsed = self.parser.parse(snapshot, generation, cancel)?;
        Self::extract_parsed(path, &parsed, cancel)
    }
    pub fn extract_parsed(
        path: &str,
        parsed: &ParsedDocument,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        if parsed.grammar().id != "kotlin" {
            return Err(kode_core::DomainError::Unsupported(
                "Kotlin extraction requires the selected Kotlin grammar".into(),
            ));
        }
        builder::Builder::new(path, parsed, cancel).build()
    }
}
