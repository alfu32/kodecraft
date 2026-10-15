//! Worker-side syntax composition and explicit grammar fallback status.
use crate::{
    keywords::KeywordProvider,
    model::{GrammarMode, GrammarStatus, SyntaxSpan},
    parser::{ParsedDocument, ParserSession},
    registry::GrammarRegistry,
};
use kode_core::{DomainError, JobTag, Result, WorkspaceGeneration, document::TextSnapshot};
use kode_platform::{
    jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler},
    paths::{ResourcePaths, compiled_resources},
};
use std::sync::{Arc, Mutex};
pub struct SyntaxReport {
    pub tag: JobTag,
    pub status: GrammarStatus,
    pub parsed: Option<ParsedDocument>,
    pub spans: Vec<SyntaxSpan>,
}
pub struct SyntaxSession {
    language: String,
    keywords: Arc<KeywordProvider>,
    parser: Option<ParserSession>,
    unavailable: Option<String>,
}
impl SyntaxSession {
    pub fn new(
        language: impl Into<String>,
        registry: &GrammarRegistry,
        keywords: Arc<KeywordProvider>,
    ) -> Self {
        let language = language.into();
        let mut unavailable = None;
        let parser = match registry.get(&language) {
            Some(grammar) => match ParserSession::new(grammar) {
                Ok(parser) => Some(parser),
                Err(error) => {
                    unavailable = Some(error.to_string());
                    None
                }
            },
            None => {
                unavailable = Some(format!(
                    "native grammar unavailable for {language}; keyword fallback"
                ));
                None
            }
        };
        Self {
            language,
            keywords,
            parser,
            unavailable,
        }
    }
    /// All text scanning/parsing happens here, never from paint or input handlers.
    pub fn highlight(
        &mut self,
        tag: JobTag,
        snapshot: &TextSnapshot,
        cancel: &CancellationToken,
    ) -> Result<SyntaxReport> {
        if tag.document != Some((snapshot.id, snapshot.revision)) {
            return Err(DomainError::Unsupported(
                "syntax request does not match its document revision".into(),
            ));
        }
        if snapshot.len_bytes() > crate::parser::MAX_SOURCE_BYTES {
            return Err(DomainError::Unsupported(
                "syntax input exceeds 16 MiB".into(),
            ));
        }
        let mut spans = self
            .keywords
            .highlight(snapshot, &self.language, || cancel.is_cancelled())?;
        let mut status = GrammarStatus {
            language: self.language.clone(),
            mode: if self.keywords.supports(&self.language) {
                GrammarMode::Keywords
            } else {
                GrammarMode::Plain
            },
            message: self.unavailable.clone(),
        };
        let mut parsed = None;
        if let Some(parser) = &mut self.parser {
            match parser.parse(snapshot, tag.workspace, cancel) {
                Ok(tree) => {
                    status.mode = GrammarMode::TreeSitter;
                    status.message = tree.has_errors().then(|| {
                        format!(
                            "{} parse issue(s); incomplete source remains usable",
                            tree.diagnostics().len()
                        )
                    });
                    spans.extend_from_slice(tree.lexical());
                    parsed = Some(tree);
                }
                Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
                Err(error) => status.message = Some(format!("{error}; keyword fallback")),
            }
        }
        spans.sort_by_key(|span| (span.range.start, span.range.end));
        Ok(SyntaxReport {
            tag,
            status,
            parsed,
            spans,
        })
    }
}
pub fn load_keywords(paths: &ResourcePaths) -> Result<KeywordProvider> {
    let defaults = compiled_resources();
    let patterns = paths.resolve("keyword-patterns.txt", &defaults)?;
    let colors = paths.resolve("token-colors.txt", &defaults)?;
    let utf8 = |bytes: Vec<u8>| {
        String::from_utf8(bytes).map_err(|error| DomainError::Parse {
            format: "syntax resource UTF-8",
            message: error.to_string(),
        })
    };
    KeywordProvider::from_resources(&utf8(patterns.bytes)?, &utf8(colors.bytes)?)
}
pub fn schedule_highlight(
    scheduler: &mut dyn WorkerScheduler<SyntaxReport>,
    owner: OwnerId,
    tag: JobTag,
    session: Arc<Mutex<SyntaxSession>>,
    snapshot: TextSnapshot,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        Delivery::Cancellable,
        Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            session
                .lock()
                .map_err(|_| DomainError::Unsupported("syntax session lock poisoned".into()))?
                .highlight(tag, &snapshot, &cancel)
        }),
    )
}
pub fn tag_for(snapshot: &TextSnapshot, generation: WorkspaceGeneration) -> Result<JobTag> {
    Ok(JobTag {
        workspace: generation,
        request: kode_core::RequestId::fresh()?,
        document: Some((snapshot.id, snapshot.revision)),
    })
}
