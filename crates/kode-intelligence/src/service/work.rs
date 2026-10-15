use super::*;
use crate::{
    frontend::registry::SemanticAdapterSession,
    ids::{FileId, SemanticIds},
    index::IncrementalSemanticIndex,
    model::{FileSemanticDelta, ResolvedSemanticProject},
};
use kode_core::{DocumentOrigin, document::Document};
use kode_platform::files::open_regular_file;
use kode_workspace::{mime::MimeCategory, scanner};
use std::{io::Read, time::Instant};

pub struct PreparedIndex {
    pub epoch: u64,
    pub snapshot: SemanticSnapshot,
    pub affected: BTreeSet<FileId>,
    pub removed: BTreeSet<FileId>,
    pub notes: Vec<String>,
    pub completed: usize,
}
pub struct IndexJob {
    pub tag: JobTag,
    pub(super) epoch: u64,
    pub(super) baseline: SemanticSnapshot,
    pub(super) inputs: Vec<CapturedSource>,
    pub(super) captures: Vec<CapturedSource>,
    pub(super) project: Option<(ScanMode, Project)>,
    pub(super) resources: Arc<IntelligenceResources>,
    pub(super) detector: MimeDetector,
    pub(super) frontend: Arc<Mutex<FrontendCache>>,
    pub(super) shared: Arc<Mutex<Shared>>,
    pub(super) notes: Vec<String>,
    pub(super) removed: BTreeSet<FileId>,
}
impl IndexJob {
    fn progress(&self, phase: IndexPhase, completed: usize, total: Option<usize>) -> Result<()> {
        let mut shared = self.shared.lock().map_err(poisoned)?;
        if shared.epoch != self.epoch {
            return Err(DomainError::Cancelled);
        }
        shared.status.phase = phase;
        shared.status.completed = completed;
        shared.status.total = total;
        Ok(())
    }
    /// Executes exclusively in the caller's F03 bounded blocking scheduler.
    pub fn run(mut self, cancel: CancellationToken) -> Result<PreparedIndex> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let revision = self
            .baseline
            .version()
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("index revision exhausted".into()))?;
        if self
            .project
            .as_ref()
            .is_some_and(|(mode, _)| *mode == ScanMode::Clear)
        {
            self.progress(IndexPhase::Resolving, 0, Some(0))?;
            let removed = self.baseline.project().deltas.keys().copied().collect();
            return Ok(PreparedIndex {
                epoch: self.epoch,
                snapshot: SemanticSnapshot::new(revision, ResolvedSemanticProject::default())?,
                affected: self.baseline.project().deltas.keys().copied().collect(),
                removed,
                notes: self.notes,
                completed: 0,
            });
        }
        let mut captures: BTreeMap<_, _> = self
            .captures
            .iter()
            .chain(&self.inputs)
            .cloned()
            .map(|source| (SemanticIds::file(&source.path), source))
            .collect();
        captures.retain(|id, _| !self.removed.contains(id));
        let mut disk_files = vec![];
        let mut complete_scan = false;
        if let Some((_, project)) = &self.project {
            self.progress(IndexPhase::Scanning, 0, None)?;
            let summary = scanner::scan(project, self.tag, cancel.clone(), |batch| {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                disk_files.extend(batch.files);
                Ok(())
            })?;
            complete_scan = !summary.incomplete;
            self.notes.extend(summary.warnings);
            if summary.incomplete {
                self.notes
                    .push("Source scan incomplete; previous missing-file facts retained".into());
            }
        }
        self.progress(
            IndexPhase::Extracting,
            0,
            Some(disk_files.len() + captures.len()),
        )?;
        let mut updates = BTreeMap::new();
        let mut present: BTreeSet<_> = captures.keys().copied().collect();
        let mut completed = 0;
        let mut frontend = self.frontend.lock().map_err(poisoned)?;
        if self
            .project
            .as_ref()
            .is_some_and(|(mode, _)| *mode == ScanMode::Rebuild)
        {
            *frontend = FrontendCache::default();
        }
        for file in disk_files {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let Some(path) = file.path.to_str() else {
                self.notes.push("Non-UTF-8 source path skipped".into());
                continue;
            };
            let id = SemanticIds::file(path);
            if captures.contains_key(&id) || self.removed.contains(&id) {
                continue;
            }
            let detected = self.detector.detect_file(&file.path, &cancel);
            match detected {
                Ok(detected) if detected.category == MimeCategory::Text => {
                    let language = detected.language.as_deref().unwrap_or("text");
                    let loaded = read_text(&file.path, &cancel).and_then(|text| {
                        let doc = Document::new(
                            DocumentId::fresh()?,
                            DocumentOrigin::File(file.path.clone()),
                            Some(language.into()),
                            &text,
                        );
                        let source = CapturedSource::new(doc.snapshot())?;
                        frontend.extract(
                            &source,
                            &self.resources,
                            self.tag.workspace,
                            &self.baseline,
                            &cancel,
                        )
                    });
                    match loaded {
                        Ok((delta, notes)) => {
                            present.insert(id);
                            updates.insert(id, delta);
                            self.notes.extend(notes);
                        }
                        Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
                        Err(error) => {
                            present.insert(id);
                            self.notes.push(format!(
                                "{}: {error}; previous facts retained",
                                file.path.display()
                            ));
                        }
                    }
                }
                Ok(_) => {}
                Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
                Err(error) => {
                    present.insert(id);
                    self.notes.push(format!(
                        "{}: {error}; previous facts retained",
                        file.path.display()
                    ));
                }
            }
            completed += 1;
            self.progress(IndexPhase::Extracting, completed, None)?;
            self.notes.truncate(32);
        }
        for (id, source) in captures {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let (delta, notes) = frontend.extract(
                &source,
                &self.resources,
                self.tag.workspace,
                &self.baseline,
                &cancel,
            )?;
            updates.insert(id, delta);
            self.notes.extend(notes);
            self.notes.truncate(32);
            completed += 1;
            self.progress(IndexPhase::Extracting, completed, None)?;
        }
        drop(frontend);
        if complete_scan {
            self.removed.extend(
                self.baseline
                    .project()
                    .deltas
                    .keys()
                    .filter(|id| !present.contains(id))
                    .copied(),
            );
        }
        self.progress(IndexPhase::Resolving, completed, Some(completed))?;
        let resolved = IncrementalSemanticIndex::prepare(
            self.baseline.project(),
            updates.into_values(),
            self.removed,
            &cancel,
        )?;
        self.notes.extend(resolved.report.notes);
        self.notes.sort();
        self.notes.dedup();
        cap_notes(&mut self.notes);
        Ok(PreparedIndex {
            epoch: self.epoch,
            snapshot: SemanticSnapshot::new(revision, resolved.report.project)?,
            affected: resolved.invalidation.affected_files,
            removed: resolved.invalidation.removed_files,
            notes: self.notes,
            completed,
        })
    }
}
pub struct IndexCommit {
    pub tag: JobTag,
    pub(super) prepared: PreparedIndex,
    pub(super) client: crate::store::StoreClient,
    pub(super) shared: Arc<Mutex<Shared>>,
    pub(super) stop: CancellationToken,
}
impl IndexCommit {
    /// Verify freshness at actual submission, then drain the accepted SQLite
    /// transaction as critical work even if shutdown or a new edit follows.
    pub async fn run(self) -> Result<(PreparedIndex, CommitReceipt)> {
        let ticket = {
            let guard = self.shared.lock().map_err(poisoned)?;
            if self.stop.is_cancelled() || guard.epoch != self.prepared.epoch {
                return Err(DomainError::Cancelled);
            }
            self.client.commit(
                self.tag,
                self.prepared.snapshot.clone(),
                None,
                CancellationToken::new(),
            )?
        };
        match ticket.resolve().await {
            Ok(reply) => Ok((self.prepared, reply.value)),
            Err(error) => {
                // A competing writer can require a reload before a retry. Read
                // failure remains visible; this never deletes foreign caches.
                if let Ok(ticket) = self.client.load(self.tag, CancellationToken::new()) {
                    let _ = ticket.resolve().await;
                }
                Err(error)
            }
        }
    }
}
struct FrontendEntry {
    language: String,
    adapter: SemanticAdapterSession,
    bytes: usize,
    touched: Instant,
    content_hash: String,
    notes: Vec<String>,
}
#[derive(Default)]
pub(super) struct FrontendCache {
    entries: BTreeMap<String, FrontendEntry>,
    /// Requalify persisted extraction with this startup's resource overrides
    /// once, even when source content and the grammar ABI are unchanged.
    validated: BTreeSet<FileId>,
}
impl FrontendCache {
    fn extract(
        &mut self,
        source: &CapturedSource,
        resources: &IntelligenceResources,
        generation: WorkspaceGeneration,
        previous: &SemanticSnapshot,
        cancel: &CancellationToken,
    ) -> Result<(FileSemanticDelta, Vec<String>)> {
        let text = source.snapshot.text();
        if text.len() > 16 * 1024 * 1024 {
            return Err(DomainError::Unsupported(
                "source exceeds native 16 MiB limit".into(),
            ));
        }
        let language = kode_syntax::registry::canonical_language(
            source.snapshot.language.as_deref().unwrap_or("text"),
        );
        let hash = SemanticIds::content_hash(&text);
        if let Some(old) = previous
            .project()
            .deltas
            .get(&SemanticIds::file(&source.path))
            && old.file.content_hash == hash
            && old.file.language_id == language
            && self.validated.contains(&SemanticIds::file(&source.path))
        {
            let mut delta = old.as_ref().clone();
            delta.file.parse_version = source.snapshot.revision.0;
            delta.file.semantic_version = source.snapshot.revision.0;
            let mut notes = self
                .entries
                .get(&source.path)
                .filter(|entry| entry.content_hash == hash)
                .map_or_else(Vec::new, |entry| entry.notes.clone());
            if resources.grammars.get(&language).is_none() {
                notes.push(format!(
                    "no native {language} grammar; using cached heuristic definitions"
                ));
            }
            return Ok((delta, notes));
        }
        if self
            .entries
            .get(&source.path)
            .is_some_and(|entry| entry.language != language)
        {
            self.entries.remove(&source.path);
        }
        if !self.entries.contains_key(&source.path) {
            while self.entries.len() >= 32
                || self.entries.values().map(|e| e.bytes).sum::<usize>() + text.len()
                    > 64 * 1024 * 1024
            {
                let Some(oldest) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| e.touched)
                    .map(|(path, _)| path.clone())
                else {
                    break;
                };
                self.entries.remove(&oldest);
            }
            self.entries.insert(
                source.path.clone(),
                FrontendEntry {
                    language: language.clone(),
                    adapter: SemanticAdapterSession::new(&resources.grammars, &language),
                    bytes: text.len(),
                    touched: Instant::now(),
                    content_hash: String::new(),
                    notes: vec![],
                },
            );
        }
        let entry = self
            .entries
            .get_mut(&source.path)
            .expect("inserted frontend session");
        entry.bytes = text.len();
        entry.touched = Instant::now();
        let report = entry.adapter.extract(
            &resources.definitions,
            &resources.keywords,
            &source.path,
            &source.snapshot,
            generation,
            cancel,
        )?;
        entry.content_hash = hash;
        entry.notes = report.notes.clone();
        self.validated.insert(SemanticIds::file(&source.path));
        while self.entries.values().map(|e| e.bytes).sum::<usize>() > 64 * 1024 * 1024 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        Ok((report.delta, report.notes))
    }
}
fn read_text(path: &std::path::Path, cancel: &CancellationToken) -> Result<String> {
    let mut file = open_regular_file(path)?;
    let metadata = file.metadata().map_err(|source| DomainError::Io {
        operation: "stat semantic source",
        path: path.into(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DomainError::Io {
            operation: "read semantic source",
            path: path.into(),
            source,
        })?;
    let after = file.metadata().map_err(|source| DomainError::Io {
        operation: "stat semantic source after read",
        path: path.into(),
        source,
    })?;
    if metadata.len() != after.len() || metadata.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(path.into()));
    }
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(DomainError::Unsupported(
            "source exceeds native 16 MiB limit".into(),
        ));
    }
    String::from_utf8(bytes).map_err(|e| DomainError::Parse {
        format: "UTF-8 semantic source",
        message: e.to_string(),
    })
}
