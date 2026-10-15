//! Main-thread coordinator; expensive work is returned for F03 bounded workers.
mod work;
use crate::{
    facade::{CapturedSource, HotSources, IntelligenceResources, NativeFacade},
    ids::FileId,
    snapshot::SemanticSnapshot,
    store::{CommitReceipt, StoreLocation, StoreWorker},
};
use kode_core::{
    DocumentId, DomainError, JobTag, RequestId, Result, Revision, WorkspaceGeneration,
    document::TextSnapshot,
};
use kode_platform::jobs::CancellationToken;
use kode_workspace::{mime::MimeDetector, project::Project};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, RwLock},
};
pub use work::{IndexCommit, IndexJob, PreparedIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPhase {
    Ready,
    Queued,
    Scanning,
    Extracting,
    Resolving,
    Persisting,
    Error,
    Stopped,
    Cancelled,
}
#[derive(Debug, Clone)]
pub struct IndexStatus {
    pub phase: IndexPhase,
    pub revision: u64,
    pub files: usize,
    pub symbols: usize,
    pub completed: usize,
    pub total: Option<usize>,
    pub notes: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct IndexChange {
    pub workspace: WorkspaceGeneration,
    pub revision: u64,
    pub affected_files: BTreeSet<FileId>,
    pub removed_files: BTreeSet<FileId>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryRevision {
    pub epoch: u64,
    pub index_revision: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScanMode {
    Rescan,
    Rebuild,
    Clear,
}
struct Shared {
    epoch: u64,
    status: IndexStatus,
}
struct Active {
    tag: JobTag,
    epoch: u64,
    mode: Option<(u64, ScanMode)>,
    inputs: Vec<(FileId, DocumentId, Revision)>,
    removed: BTreeSet<FileId>,
}
pub struct CodeIntelService {
    generation: WorkspaceGeneration,
    worker: Option<StoreWorker>,
    resources: Arc<IntelligenceResources>,
    detector: MimeDetector,
    frontend: Arc<Mutex<work::FrontendCache>>,
    shared: Arc<Mutex<Shared>>,
    hot: Arc<RwLock<HotSources>>,
    stop: CancellationToken,
    facade: Arc<NativeFacade>,
    pending: BTreeMap<FileId, CapturedSource>,
    removed: BTreeSet<FileId>,
    observed: BTreeMap<FileId, (DocumentId, Revision)>,
    active: Option<Active>,
    mode: Option<(u64, ScanMode, Project)>,
    mode_sequence: u64,
    deadline: u64,
    fixed_notes: Vec<String>,
    changes: VecDeque<IndexChange>,
}
impl CodeIntelService {
    pub async fn open(
        project: &Project,
        resources: Arc<IntelligenceResources>,
        detector: MimeDetector,
    ) -> Result<Self> {
        let (worker, mut notes) =
            match StoreWorker::start(StoreLocation::Project(project.root.clone())).await {
                Ok(worker) => (worker, vec![]),
                Err(error) => (
                    StoreWorker::start(StoreLocation::Memory).await?,
                    vec![format!("Native cache unavailable: {error}; using memory")],
                ),
            };
        notes.extend(resources.definitions.diagnostics.iter().cloned());
        notes.extend(
            resources
                .keywords
                .diagnostics
                .iter()
                .map(|d| format!("{}:{}: {}", d.source, d.line, d.message)),
        );
        cap_notes(&mut notes);
        let snapshot = worker.client().snapshot()?;
        let hot = Arc::new(RwLock::new(HotSources::default()));
        let stop = CancellationToken::new();
        let facade = Arc::new(NativeFacade::new(
            worker.client(),
            resources.clone(),
            project.generation,
            hot.clone(),
            stop.clone(),
        ));
        let status = IndexStatus {
            phase: IndexPhase::Ready,
            revision: snapshot.version(),
            files: snapshot.project().deltas.len(),
            symbols: snapshot.project().symbols.len(),
            completed: 0,
            total: None,
            notes: notes.clone(),
        };
        Ok(Self {
            generation: project.generation,
            worker: Some(worker),
            resources,
            detector,
            frontend: Arc::new(Mutex::new(work::FrontendCache::default())),
            shared: Arc::new(Mutex::new(Shared { epoch: 0, status })),
            hot,
            stop,
            facade,
            pending: BTreeMap::new(),
            removed: BTreeSet::new(),
            observed: BTreeMap::new(),
            active: None,
            mode: None,
            mode_sequence: 0,
            deadline: 0,
            fixed_notes: notes,
            changes: VecDeque::new(),
        })
    }
    pub fn facade(&self) -> Arc<NativeFacade> {
        self.facade.clone()
    }
    pub fn snapshot(&self) -> Result<SemanticSnapshot> {
        self.facade.snapshot()
    }
    pub fn status(&self) -> Result<IndexStatus> {
        Ok(self.shared.lock().map_err(poisoned)?.status.clone())
    }
    pub fn query_revision(&self) -> Result<QueryRevision> {
        let epoch = self.shared.lock().map_err(poisoned)?.epoch;
        Ok(QueryRevision {
            epoch,
            index_revision: self.snapshot()?.version(),
        })
    }
    fn advance(&self) -> Result<u64> {
        if self.stop.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let mut state = self.shared.lock().map_err(poisoned)?;
        state.epoch = state
            .epoch
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("semantic epoch exhausted".into()))?;
        state.status.phase = IndexPhase::Queued;
        Ok(state.epoch)
    }
    /// O(1) Rope capture; flattening/hash/parsing stay on the worker.
    pub fn observe(&mut self, snapshot: TextSnapshot, now: u64) -> Result<bool> {
        let source = CapturedSource::new(snapshot)?;
        let file = crate::ids::SemanticIds::file(&source.path);
        let stamp = (source.snapshot.id, source.snapshot.revision);
        if self.observed.get(&file) == Some(&stamp) {
            return Ok(false);
        }
        if self
            .observed
            .get(&file)
            .is_some_and(|(id, revision)| *id == stamp.0 && *revision > stamp.1)
        {
            return Err(DomainError::StaleRevision {
                expected: stamp.1,
                current: self.observed[&file].1,
            });
        }
        if self.pending.len() >= 128 && !self.pending.contains_key(&file) {
            return Err(DomainError::QueueFull);
        }
        self.advance()?;
        self.hot.write().map_err(poisoned)?.insert(source.clone());
        self.removed.remove(&file);
        self.observed.insert(file, stamp);
        self.pending.insert(file, source);
        self.deadline = now.saturating_add(200);
        Ok(true)
    }
    pub fn rescan(&mut self, project: Project, rebuild: bool) -> Result<()> {
        if project.generation != self.generation {
            return Err(DomainError::Cancelled);
        }
        self.advance()?;
        self.mode_sequence = self
            .mode_sequence
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("rescan identity exhausted".into()))?;
        self.mode = Some((
            self.mode_sequence,
            if rebuild {
                ScanMode::Rebuild
            } else {
                ScanMode::Rescan
            },
            project,
        ));
        self.deadline = 0;
        Ok(())
    }
    pub fn clear(&mut self, project: Project) -> Result<()> {
        if project.generation != self.generation {
            return Err(DomainError::Cancelled);
        }
        self.advance()?;
        self.mode_sequence = self
            .mode_sequence
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("rescan identity exhausted".into()))?;
        self.mode = Some((self.mode_sequence, ScanMode::Clear, project));
        self.pending.clear();
        self.deadline = 0;
        Ok(())
    }
    pub fn has_pending_work(&self) -> bool {
        self.active.is_some()
            || self.mode.is_some()
            || !self.pending.is_empty()
            || !self.removed.is_empty()
    }
    /// Cancel a workspace rescan while retaining independently queued live edits.
    /// The caller cancels the returned F03 request and waits for its completion.
    pub fn cancel_rescan(&mut self) -> Result<Option<JobTag>> {
        if !self
            .mode
            .as_ref()
            .is_some_and(|(_, mode, _)| *mode == ScanMode::Rescan)
        {
            return Ok(None);
        }
        self.advance()?;
        self.mode = None;
        if self.active.is_none() && self.pending.is_empty() && self.removed.is_empty() {
            self.shared.lock().map_err(poisoned)?.status.phase = IndexPhase::Cancelled;
        }
        Ok(self.active.as_ref().map(|active| active.tag))
    }
    pub fn remove_path(&mut self, path: &str) -> Result<()> {
        self.advance()?;
        let id = crate::ids::SemanticIds::file(path);
        self.hot.write().map_err(poisoned)?.remove(id);
        self.pending.remove(&id);
        self.removed.insert(id);
        self.deadline = 0;
        Ok(())
    }
    pub fn next_job(&mut self, now: u64) -> Result<Option<IndexJob>> {
        if self.stop.is_cancelled()
            || self.active.is_some()
            || (self.pending.is_empty() && self.mode.is_none() && self.removed.is_empty())
            || now < self.deadline
        {
            return Ok(None);
        }
        let tag = JobTag {
            workspace: self.generation,
            request: RequestId::fresh()?,
            document: None,
        };
        let epoch = self.shared.lock().map_err(poisoned)?.epoch;
        let captures = self.hot.read().map_err(poisoned)?.captures();
        let inputs: Vec<_> = self
            .pending
            .iter()
            .map(|(file, source)| (*file, source.snapshot.id, source.snapshot.revision))
            .collect();
        let mode = self.mode.as_ref().map(|(id, mode, _)| (*id, *mode));
        let job = IndexJob {
            tag,
            epoch,
            baseline: self.snapshot()?,
            inputs: self.pending.values().cloned().collect(),
            captures,
            project: self
                .mode
                .as_ref()
                .map(|(_, mode, project)| (*mode, project.clone())),
            resources: self.resources.clone(),
            detector: self.detector.clone(),
            frontend: self.frontend.clone(),
            shared: self.shared.clone(),
            notes: self.fixed_notes.clone(),
            removed: self.removed.clone(),
        };
        self.active = Some(Active {
            tag,
            epoch,
            mode,
            inputs,
            removed: self.removed.clone(),
        });
        Ok(Some(job))
    }
    pub fn prepared(
        &mut self,
        tag: JobTag,
        result: Result<PreparedIndex>,
        now: u64,
    ) -> Result<Option<IndexCommit>> {
        if !self.active.as_ref().is_some_and(|active| active.tag == tag) {
            return Ok(None);
        }
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                self.failed(tag, error, now)?;
                return Ok(None);
            }
        };
        let epoch = self.shared.lock().map_err(poisoned)?.epoch;
        if epoch != prepared.epoch || self.stop.is_cancelled() {
            self.active = None;
            self.shared.lock().map_err(poisoned)?.status.phase = if self.stop.is_cancelled() {
                IndexPhase::Stopped
            } else if self.pending.is_empty() && self.mode.is_none() && self.removed.is_empty() {
                IndexPhase::Cancelled
            } else {
                IndexPhase::Queued
            };
            return Ok(None);
        }
        self.shared.lock().map_err(poisoned)?.status.phase = IndexPhase::Persisting;
        let client = self.worker.as_ref().ok_or(DomainError::Cancelled)?.client();
        Ok(Some(IndexCommit {
            tag,
            prepared,
            client,
            shared: self.shared.clone(),
            stop: self.stop.clone(),
        }))
    }
    pub fn committed(
        &mut self,
        tag: JobTag,
        result: Result<(PreparedIndex, CommitReceipt)>,
        now: u64,
    ) -> Result<bool> {
        if !self.active.as_ref().is_some_and(|active| active.tag == tag) {
            return Ok(false);
        }
        let (prepared, receipt) = match result {
            Ok(result) => result,
            Err(error) => {
                self.failed(tag, error, now)?;
                return Ok(false);
            }
        };
        let active = self.active.take().expect("checked active index job");
        let current_epoch = self.shared.lock().map_err(poisoned)?.epoch;
        if current_epoch == active.epoch {
            for file in active.removed {
                self.removed.remove(&file);
            }
            for (file, document, revision) in active.inputs {
                if self
                    .pending
                    .get(&file)
                    .is_some_and(|s| (s.snapshot.id, s.snapshot.revision) == (document, revision))
                {
                    self.pending.remove(&file);
                }
            }
            if active.mode.is_some()
                && self.mode.as_ref().map(|(id, mode, _)| (*id, *mode)) == active.mode
            {
                self.mode = None;
            }
        }
        let snapshot = self
            .worker
            .as_ref()
            .ok_or(DomainError::Cancelled)?
            .client()
            .snapshot()?;
        let queued = !self.pending.is_empty() || self.mode.is_some() || !self.removed.is_empty();
        self.shared.lock().map_err(poisoned)?.status = IndexStatus {
            phase: if queued {
                IndexPhase::Queued
            } else {
                IndexPhase::Ready
            },
            revision: receipt.revision,
            files: snapshot.project().deltas.len(),
            symbols: snapshot.project().symbols.len(),
            completed: prepared.completed,
            total: Some(prepared.completed),
            notes: prepared.notes,
        };
        let change = IndexChange {
            workspace: self.generation,
            revision: receipt.revision,
            affected_files: prepared.affected,
            removed_files: prepared.removed,
        };
        if self.changes.len() >= 32 {
            if let Some(last) = self.changes.back_mut() {
                last.revision = change.revision;
                last.affected_files.extend(change.affected_files);
                last.removed_files.extend(change.removed_files);
            }
        } else {
            self.changes.push_back(change);
        }
        Ok(true)
    }
    pub fn failed(&mut self, tag: JobTag, error: DomainError, now: u64) -> Result<()> {
        if !self.active.as_ref().is_some_and(|active| active.tag == tag) {
            return Ok(());
        }
        self.active = None;
        let mut state = self.shared.lock().map_err(poisoned)?;
        state.status.phase = if matches!(error, DomainError::Cancelled) {
            if self.pending.is_empty() && self.mode.is_none() && self.removed.is_empty() {
                IndexPhase::Cancelled
            } else {
                IndexPhase::Queued
            }
        } else {
            IndexPhase::Error
        };
        state.status.notes = self.fixed_notes.clone();
        if !matches!(error, DomainError::Cancelled) {
            state.status.notes.push(error.to_string());
        }
        cap_notes(&mut state.status.notes);
        self.deadline = if matches!(error, DomainError::Cancelled) {
            self.deadline.max(now)
        } else {
            now.saturating_add(1000)
        };
        Ok(())
    }
    pub fn take_changes(&mut self) -> Vec<IndexChange> {
        self.changes.drain(..).collect()
    }
    pub fn stop_queries(&self) {
        self.stop.cancel();
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker.shutdown().await?;
        }
        self.shared.lock().map_err(poisoned)?.status.phase = IndexPhase::Stopped;
        Ok(())
    }
}
fn poisoned<T>(_: std::sync::PoisonError<T>) -> DomainError {
    DomainError::Unsupported("native index state poisoned".into())
}
fn cap_notes(notes: &mut Vec<String>) {
    notes.truncate(32);
    for note in notes {
        if let Some((end, _)) = note.char_indices().nth(512) {
            note.truncate(end);
        }
    }
}
