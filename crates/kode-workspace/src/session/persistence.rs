use super::{EditorStateV1, ProjectSessionV1, legacy};
use kode_core::{
    DocumentOrigin, DomainError, JobTag, RequestId, Result, WorkspaceGeneration,
    intelligence::DocumentSnapshot,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::{
    files::{NativeDocumentIo, disk_identity},
    jobs::{Delivery, OwnerId, WorkerScheduler},
};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_SESSION_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    New,
    Rust,
    Legacy,
    Previous,
}
pub struct LoadedSession {
    pub session: ProjectSessionV1,
    pub source: SessionSource,
    pub disk_identity: Option<DiskIdentity>,
    pub warning: Option<String>,
}
#[derive(Debug, Clone)]
pub struct SessionStore {
    pub root: PathBuf,
    validator: Option<fn(&ProjectSessionV1) -> Result<()>>,
}
fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("stat session", path, error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(DomainError::Unsupported(format!(
            "session must be a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_SESSION_BYTES {
        return Err(DomainError::Unsupported(
            "session exceeds 256 MiB; file retained for recovery".into(),
        ));
    }
    let mut bytes = vec![];
    fs::File::open(path)
        .and_then(|file| file.take(MAX_SESSION_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| io_error("read session", path, error))?;
    if bytes.len() as u64 > MAX_SESSION_BYTES {
        return Err(DomainError::Unsupported(
            "session grew beyond 256 MiB; file retained for recovery".into(),
        ));
    }
    Ok(Some(bytes))
}
fn parse_rust(bytes: &[u8]) -> Result<ProjectSessionV1> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let session: ProjectSessionV1 =
        serde_json::from_slice(bytes).map_err(|error| DomainError::Parse {
            format: "Rust session",
            message: error.to_string(),
        })?;
    session.validate_version()?;
    Ok(session)
}
fn atomic_copy(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path)
        .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(DomainError::Unsupported(
            "session recovery target is not a regular file".into(),
        ));
    }
    let mut file = tempfile::NamedTempFile::new_in(path.parent().expect("session parent"))
        .map_err(|error| io_error("create session recovery", path, error))?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| io_error("write session recovery", path, error))?;
    file.persist(path)
        .map_err(|error| io_error("replace session recovery", path, error.error))?;
    #[cfg(unix)]
    fs::File::open(path.parent().expect("session parent"))
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error("flush session recovery directory", path, error))?;
    Ok(())
}
impl SessionStore {
    pub fn new(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root)
            .map_err(|error| io_error("resolve session root", root, error))?;
        if !root.is_dir() {
            return Err(DomainError::Unsupported(
                "session root is not a directory".into(),
            ));
        }
        Ok(Self {
            root,
            validator: None,
        })
    }
    /// Composition supplies feature export policy while storage retains opaque
    /// JSON blocks and no dependency on feature crates.
    pub fn with_validator(mut self, validator: fn(&ProjectSessionV1) -> Result<()>) -> Self {
        self.validator = Some(validator);
        self
    }
    pub fn path(&self) -> PathBuf {
        self.root.join(".kode-rs.json")
    }
    pub fn load(&self) -> Result<LoadedSession> {
        let path = self.path();
        if let Some(bytes) = read(&path)? {
            let session = match parse_rust(&bytes) {
                Ok(session) => session,
                // Future versions never silently roll back to an older format.
                Err(error @ DomainError::Unsupported(_)) => return Err(error),
                Err(error) => {
                    let previous = self.root.join(".kode-rs.json.previous");
                    if let Some(previous_bytes) = read(&previous)? {
                        let session = parse_rust(&previous_bytes)?;
                        return Ok(LoadedSession {
                            session,
                            source: SessionSource::Previous,
                            disk_identity: None,
                            warning: Some(format!(
                                "{error}; loaded previous state. Corrupt current file remains untouched; persistence is blocked until it is explicitly recovered."
                            )),
                        });
                    }
                    return Err(error);
                }
            };
            let identity = disk_identity(&path)?;
            // Do not pair a parsed snapshot with the identity of a concurrent edit.
            if read(&path)?.as_deref() != Some(bytes.as_slice()) {
                return Err(DomainError::SaveConflict(path));
            }
            return Ok(LoadedSession {
                session,
                source: SessionSource::Rust,
                disk_identity: Some(identity),
                warning: None,
            });
        }
        if let Some(bytes) = read(&self.root.join(".kode.json"))? {
            return Ok(LoadedSession {
                session: legacy::import(&bytes)?,
                source: SessionSource::Legacy,
                disk_identity: None,
                warning: Some(
                    "Imported Kotlin v6 session; saving uses .kode-rs.json and retains .kode.json"
                        .into(),
                ),
            });
        }
        let project = crate::project::Project::new(&self.root, WorkspaceGeneration(0))?;
        Ok(LoadedSession {
            session: ProjectSessionV1 {
                source_roots: project.config.source_roots,
                ..Default::default()
            },
            source: SessionSource::New,
            disk_identity: None,
            warning: None,
        })
    }
    pub fn save(
        &self,
        session: &ProjectSessionV1,
        expected: Option<&DiskIdentity>,
    ) -> Result<DiskIdentity> {
        session.validate_version()?;
        if let Some(validate) = self.validator {
            validate(session)?;
        }
        let path = self.path();
        if let Some(expected) = expected {
            if disk_identity(&path)? != *expected {
                return Err(DomainError::SaveConflict(path));
            }
            let current = read(&path)?.ok_or_else(|| DomainError::SaveConflict(path.clone()))?;
            parse_rust(&current)?; // Never overwrite malformed or future-version data.
            atomic_copy(&self.root.join(".kode-rs.json.previous"), &current)?;
        }
        let mut text =
            serde_json::to_string_pretty(session).map_err(|error| DomainError::Parse {
                format: "Rust session",
                message: error.to_string(),
            })?;
        text.push('\n');
        if text.len() as u64 > MAX_SESSION_BYTES {
            return Err(DomainError::Unsupported(
                "session exceeds 256 MiB; dirty state retained".into(),
            ));
        }
        let snapshot = DocumentSnapshot {
            id: Default::default(),
            revision: Default::default(),
            origin: DocumentOrigin::File(path.clone()),
            language: Some("json".into()),
            format: Default::default(),
            text: text.into(),
        };
        NativeDocumentIo.save(&path, &snapshot, expected)
    }
    /// Create a distinct restricted recovery file, never overwrite an existing
    /// recovery or the source editor. The returned path is displayed to the user.
    pub fn recover_editor(&self, state: &EditorStateV1) -> Result<PathBuf> {
        let directory = self.root.join(".kode-rs");
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(DomainError::Unsupported(
                    "recovery directory must not be a symlink".into(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&directory).map_err(|error| {
                    io_error("create editor recovery directory", &directory, error)
                })?
            }
            Err(error) => {
                return Err(io_error(
                    "stat editor recovery directory",
                    &directory,
                    error,
                ));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(
                |error| io_error("restrict editor recovery directory", &directory, error),
            )?;
        }
        let bytes = serde_json::to_vec_pretty(state).map_err(|error| DomainError::Parse {
            format: "editor recovery",
            message: error.to_string(),
        })?;
        let mut temporary = tempfile::Builder::new()
            .prefix("editor-recovery-")
            .suffix(".json")
            .tempfile_in(&directory)
            .map_err(|error| io_error("create editor recovery", &directory, error))?;
        temporary
            .write_all(&bytes)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| io_error("write editor recovery", &directory, error))?;
        let (_, path) = temporary
            .keep()
            .map_err(|error| io_error("retain editor recovery", &directory, error.error))?;
        #[cfg(unix)]
        fs::File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io_error("flush editor recovery directory", &directory, error))?;
        Ok(path)
    }
}

