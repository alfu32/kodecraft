//! Bounded work execution and application-thread publication.
mod intelligence;
pub mod lsp;
use super::{
    state::{AppState, AppWork, ConfigAction, OpenDestination, PickerKind},
    workspace::{self, OpenRequest, PreparedOpen, PreparedWorkspace},
};
use crate::{
    editor::{
        folding::{self, FoldReport},
        host::{
            ClipboardJob, ClipboardResult, EditorView, EditorWork, HostDescriptor, HostRole,
            PasteTarget, QueuedEditorHost,
        },
        popups::{
            completion,
            definition::{self, NativePreviewSourceReader, NavigationResult},
        },
    },
    focus::FocusId,
    panels::{project::ProjectWork, project_search::ProjectSearchWork},
    widgets::folder_picker::FolderPicker,
};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Result,
    editor::search::SearchReport,
    intelligence::CompletionItem,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::{
    files::NativeDocumentIo,
    jobs::{
        BlockingJobFactory, Clock, Completion, Delivery, MonotonicClock, Notifications, OwnerId,
        Scheduler,
    },
};
use kode_workspace::{
    mime::{ImageCapabilities, MimeDetector},
    project::Project,
    scanner::{self, ScanBatch, ScanSummary},
    search::{self, ProjectSearchReport},
    session::ProjectSessionV1,
    tree::{self, FileChange, TreeSnapshot},
};
use ratatui::layout::Rect;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;

