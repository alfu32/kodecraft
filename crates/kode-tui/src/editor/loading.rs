//! Main-thread load and save lifecycle; I/O/decoding run in platform workers.
use crate::app::events::ResultGate;
use kode_core::services::DocumentIo;
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Result, WorkspaceGeneration,
};
use kode_platform::jobs::{Delivery, OwnerId, WorkerScheduler};
use std::sync::Arc;

pub fn schedule_load(
    scheduler: &mut dyn WorkerScheduler<PreparedDocument>,
    owner: OwnerId,
    tag: JobTag,
    id: DocumentId,
    origin: DocumentOrigin,
    language: Option<String>,
    io: Arc<dyn DocumentIo>,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        Delivery::Cancellable,
        Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let loaded = io.load(&origin, tag)?;
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            prepare(loaded, id, language)
        }),
    )
}
pub fn schedule_save(
    scheduler: &mut dyn WorkerScheduler<DiskIdentity>,
    owner: OwnerId,
    job: SaveJob,
    io: Arc<dyn DocumentIo>,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Critical,
        Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let DocumentOrigin::File(path) = &job.snapshot.origin else {
                return Err(DomainError::NoSaveTarget);
            };
            io.save(
                path,
                &job.snapshot.to_service_snapshot(),
                job.expected_disk.as_ref(),
            )
        }),
    )
}
use kode_core::{
    document::{Document, TextSnapshot, encoding::decode},
    services::{DiskIdentity, LoadedDocument},
};

pub struct PreparedDocument {
    pub document: Document,
    pub disk_identity: Option<DiskIdentity>,
}
pub fn prepare(
    loaded: LoadedDocument,
    id: DocumentId,
    language: Option<String>,
) -> Result<PreparedDocument> {
    let decoded = decode(&loaded.bytes)?;
    Ok(PreparedDocument {
        document: Document::from_decoded(id, loaded.origin, language, decoded),
        disk_identity: loaded.disk_identity,
    })
}

pub struct DocumentLoader {
    gate: ResultGate,
    active: Option<(JobTag, DocumentId, DocumentOrigin)>,
    pub ready: Option<PreparedDocument>,
    pub error: Option<DomainError>,
}
impl DocumentLoader {
    pub fn new(workspace: WorkspaceGeneration) -> Self {
        Self {
            gate: ResultGate::new(workspace),
            active: None,
            ready: None,
            error: None,
        }
    }
    pub fn begin(&mut self, id: DocumentId, origin: DocumentOrigin) -> JobTag {
        let tag = self.gate.request("open".into(), None);
        self.active = Some((tag, id, origin));
        self.ready = None;
        self.error = None;
        tag
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<PreparedDocument>) -> bool {
        if !self.gate.accepts("open", tag) {
            return false;
        }
        let Some((_, id, origin)) = &self.active else {
            return false;
        };
        match result {
            Ok(prepared)
                if prepared.document.id() == *id && prepared.document.origin() == origin =>
            {
                self.ready = Some(prepared);
                self.error = None;
            }
            Ok(_) => {
                self.error = Some(DomainError::Unsupported(
                    "loaded document identity mismatch".into(),
                ))
            }
            Err(error) => self.error = Some(error),
        }
        self.active = None;
        self.gate.dismiss("open");
        true
    }
    pub fn is_loading(&self) -> bool {
        self.active.is_some()
    }
    pub fn switch_workspace(&mut self) {
        self.gate.switch_workspace();
        self.active = None;
        self.ready = None;
        self.error = None;
    }
}

#[derive(Debug, Clone)]
pub struct SaveJob {
    pub tag: JobTag,
    pub snapshot: TextSnapshot,
    pub expected_disk: Option<DiskIdentity>,
}
/// One active save per document; a later requested snapshot waits for its result
/// and then uses the new disk identity. Critical results are never revision-gated
/// away merely because the user continued typing.
pub struct SaveController {
    workspace: WorkspaceGeneration,
    active: Option<SaveJob>,
    queued: Option<TextSnapshot>,
    pub disk_identity: Option<DiskIdentity>,
    pub error: Option<DomainError>,
}
impl SaveController {
    pub fn new(workspace: WorkspaceGeneration, disk_identity: Option<DiskIdentity>) -> Self {
        Self {
            workspace,
            active: None,
            queued: None,
            disk_identity,
            error: None,
        }
    }
    fn job(&mut self, snapshot: TextSnapshot) -> Result<SaveJob> {
        let tag = JobTag {
            workspace: self.workspace,
            request: RequestId::fresh()?,
            document: Some((snapshot.id, snapshot.revision)),
        };
        let job = SaveJob {
            tag,
            snapshot,
            expected_disk: self.disk_identity.clone(),
        };
        self.active = Some(job.clone());
        Ok(job)
    }
    pub fn request(&mut self, document: &Document) -> Result<Option<SaveJob>> {
        if document.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        if !matches!(document.origin(), DocumentOrigin::File(_)) {
            return Err(DomainError::NoSaveTarget);
        }
        let snapshot = document.snapshot();
        self.error = None;
        if let Some(active) = &self.active {
            if active.snapshot.id != snapshot.id || active.snapshot.origin != snapshot.origin {
                return Err(DomainError::Unsupported(
                    "save controller belongs to another document".into(),
                ));
            }
            self.queued = Some(snapshot);
            Ok(None)
        } else {
            self.job(snapshot).map(Some)
        }
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        result: Result<DiskIdentity>,
        document: &mut Document,
    ) -> Result<Option<SaveJob>> {
        if self.active.as_ref().is_none_or(|job| job.tag != tag) {
            return Ok(None);
        }
        let active = self.active.take().expect("active save checked");
        match result {
            Ok(identity) => {
                document.acknowledge_save(&active.snapshot)?;
                self.disk_identity = Some(identity);
                self.error = None;
                if let Some(snapshot) = self.queued.take() {
                    self.job(snapshot).map(Some)
                } else {
                    Ok(None)
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.queued = None;
                Ok(None)
            }
        }
    }
    pub fn is_saving(&self) -> bool {
        self.active.is_some()
    }
    pub fn retarget_file(&mut self, path: &std::path::Path) -> Result<()> {
        if self.is_saving() {
            return Err(DomainError::Unsupported(
                "wait for the active save before moving its file".into(),
            ));
        }
        if let Some(identity) = &mut self.disk_identity {
            identity.canonical_path = path.into();
        }
        self.error = None;
        Ok(())
    }
}