#[derive(Clone)]
pub struct SessionWriteJob {
    pub tag: JobTag,
    pub sequence: u64,
    pub session: Arc<ProjectSessionV1>,
    pub expected: Option<DiskIdentity>,
}
pub struct SessionPersistence {
    workspace: WorkspaceGeneration,
    session: Arc<ProjectSessionV1>,
    sequence: u64,
    saved_sequence: u64,
    changed_at: u64,
    active: Option<SessionWriteJob>,
    pub disk_identity: Option<DiskIdentity>,
    pub error: Option<DomainError>,
}
impl SessionPersistence {
    pub const DEBOUNCE_MS: u64 = 3_000;
    pub fn new(workspace: WorkspaceGeneration, loaded: LoadedSession) -> Self {
        Self {
            workspace,
            session: Arc::new(loaded.session),
            sequence: if loaded.source == SessionSource::Legacy {
                1
            } else {
                0
            },
            saved_sequence: 0,
            changed_at: 0,
            active: None,
            disk_identity: loaded.disk_identity,
            error: None,
        }
    }
    pub fn session(&self) -> &ProjectSessionV1 {
        &self.session
    }
    pub fn snapshot(&self) -> Arc<ProjectSessionV1> {
        self.session.clone()
    }
    pub fn changed(&mut self, session: ProjectSessionV1, now: u64) -> Result<()> {
        session.validate_version()?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("session sequence exhausted".into()))?;
        self.session = Arc::new(session);
        self.changed_at = now;
        Ok(())
    }
    pub fn is_dirty(&self) -> bool {
        self.sequence != self.saved_sequence
    }
    pub fn is_saving(&self) -> bool {
        self.active.is_some()
    }
    pub fn begin(&mut self, now: u64, force: bool) -> Result<Option<SessionWriteJob>> {
        if !self.is_dirty()
            || self.active.is_some()
            || (!force && now.saturating_sub(self.changed_at) < Self::DEBOUNCE_MS)
        {
            return Ok(None);
        }
        let job = SessionWriteJob {
            tag: JobTag {
                workspace: self.workspace,
                request: RequestId::fresh()?,
                document: None,
            },
            sequence: self.sequence,
            session: self.session.clone(),
            expected: self.disk_identity.clone(),
        };
        self.active = Some(job.clone());
        self.error = None;
        Ok(Some(job))
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<DiskIdentity>) -> bool {
        if self.active.as_ref().is_none_or(|job| job.tag != tag) {
            return false;
        }
        let job = self.active.take().expect("matching session write");
        match result {
            Ok(identity) => {
                self.saved_sequence = job.sequence;
                self.disk_identity = Some(identity);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
    /// If scheduling failed before a worker started, retire the request without
    /// losing pending state. A physical write's completion must always be drained.
    pub fn submission_failed(&mut self, tag: JobTag, error: DomainError) -> bool {
        self.complete(tag, Err(error))
    }
    /// Call after the shell has drained any active critical write and stopped
    /// interactive changes. The latest pending state is flushed off the UI thread.
    pub async fn flush_shutdown(&mut self, store: SessionStore) -> Result<()> {
        if self.is_saving() {
            return Err(DomainError::Unsupported(
                "drain the active session write before shutdown flush".into(),
            ));
        }
        if let Some(job) = self.begin(0, true)? {
            let tag = job.tag;
            let result = tokio::task::spawn_blocking(move || {
                store.save(&job.session, job.expected.as_ref())
            })
            .await
            .map_err(|error| {
                DomainError::Unsupported(format!("session flush worker failed: {error}"))
            })
            .and_then(|result| result);
            let failure = result.as_ref().err().map(ToString::to_string);
            self.complete(tag, result);
            if let Some(failure) = failure {
                return Err(DomainError::Unsupported(format!(
                    "session shutdown flush failed: {failure}"
                )));
            }
        }
        Ok(())
    }
}
pub fn schedule_write(
    scheduler: &mut dyn WorkerScheduler<DiskIdentity>,
    owner: OwnerId,
    store: SessionStore,
    job: SessionWriteJob,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Critical,
        Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            store.save(&job.session, job.expected.as_ref())
        }),
    )
}