enum Output {
    LspSettings(Box<crate::panels::settings::lsp::SettingsReport>),
    IndexPrepared(Box<kode_intelligence::service::PreparedIndex>),
    IndexCommitted(
        Box<(
            kode_intelligence::service::PreparedIndex,
            kode_intelligence::store::CommitReceipt,
        )>,
    ),
    Tree(TreeSnapshot),
    File(FileChange),
    Open(Box<PreparedOpen>),
    Hex(workspace::PreparedHex),
    Image(workspace::PreparedImage),
    Cells(kode_platform::images::cells::CellImage),
    Scan(ScanSummary),
    Session(Box<ProjectSessionV1>),
    Saved(DiskIdentity),
    Fold(FoldReport),
    Overlays(crate::editor::overlays::OverlayReport),
    Search(SearchReport),
    ProjectSearch(ProjectSearchReport),
    Completion(Vec<CompletionItem>),
    Navigation(NavigationResult),
    Clipboard(ClipboardResult),
    Project(Project),
    Picker(Project),
}
enum Context {
    LspSettings,
    IndexPrepared,
    IndexCommitted,
    Tree(bool),
    Cells,
    File,
    Open(OpenDestination),
    Scan,
    Capture(u64),
    Persist,
    Save(DocumentId),
    Fold,
    Overlays,
    Search,
    ProjectSearch,
    Completion,
    Navigation,
    Clipboard(ClipboardTarget),
    Project,
    Picker(PickerKind),
}
enum ClipboardTarget {
    None,
    Names,
    Project(FocusId),
    Editor(OwnerId, PasteTarget),
}
struct Pending {
    tag: JobTag,
    owner: OwnerId,
    delivery: Delivery,
    key: Option<(OwnerId, &'static str)>,
    context: Context,
    work: BlockingJobFactory<Output>,
}
pub struct AppRuntime {
    lsp: Option<lsp::LspRuntime>,
    intelligence: Option<kode_intelligence::service::CodeIntelService>,
    intelligence_resources: Option<Arc<kode_intelligence::facade::IntelligenceResources>>,
    overlay_worker: Option<Arc<crate::editor::overlays::OverlayWorker>>,
    query_versions: HashMap<JobTag, kode_intelligence::service::QueryRevision>,
    native_watch_revision: u64,
    native_watch_due: Option<u64>,
    native_last_poll: u64,
    native_auto_scan: bool,
    native_scan_cancel: Option<kode_platform::jobs::CancellationToken>,
    pub state: AppState,
    jobs: Scheduler<Output>,
    contexts: HashMap<JobTag, Context>,
    latest: HashMap<(OwnerId, &'static str), JobTag>,
    pending: VecDeque<Pending>,
    batches: mpsc::Receiver<ScanBatch>,
    batch_sender: mpsc::Sender<ScanBatch>,
    root_owner: OwnerId,
    clipboard_owner: OwnerId,
    detector: MimeDetector,
    images: ImageCapabilities,
    config_queue: VecDeque<(JobTag, ConfigAction)>,
    config_active: bool,
    pub notifications: Notifications,
}
impl AppRuntime {
    pub fn new(prepared: PreparedWorkspace) -> Result<Self> {
        let clock: Arc<dyn Clock> = Arc::new(MonotonicClock::default());
        Self::with_services(
            prepared,
            Arc::new(kode_platform::clipboard::ClipboardService::system()),
            clock,
        )
    }
    pub fn with_services(
        prepared: PreparedWorkspace,
        clipboard: Arc<kode_platform::clipboard::ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let notifications = Notifications::new(clock.clone());
        let jobs = Scheduler::new(notifications.clone());
        let (batch_sender, batches) = mpsc::channel(8);
        let mut state = AppState::new(prepared, clipboard, clock)?;
        let detector = Self::image_detector(MimeDetector::compiled())?;
        state.detector = detector.clone();
        Ok(Self {
            lsp: None,
            intelligence: None,
            intelligence_resources: None,
            overlay_worker: None,
            query_versions: HashMap::new(),
            native_watch_revision: 0,
            native_watch_due: None,
            native_last_poll: 0,
            native_auto_scan: true,
            native_scan_cancel: None,
            state,
            jobs,
            contexts: HashMap::new(),
            latest: HashMap::new(),
            pending: VecDeque::new(),
            batches,
            batch_sender,
            root_owner: OwnerId(RequestId::fresh()?.0),
            clipboard_owner: OwnerId(RequestId::fresh()?.0),
            detector,
            images: ImageCapabilities::new(
                kode_platform::images::DECODER_MIMES
                    .iter()
                    .map(|mime| (*mime).to_owned()),
            ),
            config_queue: VecDeque::new(),
            config_active: false,
            notifications,
        })
    }
    fn image_detector(mut detector: MimeDetector) -> Result<MimeDetector> {
        for (extension, mime) in kode_platform::images::IMAGE_EXTENSION_DEFAULTS {
            detector.register_image_extension_if_absent(extension, mime)?;
        }
        Ok(detector)
    }
    pub fn set_detector(&mut self, detector: MimeDetector) -> Result<()> {
        let detector = Self::image_detector(detector)?;
        self.state.detector = detector.clone();
        self.detector = detector;
        Ok(())
    }
    fn queue(&mut self, pending: Pending) -> Result<()> {
        if let Some(key) = pending.key {
            if let Some(old) = self.latest.insert(key, pending.tag) {
                self.jobs.cancel_request(old);
            }
            self.pending.retain(|old| old.key != Some(key));
        }
        if self.pending.len() >= 64 {
            self.contexts.insert(pending.tag, pending.context);
            return self.complete(Completion {
                tag: pending.tag,
                owner: pending.owner,
                result: Err(DomainError::QueueFull),
            });
        }
        self.pending.push_back(pending);
        Ok(())
    }
    fn blocking(
        &mut self,
        tag: JobTag,
        owner: OwnerId,
        operation: &'static str,
        critical: bool,
        context: Context,
        work: BlockingJobFactory<Output>,
    ) -> Result<()> {
        self.queue(Pending {
            tag,
            owner,
            context,
            work,
            delivery: if critical {
                Delivery::Critical
            } else {
                Delivery::Cancellable
            },
            key: (!critical).then_some((owner, operation)),
        })
    }
    pub fn pump(&mut self) -> Result<()> {
        while let Ok(batch) = self.batches.try_recv() {
            self.state.scan.batch(batch);
        }
        while let Some(completion) = self.jobs.try_next() {
            self.complete(completion)?;
        }
        for progress in self.notifications.take_progress().into_values() {
            self.state.lsp_settings.progress(&progress);
            if let Some(dialog) = &mut self.state.rescan {
                dialog.progress(&progress);
            }
        }
        self.state.tick()?;
        let mut active = vec![
            OwnerId(self.state.lsp_settings.id.0),
            self.root_owner,
            self.clipboard_owner,
            OwnerId(self.state.files.id.0),
        ];
        if let Some(view) = &self.state.editor {
            active.push(view.owner());
        }
        if let Some(view) = &self.state.image {
            active.push(OwnerId(view.id.0));
        }
        if let Some(dialog) = &self.state.search {
            active.extend([dialog.owner, OwnerId(dialog.id.0)]);
            if let Some(view) = &dialog.editor {
                active.push(view.owner());
            }
        }
        if let Some((_, picker)) = &self.state.picker {
            active.push(OwnerId(picker.id.0));
        }
        let stale: Vec<_> = self
            .latest
            .iter()
            .filter(|((owner, _), _)| !active.contains(owner))
            .map(|(key, tag)| (*key, *tag))
            .collect();
        for (key, tag) in stale {
            self.jobs.cancel_request(tag);
            self.latest.remove(&key);
            self.pending.retain(|work| work.key != Some(key));
        }
        for work in self.state.take_work() {
            if let Err(error) = self.accept(work) {
                self.state.notice = error.to_string();
            }
        }
        self.pump_intelligence()?;
        self.pump_lsp();
        if !self.config_active
            && let Some((tag, action)) = self.config_queue.pop_front()
        {
            self.start_config(tag, action)?;
        }
        while self.jobs.can_submit_blocking() {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            let tag = pending.tag;
            self.contexts.insert(tag, pending.context);
            if let Err(error) =
                self.jobs
                    .submit_blocking(pending.owner, tag, pending.delivery, pending.work)
            {
                self.complete(Completion {
                    tag,
                    owner: pending.owner,
                    result: Err(error),
                })?;
            }
        }
        Ok(())
    }
    fn accept(&mut self, work: AppWork) -> Result<()> {
        match work {
            AppWork::LspSettings(action) => self.lsp_work(action),
            AppWork::Image(job) => {
                let owner = self
                    .state
                    .image
                    .as_ref()
                    .map(|view| OwnerId(view.id.0))
                    .ok_or(DomainError::Cancelled)?;
                self.blocking(
                    job.tag,
                    owner,
                    "image-cells",
                    false,
                    Context::Cells,
                    Box::new(move |cancel| {
                        kode_platform::images::cells::render_cells(
                            &job.image,
                            job.options,
                            job.full_height,
                            || cancel.is_cancelled(),
                        )
                        .map(Output::Cells)
                    }),
                )
            }
            AppWork::Tree { picker, job } => {
                let tag = job.tag;
                let owner = if picker {
                    self.state
                        .picker
                        .as_ref()
                        .map_or(self.root_owner, |(_, picker)| OwnerId(picker.id.0))
                } else {
                    OwnerId(self.state.files.id.0)
                };
                self.blocking(
                    tag,
                    owner,
                    "tree",
                    false,
                    Context::Tree(picker),
                    Box::new(move |cancel| tree::read_tree(&job, &cancel).map(Output::Tree)),
                )
            }
            AppWork::FileAction { tag, action } => {
                let paths = match &action {
                    tree::FileAction::Rename { path, .. } | tree::FileAction::Delete { path } => {
                        vec![path.clone()]
                    }
                    _ => vec![],
                };
                self.state.documents.reserve_file_action(tag, paths)?;
                let project = self.state.project.clone();
                self.blocking(
                    tag,
                    self.root_owner,
                    "file",
                    true,
                    Context::File,
                    Box::new(move |cancel| {
                        tree::perform_action(&project, &action, &cancel).map(Output::File)
                    }),
                )
            }
            AppWork::Open {
                tag,
                request,
                destination,
            } => {
                let root = self.state.project.root.clone();
                let detector = self.detector.clone();
                let images = self.images.clone();
                let session = self.state.persistence.snapshot();
                let owner = match destination {
                    OpenDestination::Main => self.root_owner,
                    OpenDestination::ProjectSearch(id) => OwnerId(id.0),
                };
                // Registry identity owns current text/history, even if the file
                // was removed or externally changed since the first open.
                if destination == OpenDestination::Main
                    && self.state.hexes.contains_key(&request.path)
                {
                    self.state.install_hex(
                        tag,
                        workspace::PreparedHex {
                            path: request.path,
                            bytes: vec![],
                            mime: String::new(),
                            warning: None,
                        },
                    )?;
                    return Ok(());
                }
                let existing = request
                    .navigation
                    .as_ref()
                    .and_then(|target| target.document)
                    .filter(|id| self.state.documents.document(*id).is_ok())
                    .or_else(|| {
                        self.state
                            .documents
                            .find_origin(&DocumentOrigin::File(request.path.clone()))
                    });
                if let Some(id) = existing {
                    let document = self.state.documents.document(id)?.clone();
                    let mut detected = detector.detect_filename(&request.path.to_string_lossy());
                    detected.language = document.snapshot().language;
                    let stored = self.state.views.get(&id);
                    let opened = PreparedOpen {
                        prepared: crate::editor::loading::PreparedDocument {
                            document,
                            disk_identity: self.state.documents.disk_identity(id).cloned(),
                        },
                        classification: kode_workspace::mime::FileClassification::new(
                            detected, &images,
                        ),
                        caret: stored.map_or_else(Default::default, |state| state.caret.clone()),
                        scroll_rows: stored.map_or(0, |state| state.scroll_rows),
                        recovery: None,
                        warning: None,
                        metadata: stored.and_then(|state| state.metadata.clone()),
                        navigation: request.navigation,
                    };
                    self.contexts.insert(tag, Context::Open(destination));
                    return self.complete(Completion {
                        tag,
                        owner,
                        result: Ok(Output::Open(Box::new(opened))),
                    });
                }
                self.blocking(tag, owner, "open", false, Context::Open(destination), Box::new(move |cancel| {
                    let mut request = request;
                    if request.saved.is_none() { request.saved = session.open_editors.iter().find(|state| matches!(&state.origin, DocumentOrigin::File(path) if kode_workspace::session::resolve_path(&root, path).ok().as_ref() == Some(&request.path))).cloned().map(Arc::new); }
                    workspace::prepare_viewer(&root, request, tag, &detector, &images, &cancel).map(|opened| match opened { workspace::PreparedViewer::Code(opened) => Output::Open(opened), workspace::PreparedViewer::Hex(opened) => Output::Hex(opened), workspace::PreparedViewer::Image(opened) => Output::Image(opened) })
                }))
            }
            AppWork::Scan {
                tag,
                project,
                token,
            } => {
                if let Some(service) = &mut self.intelligence {
                    service.rescan(project.clone(), false)?;
                    self.native_auto_scan = true;
                    self.native_scan_cancel = Some(token.clone());
                }
                let sender = self.batch_sender.clone();
                let notify = self.notifications.clone();
                let scan_owner = self.root_owner;
                self.blocking(
                    tag,
                    self.root_owner,
                    "scan",
                    false,
                    Context::Scan,
                    Box::new(move |cancel| {
                        if cancel.is_cancelled() {
                            return Err(DomainError::Cancelled);
                        }
                        scanner::scan(&project, tag, token.clone(), |mut batch| {
                            notify.progress(
                                scan_owner,
                                tag,
                                batch.files.last().map_or_else(String::new, |file| {
                                    file.relative.to_string_lossy().into_owned()
                                }),
                                batch.progress.accepted,
                                None,
                            );
                            loop {
                                if cancel.is_cancelled() || token.is_cancelled() {
                                    token.cancel();
                                    return Err(DomainError::Cancelled);
                                }
                                match sender.try_send(batch) {
                                    Ok(()) => {
                                        notify.repaint();
                                        return Ok(());
                                    }
                                    Err(mpsc::error::TrySendError::Full(value)) => {
                                        batch = value;
                                        std::thread::sleep(Duration::from_millis(1));
                                    }
                                    Err(_) => return Err(DomainError::Cancelled),
                                }
                            }
                        })
                        .map(Output::Scan)
                    }),
                )
            }
            AppWork::Capture {
                tag,
                sequence,
                project,
                base,
                documents,
                active,
                changes,
            } => self.blocking(
                tag,
                self.root_owner,
                "capture",
                false,
                Context::Capture(sequence),
                Box::new(move |cancel| {
                    if cancel.is_cancelled() {
                        return Err(DomainError::Cancelled);
                    }
                    workspace::capture_changes(base, &project, documents, active, changes)
                        .map(|session| Output::Session(Box::new(session)))
                }),
            ),
            AppWork::Persist(job) => {
                let store = self.state.store.clone();
                self.blocking(
                    job.tag,
                    self.root_owner,
                    "persist",
                    true,
                    Context::Persist,
                    Box::new(move |_| {
                        store
                            .save(&job.session, job.expected.as_ref())
                            .map(Output::Saved)
                    }),
                )
            }
            AppWork::Editor { owner, work } => self.editor_work(owner, work),
            AppWork::Project(work) => {
                match work {
                    ProjectWork::AddSource | ProjectWork::AddExclusion => {
                        let kind = if work == ProjectWork::AddSource {
                            PickerKind::Source
                        } else {
                            PickerKind::Exclusion
                        };
                        let tag = self.state.tag()?;
                        self.state.pending_picker = Some(tag);
                        self.accept(AppWork::Picker { tag, kind })?;
                    }
                    ProjectWork::RemoveSource(path) => {
                        let tag = self.state.tag()?;
                        self.accept(AppWork::Configure {
                            tag,
                            action: ConfigAction::RemoveSource(path),
                        })?;
                    }
                    ProjectWork::RemoveExclusion(path) => {
                        let tag = self.state.tag()?;
                        self.accept(AppWork::Configure {
                            tag,
                            action: ConfigAction::RemoveExclusion(path),
                        })?;
                    }
                    ProjectWork::Rescan => self.state.request_scan(true)?,
                    ProjectWork::RebuildIndex => self.rebuild_index()?,
                    ProjectWork::ClearIndex => self.clear_index()?,
                }
                Ok(())
            }
            AppWork::Configure { tag, action } => {
                self.config_queue.push_back((tag, action));
                Ok(())
            }
            AppWork::ProjectSearch(work) => match work {
                ProjectSearchWork::Search(job) => {
                    let owner = self
                        .state
                        .search
                        .as_ref()
                        .map_or(self.root_owner, |dialog| dialog.owner);
                    self.blocking(
                        job.tag,
                        owner,
                        "project-search",
                        false,
                        Context::ProjectSearch,
                        Box::new(move |cancel| {
                            search::search(&job, &cancel).map(Output::ProjectSearch)
                        }),
                    )
                }
                ProjectSearchWork::OpenHit { tag, hit } => {
                    let id = self
                        .state
                        .search
                        .as_ref()
                        .map(|dialog| dialog.id)
                        .ok_or(DomainError::Cancelled)?;
                    self.accept(AppWork::Open {
                        tag,
                        request: OpenRequest {
                            path: hit.path,
                            saved: None,
                            navigation: None,
                        },
                        destination: OpenDestination::ProjectSearch(id),
                    })
                }
                ProjectSearchWork::Editor { owner, work } => self.editor_work(owner, work),
                ProjectSearchWork::Clipboard { tag, job } => {
                    let id = self
                        .state
                        .search
                        .as_ref()
                        .map(|dialog| dialog.id)
                        .ok_or(DomainError::Cancelled)?;
                    self.clipboard_work(tag, job, ClipboardTarget::Project(id))
                }
                ProjectSearchWork::Dismiss => Ok(()),
            },
            AppWork::Clipboard { tag, job, names } => self.clipboard_work(
                tag,
                job,
                if names {
                    ClipboardTarget::Names
                } else {
                    ClipboardTarget::None
                },
            ),
            AppWork::Picker { tag, kind } => {
                let root = if kind == PickerKind::Workspace {
                    self.state
                        .project
                        .root
                        .ancestors()
                        .last()
                        .expect("absolute workspace root")
                        .to_path_buf()
                } else {
                    self.state.project.root.clone()
                };
                let generation = self.state.generation;
                self.blocking(
                    tag,
                    self.root_owner,
                    "picker",
                    false,
                    Context::Picker(kind),
                    Box::new(move |cancel| {
                        if cancel.is_cancelled() {
                            return Err(DomainError::Cancelled);
                        }
                        Project::new(&root, generation).map(Output::Picker)
                    }),
                )
            }
        }
    }
    fn view(&self, owner: OwnerId) -> Option<&EditorView<QueuedEditorHost>> {
        self.state
            .editor
            .as_ref()
            .filter(|view| view.owner() == owner)
            .or_else(|| {
                self.state
                    .search
                    .as_ref()?
                    .editor
                    .as_ref()
                    .filter(|view| view.owner() == owner)
            })
    }
    fn start_config(&mut self, tag: JobTag, action: ConfigAction) -> Result<()> {
        self.config_active = true;
        let mut project = self.state.project.clone();
        self.blocking(
            tag,
            self.root_owner,
            "config",
            true,
            Context::Project,
            Box::new(move |cancel| {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                match action {
                    ConfigAction::AddSource(path) => {
                        project.add_source_root(&path)?;
                    }
                    ConfigAction::RemoveSource(path) => {
                        project.remove_source_root(&path)?;
                    }
                    ConfigAction::AddExclusion(path) => {
                        project.exclude(&path)?;
                    }
                    ConfigAction::RemoveExclusion(path) => {
                        project.include(&path)?;
                    }
                }
                Ok(Output::Project(project))
            }),
        )
    }
    fn clipboard_work(
        &mut self,
        tag: JobTag,
        job: ClipboardJob,
        target: ClipboardTarget,
    ) -> Result<()> {
        use kode_core::services::Clipboard;
        let clipboard = self.state.clipboard.clone();
        let operation = if matches!(job, ClipboardJob::Read) {
            "clipboard-read"
        } else {
            "clipboard-publish"
        };
        self.blocking(
            tag,
            self.clipboard_owner,
            operation,
            false,
            Context::Clipboard(target),
            Box::new(move |cancel| {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                match job {
                    ClipboardJob::Read => clipboard
                        .read()
                        .map(|text| Output::Clipboard(ClipboardResult::Read(text))),
                    ClipboardJob::Publish(epoch) => {
                        Ok(Output::Clipboard(ClipboardResult::Published {
                            epoch,
                            native: clipboard.flush_native(epoch),
                        }))
                    }
                }
            }),
        )
    }
    fn editor_work(&mut self, owner: OwnerId, work: EditorWork) -> Result<()> {
        match work {
            EditorWork::SaveFile(job) => {
                let id = job.snapshot.id;
                let save_owner = self.state.documents.save_owner(id)?;
                self.blocking(
                    job.tag,
                    save_owner,
                    "save",
                    true,
                    Context::Save(id),
                    Box::new(move |_| {
                        let DocumentOrigin::File(path) = &job.snapshot.origin else {
                            return Err(DomainError::NoSaveTarget);
                        };
                        NativeDocumentIo
                            .save(
                                path,
                                &job.snapshot.to_service_snapshot(),
                                job.expected_disk.as_ref(),
                            )
                            .map(Output::Saved)
                    }),
                )
            }
            EditorWork::SaveOwner(job) => {
                self.state.documents.complete_owner_save(
                    job.snapshot.id,
                    job.tag,
                    Err(DomainError::NoSaveTarget),
                )?;
                Ok(())
            }
            EditorWork::Folds { tag, snapshot } => self.blocking(
                tag,
                owner,
                "fold",
                false,
                Context::Fold,
                Box::new(move |cancel| {
                    folding::discover(&snapshot, || cancel.is_cancelled()).map(Output::Fold)
                }),
            ),
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => self.blocking(
                tag,
                owner,
                "search",
                false,
                Context::Search,
                Box::new(move |cancel| {
                    kode_core::editor::search::scan(&snapshot, &query, || cancel.is_cancelled())
                        .map(Output::Search)
                }),
            ),
            EditorWork::Completion(job) => {
                self.stamp_query(job.tag)?;
                let external = self.view(owner).and_then(|view| view.host.external.clone());
                let provider = self
                    .view(owner)
                    .and_then(|view| view.host.intelligence.clone());
                self.blocking(
                    job.tag,
                    owner,
                    "completion",
                    false,
                    Context::Completion,
                    Box::new(move |cancel| {
                        completion::prepare(&job, external.as_deref(), provider.as_deref(), || {
                            cancel.is_cancelled()
                        })
                        .map(Output::Completion)
                    }),
                )
            }
            EditorWork::Navigation(job) => {
                self.stamp_query(job.tag)?;
                let captured = self.intelligence.as_ref().map(|service| service.facade());
                let provider = self
                    .view(owner)
                    .and_then(|view| view.host.intelligence.clone())
                    .ok_or_else(|| {
                        DomainError::Unsupported("code intelligence is unavailable".into())
                    })?;
                self.blocking(
                    job.tag,
                    owner,
                    "navigation",
                    false,
                    Context::Navigation,
                    Box::new(move |cancel| {
                        definition::prepare(
                            &job,
                            provider.as_ref(),
                            &intelligence::AppPreviewSourceReader {
                                native: captured,
                                disk: NativePreviewSourceReader {
                                    io: Arc::new(NativeDocumentIo),
                                },
                            },
                            Some(&intelligence::AppPreviewHighlighter {
                                provider: provider.clone(),
                            }),
                            || cancel.is_cancelled(),
                        )
                        .map(Output::Navigation)
                    }),
                )
            }
            EditorWork::Overlays(job) => {
                let worker = self.overlay_worker.clone().ok_or_else(|| {
                    DomainError::Unsupported("highlight worker unavailable".into())
                })?;
                let provider = self
                    .view(owner)
                    .and_then(|view| view.host.intelligence.clone());
                self.stamp_query(job.tag)?;
                self.blocking(
                    job.tag,
                    owner,
                    "overlays",
                    false,
                    Context::Overlays,
                    Box::new(move |cancel| {
                        worker
                            .prepare(&job, provider.as_deref(), &cancel)
                            .map(Output::Overlays)
                    }),
                )
            }
            EditorWork::ClipboardPublish { epoch } => {
                let tag = self.state.tag()?;
                self.clipboard_work(tag, ClipboardJob::Publish(epoch), ClipboardTarget::None)
            }
            EditorWork::ClipboardRead(target) => {
                let tag = self.state.tag()?;
                self.clipboard_work(
                    tag,
                    ClipboardJob::Read,
                    ClipboardTarget::Editor(owner, target),
                )
            }
            EditorWork::Changed(_) => {
                self.state.changed()?;
                Ok(())
            }
            EditorWork::Navigate(target) => {
                if target.missing_source && target.document.is_none() {
                    return Err(DomainError::Unsupported(
                        "navigation source is unavailable".into(),
                    ));
                }
                let path = if target.path.is_absolute() {
                    target.path.clone()
                } else {
                    self.state.project.root.join(&target.path)
                };
                self.state.open(
                    OpenRequest {
                        path,
                        saved: None,
                        navigation: Some(target),
                    },
                    OpenDestination::Main,
                )
            }
            EditorWork::Execute(_) => Err(DomainError::Unsupported(
                "no execution service registered for this editor".into(),
            )),
            EditorWork::FieldLimitReached => Err(DomainError::Unsupported(
                "editor field exceeds its character limit".into(),
            )),
        }
    }
    fn complete(&mut self, completion: Completion<Output>) -> Result<()> {
        let Some(context) = self.contexts.remove(&completion.tag) else {
            return Ok(());
        };
        let Completion { tag, owner, result } = completion;
        if tag.workspace != self.state.generation {
            return Ok(());
        }
        if let Some(expected) = self.query_versions.remove(&tag)
            && self
                .intelligence
                .as_ref()
                .and_then(|service| service.query_revision().ok())
                != Some(expected)
        {
            if matches!(context, Context::Overlays) {
                return self.complete_editor(owner, tag, context, Err(DomainError::Cancelled));
            }
            return Ok(());
        }
        if self.state.closing
            && matches!(
                context,
                Context::Tree(_)
                    | Context::Cells
                    | Context::Open(_)
                    | Context::Fold
                    | Context::Overlays
                    | Context::Search
                    | Context::ProjectSearch
                    | Context::Completion
                    | Context::Navigation
                    | Context::Clipboard(_)
                    | Context::Picker(_)
            )
        {
            return Ok(());
        }
        match context {
            Context::LspSettings => {
                self.state.lsp_settings.complete(
                    tag,
                    result.and_then(|output| {
                        if let Output::LspSettings(report) = output {
                            Ok(*report)
                        } else {
                            Err(DomainError::Unsupported("wrong LSP settings result".into()))
                        }
                    }),
                );
            }
            Context::IndexPrepared => self.index_prepared(tag, result)?,
            Context::IndexCommitted => self.index_committed(tag, result)?,
            Context::Cells => {
                if let Some(view) = &mut self.state.image
                    && OwnerId(view.id.0) == owner
                {
                    let result = result.and_then(|output| {
                        if let Output::Cells(cells) = output {
                            Ok(cells)
                        } else {
                            Err(DomainError::Unsupported("wrong image cell result".into()))
                        }
                    });
                    view.complete(tag, result);
                }
            }
            Context::Tree(picker) => {
                let result = result.and_then(|output| {
                    if let Output::Tree(tree) = output {
                        Ok(tree)
                    } else {
                        Err(DomainError::Unsupported("wrong tree worker result".into()))
                    }
                });
                if picker {
                    if let Some((_, picker)) = &mut self.state.picker {
                        picker.complete(tag, result);
                    }
                } else {
                    self.state.files.complete_refresh(tag, result);
                }
            }
            Context::File => {
                let result = result.and_then(|output| {
                    if let Output::File(change) = output {
                        Ok(change)
                    } else {
                        Err(DomainError::Unsupported("wrong file worker result".into()))
                    }
                });
                self.state
                    .documents
                    .finish_file_action(tag, result.as_ref().ok())?;
                if let Some(change) = self.state.files.complete_action(tag, result)? {
                    self.index_file_change(&change)?;
                    self.state.reconcile_hex_change(&change);
                    self.state.project = self.state.files.project.clone();
                    if let FileChange::Renamed { from, to } = change {
                        self.state.changed()?;
                        self.state.session_changes.push((
                            self.state.sequence,
                            workspace::SessionChange::Move { from, to },
                        ));
                    }
                    self.state.changed()?;
                }
            }
            Context::Open(destination) => match result {
                Ok(Output::Open(opened)) => match destination {
                    OpenDestination::Main => {
                        self.state.install_main(tag, *opened)?;
                    }
                    OpenDestination::ProjectSearch(id) => {
                        if self
                            .state
                            .search
                            .as_ref()
                            .is_some_and(|dialog| dialog.id == id)
                        {
                            let origin = opened.prepared.document.origin().clone();
                            let mut descriptor = HostDescriptor::new(
                                HostRole::ProjectSearch,
                                "project-search".into(),
                                "preview".into(),
                                "preview".into(),
                                self.state.project.root.clone(),
                            );
                            descriptor.language = opened.classification.detected.language;
                            descriptor.mime = Some(opened.classification.detected.mime);
                            let document = match self.state.documents.find_origin(&origin) {
                                Some(document) => document,
                                None => self.state.documents.insert_file(
                                    opened.prepared,
                                    &descriptor,
                                    self.state.generation,
                                )?,
                            };
                            let view = EditorView::new(
                                document,
                                &self.state.documents,
                                QueuedEditorHost::new(descriptor),
                                self.state.generation,
                                self.state.clipboard.clone(),
                                self.state.clock.clone(),
                            )?;
                            if let Some(dialog) = &mut self.state.search {
                                dialog.attach_editor(
                                    tag,
                                    view,
                                    &mut self.state.documents,
                                    &mut self.state.focus,
                                )?;
                            }
                        }
                    }
                },
                Ok(Output::Image(opened)) => match destination {
                    OpenDestination::Main => {
                        self.state.install_image(tag, opened)?;
                    }
                    OpenDestination::ProjectSearch(id) => {
                        if let Some(dialog) = &mut self.state.search
                            && dialog.id == id
                        {
                            dialog.load_failed(
                                tag,
                                DomainError::Unsupported("search preview requires text".into()),
                            );
                        }
                    }
                },
                Ok(Output::Hex(opened)) => match destination {
                    OpenDestination::Main => {
                        self.state.install_hex(tag, opened)?;
                    }
                    OpenDestination::ProjectSearch(id) => {
                        if let Some(dialog) = &mut self.state.search
                            && dialog.id == id
                        {
                            dialog.load_failed(
                                tag,
                                DomainError::Unsupported("search preview requires text".into()),
                            );
                        }
                    }
                },
                Err(error) => {
                    if let OpenDestination::ProjectSearch(id) = destination {
                        if let Some(dialog) = &mut self.state.search
                            && dialog.id == id
                        {
                            dialog.load_failed(tag, error);
                        }
                    } else if self.state.pending_open == Some(tag) {
                        self.state.pending_open = None;
                        self.state.notice = error.to_string();
                    }
                }
                _ => {}
            },
            Context::Scan => {
                while let Ok(batch) = self.batches.try_recv() {
                    self.state.scan.batch(batch);
                }
                let result = result.and_then(|output| {
                    if let Output::Scan(summary) = output {
                        Ok(summary)
                    } else {
                        Err(DomainError::Unsupported("wrong scan worker result".into()))
                    }
                });
                let message = result.as_ref().err().map(ToString::to_string);
                let aborted = matches!(result, Err(DomainError::Cancelled));
                if self.state.scan.complete(tag, result) {
                    self.state.scan_token = None;
                    if self.intelligence.is_none()
                        && let Some(dialog) = &mut self.state.rescan
                    {
                        dialog.complete(
                            tag,
                            if aborted {
                                Err(DomainError::Cancelled)
                            } else if let Some(error) = &message {
                                Err(DomainError::Unsupported(error.clone()))
                            } else {
                                Ok(())
                            },
                        );
                    }
                }
                if let Some(error) = message {
                    self.state.notice = error;
                }
            }
            Context::Capture(sequence) => {
                let result = result.and_then(|output| {
                    if let Output::Session(session) = output {
                        Ok(*session)
                    } else {
                        Err(DomainError::Unsupported("wrong capture result".into()))
                    }
                });
                self.state.complete_capture(tag, sequence, result)?;
            }
            Context::Persist => {
                let result = result.and_then(|output| {
                    if let Output::Saved(identity) = output {
                        Ok(identity)
                    } else {
                        Err(DomainError::Unsupported("wrong persist result".into()))
                    }
                });
                let failure = result.as_ref().err().map(ToString::to_string);
                self.state.persistence.complete(tag, result);
                if let Some(error) = failure {
                    self.state.notice = error;
                    self.state.closing = false;
                    self.state.quit_requested = false;
                    self.state.switch_requested = None;
                }
            }
            Context::Save(id) => {
                let result = result.and_then(|output| {
                    if let Output::Saved(identity) = output {
                        Ok(identity)
                    } else {
                        Err(DomainError::Unsupported("wrong save result".into()))
                    }
                });
                if let Some(work) = self.state.documents.complete_file_save(id, tag, result)? {
                    self.editor_work(owner, work)?;
                }
                if let Some(error) = self.state.documents.save_error(id) {
                    self.state.notice = error.to_string();
                }
                self.state.changed()?;
            }
            Context::Picker(kind) => {
                if self.state.pending_picker == Some(tag) {
                    self.state.pending_picker = None;
                    match result {
                        Ok(Output::Picker(project)) => {
                            let mut picker = FolderPicker::new(
                                project,
                                match kind {
                                    PickerKind::Workspace => "Select workspace folder",
                                    PickerKind::Source => "Select source folder",
                                    PickerKind::Exclusion => "Select scan exclusion",
                                }
                                .into(),
                                kind != PickerKind::Exclusion,
                                &mut self.state.focus,
                            )?;
                            if kind == PickerKind::Workspace {
                                picker.focus_path(self.state.project.root.clone())?;
                            }
                            self.state.picker = Some((kind, picker));
                        }
                        Err(error) => self.state.notice = error.to_string(),
                        _ => {}
                    }
                }
            }
            Context::Project => {
                self.config_active = false;
                match result {
                    Ok(Output::Project(project)) => {
                        self.state.project = project.clone();
                        self.state.files.project = project;
                        self.state.files.refresh()?;
                        self.state.changed()?;
                        self.state.request_scan(false)?;
                    }
                    Err(error) => self.state.project_panel.message = error.to_string(),
                    _ => {}
                }
            }
            Context::ProjectSearch => {
                if let Some(dialog) = &mut self.state.search {
                    let result = result.and_then(|output| {
                        if let Output::ProjectSearch(report) = output {
                            Ok(report)
                        } else {
                            Err(DomainError::Unsupported(
                                "wrong project search result".into(),
                            ))
                        }
                    });
                    dialog.complete_search(tag, result)?;
                }
            }
            Context::Clipboard(target) => self.complete_clipboard(tag, target, result)?,
            context => self.complete_editor(owner, tag, context, result)?,
        }
        self.state.drain_ui()
    }
    fn take_view(&mut self, owner: OwnerId) -> Option<(bool, EditorView<QueuedEditorHost>, Rect)> {
        if self
            .state
            .editor
            .as_ref()
            .is_some_and(|view| view.owner() == owner)
        {
            return Some((false, self.state.editor.take()?, self.state.geometry.viewer));
        }
        let dialog = self.state.search.as_mut()?;
        if dialog
            .editor
            .as_ref()
            .is_some_and(|view| view.owner() == owner)
        {
            Some((true, dialog.editor.take()?, dialog.geometry.editor))
        } else {
            None
        }
    }
    fn return_view(&mut self, search: bool, mut view: EditorView<QueuedEditorHost>) {
        let owner = view.owner();
        self.state.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        if search {
            if let Some(dialog) = &mut self.state.search {
                dialog.editor = Some(view);
            }
        } else {
            self.state.editor = Some(view);
        }
    }
    fn complete_editor(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        context: Context,
        result: Result<Output>,
    ) -> Result<()> {
        let Some((search, mut view, area)) = self.take_view(owner) else {
            return Ok(());
        };
        let result = match context {
            Context::Overlays => view
                .complete_overlays(
                    tag,
                    result.and_then(|output| {
                        if let Output::Overlays(report) = output {
                            Ok(report)
                        } else {
                            Err(DomainError::Unsupported("wrong highlight result".into()))
                        }
                    }),
                    &self.state.documents,
                )
                .map(|_| ()),
            Context::Fold => view
                .complete_folds(
                    tag,
                    result.and_then(|output| {
                        if let Output::Fold(report) = output {
                            Ok(report)
                        } else {
                            Err(DomainError::Unsupported("wrong fold result".into()))
                        }
                    }),
                    &self.state.documents,
                    area,
                )
                .map(|_| ()),
            Context::Search => view
                .complete_search(
                    tag,
                    result.and_then(|output| {
                        if let Output::Search(report) = output {
                            Ok(report)
                        } else {
                            Err(DomainError::Unsupported("wrong search result".into()))
                        }
                    }),
                    &mut self.state.documents,
                    area,
                )
                .map(|_| ()),
            Context::Completion => view
                .complete_completion(
                    tag,
                    result.and_then(|output| {
                        if let Output::Completion(items) = output {
                            Ok(items)
                        } else {
                            Err(DomainError::Unsupported("wrong completion result".into()))
                        }
                    }),
                    &self.state.documents,
                    &self.state.focus,
                )
                .map(|_| ()),
            Context::Navigation => {
                let active = self.state.focus.focused == Some(view.input.id);
                view.complete_navigation(
                    tag,
                    result.and_then(|output| {
                        if let Output::Navigation(targets) = output {
                            Ok(targets)
                        } else {
                            Err(DomainError::Unsupported("wrong navigation result".into()))
                        }
                    }),
                    &self.state.documents,
                    active,
                )
                .map(|_| ())
            }
            _ => Ok(()),
        };
        self.return_view(search, view);
        result
    }
    fn complete_clipboard(
        &mut self,
        tag: JobTag,
        target: ClipboardTarget,
        result: Result<Output>,
    ) -> Result<()> {
        let result = result.and_then(|output| {
            if let Output::Clipboard(result) = output {
                Ok(result)
            } else {
                Err(DomainError::Unsupported("wrong clipboard result".into()))
            }
        });
        match target {
            ClipboardTarget::None => {}
            ClipboardTarget::Names => {
                self.state.files.complete_name_clipboard(
                    tag,
                    result.and_then(|result| {
                        if let ClipboardResult::Read(text) = result {
                            Ok(text)
                        } else {
                            Err(DomainError::Unsupported("wrong clipboard read".into()))
                        }
                    }),
                    &self.state.focus,
                );
            }
            ClipboardTarget::Project(id) => {
                if let Some(dialog) = &mut self.state.search
                    && dialog.id == id
                {
                    dialog.complete_clipboard(tag, result, &self.state.documents)?;
                }
            }
            ClipboardTarget::Editor(owner, target) => {
                if let Some((search, mut view, area)) = self.take_view(owner) {
                    let accepted = view.complete_clipboard(
                        &target,
                        result.and_then(|result| {
                            if let ClipboardResult::Read(text) = result {
                                Ok(text)
                            } else {
                                Err(DomainError::Unsupported("wrong clipboard read".into()))
                            }
                        }),
                        &mut self.state.documents,
                        &self.state.focus,
                        area,
                    );
                    self.return_view(search, view);
                    accepted?;
                }
            }
        }
        Ok(())
    }
    pub fn ready_to_leave(&self) -> bool {
        self.state.closing
            && self.state.pending_capture.is_none()
            && self.state.sequence == self.state.captured_sequence
            && !self.state.persistence.is_dirty()
            && !self.state.persistence.is_saving()
            && !self.config_active
            && self.config_queue.is_empty()
            && !self
                .state
                .documents
                .ids()
                .any(|id| self.state.documents.save_in_progress(id))
            && !self.contexts.values().any(|context| {
                matches!(
                    context,
                    Context::File | Context::Persist | Context::Save(_) | Context::Project
                )
            })
            && !self
                .pending
                .iter()
                .any(|pending| pending.delivery == Delivery::Critical)
    }
    pub async fn flush_before_leave(&mut self) -> Result<()> {
        self.state.closing = true;
        loop {
            self.pump()?;
            if self.ready_to_leave() {
                return Ok(());
            }
            if !self.state.closing {
                return Err(DomainError::Unsupported(format!(
                    "workspace remains open: {}",
                    self.state.notice
                )));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    pub async fn quiesce(&mut self) -> Result<()> {
        if !self.ready_to_leave() {
            return Err(DomainError::Unsupported(
                "workspace must be persisted before service disposal".into(),
            ));
        }
        self.dispose_services().await
    }
    async fn dispose_services(&mut self) -> Result<()> {
        self.state.lsp_settings.cancel();
        if let Some(service) = &self.intelligence {
            service.stop_queries();
        }
        if let Some(token) = self.state.scan_token.take() {
            token.cancel();
        }
        self.jobs.cancel_workspace(self.state.generation);
        let jobs = std::mem::replace(&mut self.jobs, Scheduler::new(self.notifications.clone()));
        for completion in jobs.shutdown().await {
            self.complete(completion)?;
        }
        let lsp_cleanup = if let Some(lifecycle) = self
            .lsp
            .as_ref()
            .and_then(|runtime| runtime.lifecycle.clone())
        {
            tokio::task::spawn_blocking(move || lifecycle.stop_all())
                .await
                .map_err(|error| DomainError::Unsupported(format!("LSP shutdown failed: {error}")))
                .and_then(|result| result)
        } else {
            Ok(())
        };
        let intelligence_cleanup = if let Some(service) = self.intelligence.take() {
            service.shutdown().await
        } else {
            Ok(())
        };
        self.query_versions.clear();
        self.overlay_worker = None;
        self.pending.clear();
        self.contexts.clear();
        self.latest.clear();
        if let Some(watcher) = self.state.watcher.take() {
            tokio::task::spawn_blocking(move || drop(watcher))
                .await
                .map_err(|error| {
                    DomainError::Unsupported(format!("watcher cleanup failed: {error}"))
                })?;
        }
        lsp_cleanup?;
        intelligence_cleanup
    }
    /// A broken terminal cannot keep an interactive workspace open. Try the
    /// normal flush, then preserve dirty buffers in distinct recovery files if
    /// the guarded session write was refused, before joining every worker.
    pub async fn terminal_failed(&mut self) -> Result<()> {
        let flushed = self.flush_before_leave().await;
        let recovery = if flushed.is_err() {
            let store = self.state.store.clone();
            let documents: Vec<_> = self
                .state
                .documents
                .ids()
                .filter_map(|id| self.state.documents.document(id).ok())
                .filter(|document| document.is_dirty())
                .cloned()
                .collect();
            tokio::task::spawn_blocking(move || {
                for document in documents {
                    let state = kode_workspace::session::EditorStateV1::capture(
                        &store.root,
                        &document,
                        &Default::default(),
                        0,
                        None,
                    );
                    store.recover_editor(&state)?;
                }
                Ok(())
            })
            .await
            .map_err(|error| {
                DomainError::Unsupported(format!("terminal recovery failed: {error}"))
            })?
        } else {
            Ok(())
        };
        self.state.closing = true;
        let disposed = self.dispose_services().await;
        recovery?;
        disposed?;
        flushed
    }
    pub async fn switch_workspace(&mut self, root: std::path::PathBuf) -> Result<()> {
        self.flush_before_leave().await?;
        self.quiesce().await?;
        let generation =
            kode_core::WorkspaceGeneration(self.state.generation.0.checked_add(1).ok_or_else(
                || DomainError::Unsupported("workspace generation exhausted".into()),
            )?);
        let prepared = tokio::task::spawn_blocking(move || {
            workspace::prepare_workspace(
                &root,
                generation,
                None,
                &kode_platform::jobs::CancellationToken::new(),
            )
        })
        .await
        .map_err(|error| DomainError::Unsupported(format!("workspace loading failed: {error}")))?;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.state.closing = false;
                self.state.switch_requested = None;
                self.state.quit_requested = false;
                let root = self.state.project.root.clone();
                self.state.watcher = Some(
                    tokio::task::spawn_blocking(move || {
                        kode_workspace::watch::TreeWatcher::new(&root)
                    })
                    .await
                    .map_err(|error| {
                        DomainError::Unsupported(format!("watcher restart failed: {error}"))
                    })?,
                );
                if let Some(resources) = self.intelligence_resources.clone() {
                    self.start_intelligence(resources).await?;
                }
                if self.lsp_service().is_some() {
                    self.bind_builtin_lsp()?;
                }
                if self.lsp.is_some() {
                    self.state.lsp_settings.request_refresh();
                }
                return Err(error);
            }
        };
        let mut next = AppState::new(
            prepared,
            self.state.clipboard.clone(),
            self.state.clock.clone(),
        )?;
        next.detector = self.detector.clone();
        let previous = std::mem::replace(&mut self.state, next);
        tokio::task::spawn_blocking(move || drop(previous))
            .await
            .map_err(|error| {
                DomainError::Unsupported(format!("workspace cleanup failed: {error}"))
            })?;
        if let Some(resources) = self.intelligence_resources.clone() {
            self.start_intelligence(resources).await?;
        }
        if self.lsp_service().is_some() {
            self.bind_builtin_lsp()?;
        }
        if self.lsp.is_some() {
            self.state.lsp_settings.request_refresh();
        }
        Ok(())
    }
}
