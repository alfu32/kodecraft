//! Filesystem/bootstrap/session operations consumed by the application workers.
use crate::editor::loading::{PreparedDocument, prepare};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, Result, WorkspaceGeneration,
    document::Document,
    editor::selection::CaretState,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::{files::NativeDocumentIo, jobs::CancellationToken};
use kode_workspace::{
    mime::{FileClassification, ImageCapabilities, MimeDetector, ViewerKind},
    project::Project,
    session::{
        EditorStateV1, ProjectSessionV1,
        persistence::{LoadedSession, SessionSource, SessionStore},
        relative_path, resolve_path,
        restore::restore_with_recovery,
    },
    watch::TreeWatcher,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct PreparedWorkspace {
    pub project: Project,
    pub session: LoadedSession,
    pub store: SessionStore,
    pub watcher: TreeWatcher,
    pub initial: Option<OpenRequest>,
}
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub path: PathBuf,
    pub saved: Option<Arc<EditorStateV1>>,
    pub navigation: Option<kode_core::intelligence::NavigationTarget>,
}
pub struct PreparedOpen {
    pub prepared: PreparedDocument,
    pub classification: FileClassification,
    pub caret: CaretState,
    pub scroll_rows: usize,
    pub recovery: Option<PathBuf>,
    pub warning: Option<String>,
    pub metadata: Option<Arc<EditorStateV1>>,
    pub navigation: Option<kode_core::intelligence::NavigationTarget>,
}
pub struct PreparedHex {
    pub path: PathBuf,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub warning: Option<String>,
}
pub struct PreparedImage {
    pub path: PathBuf,
    pub mime: String,
    pub image: kode_platform::images::DecodedImage,
}
pub enum PreparedViewer {
    Code(Box<PreparedOpen>),
    Hex(PreparedHex),
    Image(PreparedImage),
}
/// Worker-only routing. Binary load failures are explicit, never empty success.
pub fn prepare_viewer(
    root: &Path,
    request: OpenRequest,
    tag: JobTag,
    detector: &MimeDetector,
    images: &ImageCapabilities,
    cancel: &CancellationToken,
) -> Result<PreparedViewer> {
    let detected = match detector.detect_file(&request.path, cancel) {
        Ok(detected) => detected,
        Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
        Err(_) if request.saved.is_some() => {
            detector.detect_filename(&request.path.to_string_lossy())
        }
        Err(error) => return Err(error),
    };
    let mut classification = FileClassification::new(detected, images);
    if classification.viewer == ViewerKind::Code {
        match prepare_open(root, request.clone(), tag, detector, images, cancel) {
            Ok(opened) => return Ok(PreparedViewer::Code(Box::new(opened))),
            Err(
                error @ DomainError::Parse {
                    format: "UTF-8 text",
                    ..
                },
            ) => {
                classification.message = Some(format!("{error}; inspecting raw bytes"));
            }
            Err(error) => return Err(error),
        }
    }
    if classification.viewer == ViewerKind::Image {
        match kode_platform::images::decode::load_image(&request.path, cancel) {
            Ok(image) => {
                let path =
                    std::fs::canonicalize(&request.path).map_err(|source| DomainError::Io {
                        operation: "resolve image target",
                        path: request.path.clone(),
                        source,
                    })?;
                return Ok(PreparedViewer::Image(PreparedImage {
                    path,
                    mime: classification.detected.mime,
                    image,
                }));
            }
            Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
            Err(error) => {
                classification.message = Some(format!("{error}; inspecting raw image bytes"))
            }
        }
    }
    let canonical = std::fs::canonicalize(&request.path).map_err(|source| DomainError::Io {
        operation: "resolve binary target",
        path: request.path.clone(),
        source,
    })?;
    let limit = 64 * 1024 * 1024;
    let mut file = kode_platform::files::open_regular_file(&canonical)?;
    let before = file.metadata().map_err(|source| DomainError::Io {
        operation: "stat binary",
        path: request.path.clone(),
        source,
    })?;
    if before.len() > limit as u64 {
        return Err(DomainError::Unsupported(
            "hex file exceeds the 64 MiB buffer limit".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    let mut chunk = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let read = file.read(&mut chunk).map_err(|source| DomainError::Io {
            operation: "read binary",
            path: request.path.clone(),
            source,
        })?;
        if read == 0 {
            break;
        }
        if bytes.len() + read > limit {
            return Err(DomainError::Unsupported(
                "hex file grew beyond the 64 MiB buffer limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let after = file.metadata().map_err(|source| DomainError::Io {
        operation: "stat binary after load",
        path: request.path.clone(),
        source,
    })?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(request.path));
    }
    Ok(PreparedViewer::Hex(PreparedHex {
        path: canonical,
        bytes,
        mime: classification.detected.mime,
        warning: classification.message,
    }))
}
pub fn prepare_workspace(
    root: &Path,
    generation: WorkspaceGeneration,
    explicit_file: Option<PathBuf>,
    cancel: &CancellationToken,
) -> Result<PreparedWorkspace> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let initial_project = Project::new(root, generation)?;
    let store = SessionStore::new(&initial_project.root)?;
    let loaded = match store.load() {
        Ok(loaded) => loaded,
        Err(error) => LoadedSession {
            session: ProjectSessionV1 {
                source_roots: initial_project.config.source_roots,
                ..Default::default()
            },
            source: SessionSource::New,
            disk_identity: None,
            warning: Some(format!(
                "{error}; session file retained. Changes cannot overwrite it automatically."
            )),
        },
    };
    let project = Project::configured(&store.root, generation, loaded.session.project_config())?;
    let saved_for = |path: &Path| {
        loaded.session.open_editors.iter().chain(loaded.session.recent_files.iter().filter_map(|recent| recent.editor.as_ref()))
        .find(|state| matches!(&state.origin, DocumentOrigin::File(origin) if resolve_path(&store.root, origin).is_ok_and(|origin| origin == path))).cloned()
    };
    let initial = if let Some(path) = explicit_file {
        Some(OpenRequest {
            navigation: None,
            saved: saved_for(&path).map(Arc::new),
            path,
        })
    } else if let Some(DocumentOrigin::File(path)) = &loaded.session.active_origin {
        resolve_path(&store.root, path)
            .ok()
            .map(|path| OpenRequest {
                saved: saved_for(&path).map(Arc::new),
                path,
                navigation: None,
            })
    } else if let Some(recent) = loaded
        .session
        .recent_files
        .iter()
        .max_by_key(|recent| recent.last_opened_epoch_millis)
    {
        resolve_path(&store.root, &recent.path)
            .ok()
            .map(|path| OpenRequest {
                saved: saved_for(&path).map(Arc::new),
                path,
                navigation: None,
            })
    } else {
        let first = loaded
            .session
            .most_recent_editor()
            .filter(|state| matches!(state.origin, DocumentOrigin::File(_)))
            .or_else(|| {
                loaded
                    .session
                    .open_editors
                    .iter()
                    .find(|state| matches!(state.origin, DocumentOrigin::File(_)))
            });
        first.and_then(|state| match &state.origin {
            DocumentOrigin::File(path) => {
                resolve_path(&store.root, path)
                    .ok()
                    .map(|path| OpenRequest {
                        path,
                        saved: Some(Arc::new(state.clone())),
                        navigation: None,
                    })
            }
            _ => None,
        })
    };
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let watcher = TreeWatcher::new(&project.root);
    Ok(PreparedWorkspace {
        project,
        session: loaded,
        store,
        watcher,
        initial,
    })
}
pub fn prepare_open(
    root: &Path,
    request: OpenRequest,
    tag: JobTag,
    detector: &MimeDetector,
    images: &ImageCapabilities,
    cancel: &CancellationToken,
) -> Result<PreparedOpen> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    // A missing saved file can still recover its dirty buffer. Sniffing failure
    // is retained as feedback instead of substituting an empty document.
    let detected = match detector.detect_file(&request.path, cancel) {
        Ok(detected) => detected,
        Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
        Err(error) if request.saved.is_some() => {
            let _ = error;
            detector.detect_filename(&request.path.to_string_lossy())
        }
        Err(error) => return Err(error),
    };
    let classification = FileClassification::new(detected, images);
    if classification.viewer != ViewerKind::Code {
        return Err(DomainError::Unsupported(format!(
            "{} needs a {:?} viewer",
            request.path.display(),
            classification.viewer
        )));
    }
    let (prepared, caret, scroll_rows, recovery, warning, metadata) =
        if let Some(state) = request.saved {
            let store = SessionStore::new(root)?;
            let restored = restore_with_recovery(&store, &state, tag, &NativeDocumentIo)?;
            let warning = restored.editor.disk_error.as_ref().map(ToString::to_string);
            let metadata = Some(state);
            (
                PreparedDocument {
                    document: restored.editor.document,
                    disk_identity: restored.editor.disk_identity,
                },
                restored.editor.caret,
                restored.editor.scroll_top,
                restored.recovery_path,
                warning,
                metadata,
            )
        } else {
            let loaded = NativeDocumentIo.load(&DocumentOrigin::File(request.path), tag)?;
            let prepared = prepare(
                loaded,
                DocumentId::fresh()?,
                classification.detected.language.clone(),
            )?;
            (prepared, CaretState::default(), 0, None, None, None)
        };
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    Ok(PreparedOpen {
        prepared,
        classification,
        caret,
        scroll_rows,
        recovery,
        warning,
        metadata,
        navigation: request.navigation,
    })
}
#[derive(Clone)]
pub struct CapturedDocument {
    pub document: Document,
    pub caret: CaretState,
    pub scroll_rows: usize,
    pub disk_identity: Option<DiskIdentity>,
    pub metadata: Option<Arc<EditorStateV1>>,
}
#[derive(Clone)]
pub enum SessionChange {
    Recent(Box<kode_workspace::session::RecentFileV1>),
    RemoveRecent(PathBuf),
    Move { from: PathBuf, to: PathBuf },
}
pub fn capture_changes(
    base: Arc<ProjectSessionV1>,
    project: &Project,
    captured: Vec<CapturedDocument>,
    active: Option<DocumentOrigin>,
    changes: Vec<SessionChange>,
) -> Result<ProjectSessionV1> {
    let mut base = (*base).clone();
    for change in changes {
        match change {
            SessionChange::Recent(mut recent) => {
                if let Some(old) = base.recent_files.iter().find(|old| old.path == recent.path) {
                    recent.extra = old.extra.clone();
                    if recent.editor.is_none() {
                        recent.editor = old.editor.clone();
                    }
                }
                base.record_recent(*recent);
            }
            SessionChange::RemoveRecent(path) => base.remove_recent(&project.root, &path)?,
            SessionChange::Move { from, to } => base.reconcile_move(&project.root, &from, &to)?,
        }
    }
    capture_session(base, project, captured, active)
}
/// Worker-only serialization snapshot. Merge, rather than erase, states which
/// have not opened successfully (including recoverable old feature metadata).
pub fn capture_session(
    mut base: ProjectSessionV1,
    project: &Project,
    captured: Vec<CapturedDocument>,
    active: Option<DocumentOrigin>,
) -> Result<ProjectSessionV1> {
    base.source_roots = project.config.source_roots.clone();
    base.scan_exclusions = project.config.exclusions.clone();
    base.ignore_patterns = project.config.ignore_patterns.clone();
    base.active_origin = active.map(|mut origin| {
        if let DocumentOrigin::File(path) = &mut origin {
            *path = relative_path(&project.root, path);
        }
        origin
    });
    for captured in captured {
        if !matches!(captured.document.origin(), DocumentOrigin::File(_)) {
            continue;
        }
        let mut state = EditorStateV1::capture(
            &project.root,
            &captured.document,
            &captured.caret,
            captured.scroll_rows,
            captured.disk_identity,
        );
        if let Some(metadata) = captured.metadata {
            state.mime = metadata.mime.clone();
            state.grammar_language = metadata.grammar_language.clone();
            state.grammar_available = metadata.grammar_available;
            state.extra = metadata.extra.clone();
        }
        let same = |existing: &EditorStateV1| match (&existing.origin, &state.origin) {
            (DocumentOrigin::File(a), DocumentOrigin::File(b)) => {
                resolve_path(&project.root, a).ok() == resolve_path(&project.root, b).ok()
            }
            (a, b) => a == b,
        };
        if let Some(index) = base.open_editors.iter().position(same) {
            base.open_editors[index] = state.clone();
        } else {
            base.open_editors.push(state.clone());
        }
        let DocumentOrigin::File(path) = &state.origin else {
            continue;
        };
        for recent in &mut base.recent_files {
            if resolve_path(&project.root, &recent.path)? == resolve_path(&project.root, path)? {
                recent.editor = Some(state.clone());
                recent.dirty = !state.history.saved_baseline_known
                    || state.history.current.text != state.history.saved_text;
                recent.last_modified_millis = state.last_modified_millis;
            }
        }
    }
    Ok(base)
}
pub fn epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
