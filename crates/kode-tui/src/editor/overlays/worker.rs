use super::PaintSpan;
use kode_core::{
    DocumentId, DomainError, JobTag, Result, TextRange, document::TextSnapshot,
    intelligence::IntelligenceProvider,
};
use kode_intelligence::facade::IntelligenceResources;
use kode_platform::jobs::CancellationToken;
use kode_syntax::{model::GrammarStatus, service::SyntaxSession};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const MAX_SPANS: usize = 32_768;
#[derive(Debug, Clone)]
pub struct OverlayJob {
    pub tag: JobTag,
    pub snapshot: TextSnapshot,
    pub range: TextRange,
    pub language: String,
}
#[derive(Debug)]
pub struct OverlayReport {
    pub tag: JobTag,
    pub range: TextRange,
    pub lexical: Vec<PaintSpan>,
    /// None keeps remapped semantic paint after an explicit provider failure.
    /// Some(empty) clears it when the provider is ready and has no occurrences.
    pub semantic: Option<Vec<PaintSpan>>,
    pub status: GrammarStatus,
    pub notes: Vec<String>,
}
struct Entry {
    document: DocumentId,
    language: String,
    bytes: usize,
    session: Arc<Mutex<SyntaxSession>>,
}
pub struct OverlayWorker {
    resources: Arc<IntelligenceResources>,
    sessions: Mutex<VecDeque<Entry>>,
}
impl OverlayWorker {
    pub fn new(resources: Arc<IntelligenceResources>) -> Self {
        Self {
            resources,
            sessions: Mutex::default(),
        }
    }
    pub fn prepare(
        &self,
        job: &OverlayJob,
        provider: Option<&dyn IntelligenceProvider>,
        cancel: &CancellationToken,
    ) -> Result<OverlayReport> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        job.snapshot.slice(job.range)?;
        let session = {
            let mut pool = self
                .sessions
                .lock()
                .map_err(|_| DomainError::Unsupported("overlay pool lock poisoned".into()))?;
            let entry = pool
                .iter()
                .position(|entry| {
                    entry.document == job.snapshot.id && entry.language == job.language
                })
                .and_then(|index| pool.remove(index));
            let session = entry.map_or_else(
                || {
                    Arc::new(Mutex::new(SyntaxSession::new(
                        &job.language,
                        &self.resources.grammars,
                        self.resources.keywords.clone(),
                    )))
                },
                |entry| entry.session,
            );
            pool.push_back(Entry {
                document: job.snapshot.id,
                language: job.language.clone(),
                bytes: job.snapshot.len_bytes(),
                session: session.clone(),
            });
            while pool.len() > 32
                || pool.len() > 1 && pool.iter().map(|e| e.bytes).sum::<usize>() > 64 * 1024 * 1024
            {
                pool.pop_front();
            }
            session
        };
        let syntax = {
            let mut session = session
                .lock()
                .map_err(|_| DomainError::Unsupported("overlay parser lock poisoned".into()))?;
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            session.highlight(job.tag, &job.snapshot, cancel)?
        };
        let intersects =
            |range: TextRange| range.start < job.range.end && job.range.start < range.end;
        let mut lexical: Vec<_> = syntax
            .spans
            .into_iter()
            .filter(|s| intersects(s.range))
            .map(|s| PaintSpan {
                range: s.range,
                priority: if s.class.contains("comment") || s.class.contains("string") {
                    14
                } else {
                    10
                },
                class: s.class,
                color: s.color,
            })
            .collect();
        let mut notes = vec![];
        let mut semantic = if let Some(provider) = provider {
            match provider.highlights(&job.snapshot.to_service_snapshot()) {
                Ok(spans) => {
                    let mut accepted = vec![];
                    for span in spans.into_iter().filter(|s| intersects(s.range)) {
                        if job.snapshot.slice(span.range).is_err() || span.range.is_empty() {
                            notes.push("provider returned an invalid highlight range".into());
                            continue;
                        }
                        accepted.push(PaintSpan {
                            range: span.range,
                            class: span.class,
                            color: None,
                            priority: 20,
                        });
                    }
                    Some(accepted)
                }
                Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
                Err(error) => {
                    notes.push(error.to_string());
                    None
                }
            }
        } else {
            Some(vec![])
        };
        if lexical.len() > MAX_SPANS || semantic.as_ref().is_some_and(|s| s.len() > MAX_SPANS) {
            notes.push(format!(
                "visible highlight capture limit ({MAX_SPANS}) reached"
            ));
            lexical.truncate(MAX_SPANS);
            if let Some(semantic) = &mut semantic {
                semantic.truncate(MAX_SPANS);
            }
        }
        notes.truncate(8);
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        Ok(OverlayReport {
            tag: job.tag,
            range: job.range,
            lexical,
            semantic,
            status: syntax.status,
            notes,
        })
    }
}
