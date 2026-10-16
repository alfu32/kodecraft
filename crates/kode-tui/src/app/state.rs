//! Application-thread ownership of panels, views, documents and pending work.
use super::workspace::{
    CapturedDocument, OpenRequest, PreparedOpen, PreparedWorkspace, SessionChange,
};
use crate::{
    editor::{
        display::display_text,
        host::{
            ClipboardJob, EditorDocuments, EditorView, EditorWork, HostDescriptor, HostRole,
            QueuedEditorHost,
        },
        input::EditorInputHooks,
    },
    focus::{FocusId, FocusState},
    layout::{Axis, Splitter, split},
    panels::{
        files::{FilesPanel, FilesWork, RecentEntry},
        project::{ProjectPanel, ProjectWork, RescanDialog},
        project_search::{ProjectSearchDialog, ProjectSearchWork},
    },
    theme::Theme,
    widgets::{
        controls::draw_text,
        folder_picker::{FolderPicker, PickerWork},
        tabs::Tabs,
    },
};
use kode_core::{
    DocumentId, DocumentOrigin, JobTag, RequestId, Result, WorkspaceGeneration,
    editor::selection::CaretState,
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock, OwnerId},
};
use kode_workspace::{
    project::Project,
    scanner::ScanCollector,
    session::{
        EditorStateV1, ProjectSessionV1, RecentFileV1,
        persistence::{SessionPersistence, SessionStore, SessionWriteJob},
    },
    tree::{FileAction, TreeJob},
    watch::TreeWatcher,
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Paragraph, Widget, Wrap},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenDestination {
    Main,
    ProjectSearch(FocusId),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Workspace,
    Source,
    Exclusion,
}
#[derive(Debug, Clone)]
pub enum ConfigAction {
    AddSource(PathBuf),
    RemoveSource(PathBuf),
    AddExclusion(PathBuf),
    RemoveExclusion(PathBuf),
}
pub enum AppWork {
    RestFormClipboard {
        tag: JobTag,
        job: ClipboardJob,
        proof: Option<crate::panels::rest::editor::form::PasteProof>,
    },
    Rest(Box<crate::panels::rest::state::Work>),
    RestClipboard {
        tag: JobTag,
        job: ClipboardJob,
        proof: Option<crate::panels::rest::sidebar::PasteProof>,
    },
    SqlCancel {
        tag: JobTag,
        console: String,
        request: RequestId,
        execution: String,
    },
    SqlHighlight {
        tag: JobTag,
        execution: crate::panels::database::console::Highlight,
    },
    SqlControl {
        tag: JobTag,
        source: String,
        action: crate::panels::database::console::Control,
    },
    SqlSave {
        job: crate::editor::host::OwnerSaveJob,
        path: PathBuf,
        expected: Option<kode_core::services::DiskIdentity>,
    },
    SqlClipboard {
        tag: JobTag,
        job: ClipboardJob,
        proof: Option<crate::panels::database::session_dialog::PasteProof>,
    },
    Tree {
        picker: bool,
        job: TreeJob,
    },
    FileAction {
        tag: JobTag,
        action: FileAction,
    },
    Open {
        tag: JobTag,
        request: OpenRequest,
        destination: OpenDestination,
    },
    Editor {
        owner: OwnerId,
        work: EditorWork,
    },
    Project(ProjectWork),
    Git(crate::panels::git::GitWork),
    Database(Box<crate::panels::database::controller::Work>),
    DatabaseClipboard {
        tag: JobTag,
        job: ClipboardJob,
        proof: Option<crate::panels::database::controller::PasteProof>,
    },
    Diff(crate::panels::git::diff::DiffWork),
    LspSettings(crate::panels::settings::lsp::SettingsAction),
    Image(crate::viewers::image::ImageJob),
    Configure {
        tag: JobTag,
        action: ConfigAction,
    },
    ProjectSearch(ProjectSearchWork),
    Clipboard {
        tag: JobTag,
        job: ClipboardJob,
        names: bool,
    },
    Scan {
        tag: JobTag,
        project: Project,
        token: CancellationToken,
    },
    Capture {
        tag: JobTag,
        sequence: u64,
        project: Project,
        base: Arc<ProjectSessionV1>,
        documents: Vec<CapturedDocument>,
        active: Option<DocumentOrigin>,
        changes: Vec<SessionChange>,
        sql: Option<(
            kode_database::console::PersistenceSnapshot,
            HashMap<DocumentId, String>,
        )>,
        rest: Option<Box<crate::panels::rest::state::Capture>>,
    },
    Persist(SessionWriteJob),
    Picker {
        tag: JobTag,
        kind: PickerKind,
    },
}
#[derive(Debug, Default, Clone, Copy)]
pub struct AppGeometry {
    pub status: Rect,
    pub tabs: Rect,
    pub panel: Rect,
    pub viewer: Rect,
    pub body: Rect,
    pub sql_toolbar: Rect,
    pub sql_context: Rect,
    pub sql_results: Rect,
}
#[derive(Clone)]
pub struct StoredView {
    pub caret: CaretState,
    pub scroll_rows: usize,
    pub metadata: Option<Arc<EditorStateV1>>,
}
pub struct AppState {
    pub generation: WorkspaceGeneration,
    pub project: Project,
    pub documents: EditorDocuments,
    pub editor: Option<EditorView<QueuedEditorHost>>,
    pub hexes: HashMap<PathBuf, crate::viewers::hex::HexView>,
    pub active_hex: Option<PathBuf>,
    pub image: Option<crate::viewers::image::ImageView>,
    pub image_settings: crate::viewers::image::ImageSettings,
    pub files: FilesPanel,
    pub project_panel: ProjectPanel,
    pub lsp_settings: crate::panels::settings::lsp::LspSettings,
    pub git: crate::panels::git::GitPanel,
    pub database: crate::panels::database::controller::DatabasePanel,
    pub sql_sources: HashMap<DocumentId, String>,
    pub sql: crate::panels::database::console::SqlConsoles,
    pub rest: crate::panels::rest::state::RestState,
    pub diff: Option<crate::panels::git::diff::DiffViewer>,
    pub tabs: Tabs,
    pub tabs_id: FocusId,
    pub focus: FocusState,
    pub splitter: Splitter,
    pub geometry: AppGeometry,
    pub search: Option<ProjectSearchDialog>,
    pub picker: Option<(PickerKind, FolderPicker)>,
    pub rescan: Option<RescanDialog>,
    pub scan: ScanCollector,
    pub scan_token: Option<CancellationToken>,
    pub persistence: SessionPersistence,
    pub store: SessionStore,
    pub watcher: Option<TreeWatcher>,
    pub notice: String,
    pub intelligence_status: String,
    pub intelligence_revision: u64,
    pub splash: bool,
    pub closing: bool,
    pub window_focused: bool,
    pub quit_requested: bool,
    pub switch_requested: Option<PathBuf>,
    pub clipboard: Arc<ClipboardService>,
    pub clock: Arc<dyn Clock>,
    pub views: HashMap<DocumentId, StoredView>,
    pub sequence: u64,
    pub last_changed: u64,
    pub captured_sequence: u64,
    pub pending_capture: Option<JobTag>,
    pub pending_open: Option<JobTag>,
    pub pending_picker: Option<JobTag>,
    center_editor: bool,
    pub detector: kode_workspace::mime::MimeDetector,
    pub session_changes: Vec<(u64, SessionChange)>,
    pub work: Vec<AppWork>,
}
impl AppState {
    pub fn new(
        prepared: PreparedWorkspace,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let generation = prepared.project.generation;
        let project = prepared.project;
        let files = FilesPanel::new(project.clone())?;
        let focus = FocusState {
            focused: Some(files.id),
            ..Default::default()
        };
        let initial = prepared.initial;
        let rest = crate::panels::rest::state::RestState::new(&prepared.session.session.rest_api)?;
        let notice = prepared.session.warning.clone().unwrap_or_default();
        let mut documents = EditorDocuments::default();
        let git = crate::panels::git::GitPanel::new(
            project.root.clone(),
            generation,
            &mut documents,
            clipboard.clone(),
            clock.clone(),
        )?;
        let mut state = Self {
            generation,
            project,
            documents,
            editor: None,
            hexes: HashMap::new(),
            diff: None,
            active_hex: None,
            image: None,
            image_settings: Default::default(),
            files,
            project_panel: ProjectPanel::new()?,
            lsp_settings: crate::panels::settings::lsp::LspSettings::new()?,
            git,
            database: crate::panels::database::controller::DatabasePanel::new(generation)?,
            sql_sources: HashMap::new(),
            sql: Default::default(),
            rest,
            tabs: Tabs::new(
                vec![
                    "Files".into(),
                    "Project".into(),
                    "Help".into(),
                    "Settings".into(),
                    "Git".into(),
                    "Databases".into(),
                    "REST".into(),
                ],
                Axis::Vertical,
            ),
            tabs_id: FocusId(RequestId::fresh()?.0),
            focus,
            splitter: Splitter::new(Axis::Horizontal, 36),
            geometry: Default::default(),
            search: None,
            picker: None,
            rescan: None,
            scan: ScanCollector::new(generation),
            scan_token: None,
            persistence: SessionPersistence::new(generation, prepared.session),
            store: prepared.store,
            watcher: Some(prepared.watcher),
            notice,
            intelligence_status: String::new(),
            intelligence_revision: 0,
            splash: initial.is_none(),
            closing: false,
            window_focused: true,
            quit_requested: false,
            switch_requested: None,
            clipboard,
            clock,
            views: HashMap::new(),
            sequence: 0,
            last_changed: 0,
            captured_sequence: 0,
            pending_capture: None,
            pending_open: None,
            pending_picker: None,
            center_editor: false,
            detector: kode_workspace::mime::MimeDetector::compiled(),
            session_changes: vec![],
            work: vec![],
        };
        state.update_recents()?;
        state.files.refresh()?;
        state.drain_files()?;
        state.request_scan(false)?;
        if let Some(initial) = initial {
            state.open(initial, OpenDestination::Main)?;
        }
        state.restore_sql()?;
        state.restore_rest()?;
        Ok(state)
    }
    pub fn tag(&self) -> Result<JobTag> {
        Ok(JobTag {
            workspace: self.generation,
            request: RequestId::fresh()?,
            document: None,
        })
    }
    pub fn changed(&mut self) -> Result<()> {
        self.last_changed = self.clock.now_millis();
        self.sequence = self.sequence.checked_add(1).ok_or_else(|| {
            kode_core::DomainError::Unsupported("workspace sequence exhausted".into())
        })?;
        Ok(())
    }
    pub fn take_work(&mut self) -> Vec<AppWork> {
        std::mem::take(&mut self.work)
    }
    pub fn update_recents(&mut self) -> Result<()> {
        self.files.recents = self
            .persistence
            .session()
            .recent_files
            .iter()
            .map(|recent| {
                Ok(RecentEntry {
                    path: kode_workspace::session::resolve_path(&self.project.root, &recent.path)?,
                    dirty: recent.dirty,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for (_, change) in &self.session_changes {
            match change {
                SessionChange::Recent(recent) => {
                    let path =
                        kode_workspace::session::resolve_path(&self.project.root, &recent.path)?;
                    self.files.recents.retain(|entry| entry.path != path);
                    self.files.recents.push(RecentEntry {
                        path,
                        dirty: recent.dirty,
                    });
                }
                SessionChange::RemoveRecent(path) => {
                    self.files.recents.retain(|entry| &entry.path != path)
                }
                SessionChange::Move { from, to } => {
                    for recent in &mut self.files.recents {
                        if let Some(path) = kode_workspace::tree::moved_path(&recent.path, from, to)
                        {
                            recent.path = path;
                        }
                    }
                }
            }
        }
        self.files
            .recents
            .sort_by_key(|recent| recent.path.to_string_lossy().to_lowercase());
        self.files.recents.truncate(20);
        self.files.active_path = self
            .editor
            .as_ref()
            .and_then(|view| self.documents.document(view.document_id()).ok())
            .and_then(|document| match document.origin() {
                DocumentOrigin::File(path) => Some(path.clone()),
                _ => None,
            })
            .or_else(|| self.active_hex.clone())
            .or_else(|| self.image.as_ref().map(|view| view.path.clone()));
        Ok(())
    }
    pub fn viewer_id(&self) -> Option<FocusId> {
        if let Some(form) = &self.rest.form {
            return Some(form.id);
        }
        if let Some(diff) = &self.diff {
            return Some(diff.id());
        }
        self.editor
            .as_ref()
            .map(|view| view.input.id)
            .or_else(|| {
                self.active_hex
                    .as_ref()
                    .and_then(|path| self.hexes.get(path))
                    .map(|view| view.id)
            })
            .or_else(|| self.image.as_ref().map(|view| view.id))
    }
    pub fn clear_diff(&mut self) -> Result<()> {
        self.git.cancel_diff();
        if let Some(mut diff) = self.diff.take() {
            let focused = self.focus.focused == Some(diff.id());
            diff.detach(&mut self.documents, &mut self.focus)?;
            self.work
                .extend(diff.take_work().into_iter().map(AppWork::Diff));
            if focused {
                self.focus.focused = self.viewer_id();
            }
        }
        Ok(())
    }
    pub(super) fn show_diff(&mut self, plan: Arc<kode_git::diff::DiffPlan>) -> Result<()> {
        self.deactivate_rest_form();
        self.rest.pending_open = None;
        self.rest.ready_open = None;
        let root = self
            .git
            .read
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.location.work_tree.clone())
            .unwrap_or_else(|| self.project.root.clone());
        let source_revision = self
            .documents
            .find_origin(&DocumentOrigin::File(root.join(&plan.contents().path)))
            .and_then(|id| {
                self.documents
                    .document(id)
                    .ok()
                    .map(|document| (id, document.revision()))
            });
        if let Some(diff) = &mut self.diff
            && (
                diff.plan.contents().path.as_path(),
                diff.plan.contents().staged,
                &diff.plan.contents().revision,
            ) == (
                plan.contents().path.as_path(),
                plan.contents().staged,
                &plan.contents().revision,
            )
        {
            diff.replace_plan(plan);
            diff.source_revision = source_revision;
            return Ok(());
        }
        self.clear_diff()?;
        if let Some(view) = &mut self.editor {
            view.completion.dismiss();
            view.navigation.dismiss();
        }
        let root = self
            .git
            .read
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.location.work_tree.clone())
            .unwrap_or_else(|| self.project.root.clone());
        let mut diff = crate::panels::git::diff::DiffViewer::new(
            root,
            plan,
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        diff.resize(self.geometry.viewer);
        diff.source_revision = source_revision;
        self.focus.captured = None;
        self.splash = false;
        self.focus.focused = Some(diff.id());
        self.diff = Some(diff);
        Ok(())
    }
    pub(super) fn detach_image(&mut self) {
        if let Some(mut view) = self.image.take() {
            view.detach(&mut self.focus);
            self.image_settings = view.settings;
        }
    }
    pub fn install_image(
        &mut self,
        tag: JobTag,
        opened: super::workspace::PreparedImage,
    ) -> Result<bool> {
        if self.closing || self.pending_open != Some(tag) {
            return Ok(false);
        }
        self.pending_open = None;
        self.cache_main_view()?;
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        if let Some(path) = self.active_hex.take()
            && let Some(view) = self.hexes.get_mut(&path)
        {
            view.detach(&mut self.focus);
        }
        self.detach_image();
        let path = opened.path;
        let view = crate::viewers::image::ImageView::new(
            path.clone(),
            opened.mime,
            opened.image,
            self.generation,
            self.image_settings,
        )?;
        self.focus.focused = Some(view.id);
        self.image = Some(view);
        self.splash = false;
        self.notice.clear();
        self.changed()?;
        self.session_changes.push((
            self.sequence,
            SessionChange::Recent(Box::new(RecentFileV1 {
                path: kode_workspace::session::relative_path(&self.project.root, &path),
                last_opened_epoch_millis: super::workspace::epoch_millis(),
                last_modified_millis: None,
                editor: None,
                dirty: false,
                viewer_type: "IMAGE".into(),
                extra: Default::default(),
            })),
        ));
        self.update_recents()?;
        Ok(true)
    }
    pub fn active_hex_view(&mut self) -> Option<&mut crate::viewers::hex::HexView> {
        self.active_hex
            .as_ref()
            .and_then(|path| self.hexes.get_mut(path))
    }
    pub fn install_hex(
        &mut self,
        tag: JobTag,
        opened: super::workspace::PreparedHex,
    ) -> Result<bool> {
        if self.closing || self.pending_open != Some(tag) {
            return Ok(false);
        }
        self.pending_open = None;
        self.detach_image();
        self.cache_main_view()?;
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        if let Some(path) = &self.active_hex
            && let Some(view) = self.hexes.get_mut(path)
        {
            view.detach(&mut self.focus);
        }
        let path = opened.path;
        let view = match self.hexes.entry(path.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(
                crate::viewers::hex::HexView::new(path.clone(), opened.mime, opened.bytes)?,
            ),
        };
        self.focus.focused = Some(view.id);
        self.active_hex = Some(path.clone());
        self.splash = false;
        self.notice = opened
            .warning
            .map_or_else(String::new, |warning| format!("{warning}; "))
            + "Hex edits remain in memory; binary saving is unavailable";
        self.changed()?;
        self.session_changes.push((
            self.sequence,
            SessionChange::Recent(Box::new(RecentFileV1 {
                path: kode_workspace::session::relative_path(&self.project.root, &path),
                last_opened_epoch_millis: super::workspace::epoch_millis(),
                last_modified_millis: None,
                editor: None,
                dirty: false,
                viewer_type: "HEX".into(),
                extra: Default::default(),
            })),
        ));
        self.update_recents()?;
        Ok(true)
    }
    pub fn reconcile_hex_change(&mut self, change: &kode_workspace::tree::FileChange) {
        if let kode_workspace::tree::FileChange::Renamed { from, to } = change {
            for (path, mut view) in std::mem::take(&mut self.hexes) {
                let path = kode_workspace::tree::moved_path(&path, from, to).unwrap_or(path);
                view.path = path.clone();
                self.hexes.insert(path, view);
            }
            if let Some(view) = &mut self.image
                && let Some(path) = kode_workspace::tree::moved_path(&view.path, from, to)
            {
                view.path = path;
            }
            self.active_hex = self.active_hex.as_ref().map(|path| {
                kode_workspace::tree::moved_path(path, from, to).unwrap_or_else(|| path.clone())
            });
        }
    }
    pub fn open(&mut self, request: OpenRequest, destination: OpenDestination) -> Result<()> {
        self.open_impl(request, destination, true)
    }
    pub(super) fn open_background(&mut self, request: OpenRequest) -> Result<()> {
        self.open_impl(request, OpenDestination::Main, false)
    }
    fn open_impl(
        &mut self,
        request: OpenRequest,
        destination: OpenDestination,
        clear: bool,
    ) -> Result<()> {
        if destination == OpenDestination::Main {
            self.deactivate_rest_form();
            self.rest.pending_open = None;
            self.rest.ready_open = None;
        }
        let tag = self.tag()?;
        if destination == OpenDestination::Main {
            if clear {
                self.clear_diff()?;
            }
            self.pending_open = Some(tag);
        }
        self.work.push(AppWork::Open {
            tag,
            request,
            destination,
        });
        Ok(())
    }
    pub fn request_scan(&mut self, explicit: bool) -> Result<()> {
        if let Some(token) = self.scan_token.take() {
            token.cancel();
        }
        let tag = self.scan.begin()?;
        let token = CancellationToken::new();
        self.scan_token = Some(token.clone());
        if explicit {
            self.rescan = Some(RescanDialog::new(tag, token.clone()));
        }
        self.work.push(AppWork::Scan {
            tag,
            project: self.project.clone(),
            token,
        });
        Ok(())
    }
    pub fn cache_main_view(&mut self) -> Result<()> {
        if let Some(view) = &self.editor {
            let id = view.document_id();
            let metadata = self.views.get(&id).and_then(|state| state.metadata.clone());
            self.views.insert(
                id,
                StoredView {
                    caret: view.input.state.caret.clone(),
                    scroll_rows: view.input.state.scroll_rows,
                    metadata,
                },
            );
        }
        Ok(())
    }
    pub fn install_main(&mut self, tag: JobTag, opened: PreparedOpen) -> Result<bool> {
        if self.closing || self.pending_open != Some(tag) {
            return Ok(false);
        }
        self.pending_open = None;
        self.detach_image();
        if let Some(path) = self.active_hex.take()
            && let Some(view) = self.hexes.get_mut(&path)
        {
            view.detach(&mut self.focus);
        }
        self.notice.clear();
        self.cache_main_view()?;
        let origin = opened.prepared.document.origin().clone();
        let mut descriptor = HostDescriptor::new(
            HostRole::File,
            "workspace".into(),
            origin_key(&origin),
            origin_key(&origin),
            self.project.root.clone(),
        );
        descriptor.language = opened.classification.detected.language.clone();
        descriptor.mime = Some(opened.classification.detected.mime.clone());
        let id = match self.documents.find_origin(&origin) {
            Some(id) => id,
            None => self
                .documents
                .insert_file(opened.prepared, &descriptor, self.generation)?,
        };
        let mut view = EditorView::new(
            id,
            &self.documents,
            QueuedEditorHost::new(descriptor),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        if let Some(stored) = self.views.get(&id) {
            view.input.state.caret = stored.caret.clone();
            view.input.state.scroll_rows = stored.scroll_rows;
        } else {
            view.input.state.caret = opened.caret;
            view.input.state.scroll_rows = opened.scroll_rows;
        }
        if let Some(target) = opened.navigation {
            let snapshot = self.documents.document(id)?.snapshot();
            let position = target.location.map_or_else(
                || snapshot.clamp_byte(target.identifier_range.start),
                |position| snapshot.utf16_to_byte_clamped(position),
            );
            view.input.state.caret.move_to(&snapshot, position, false)?;
            view.visuals.reveal_caret(&snapshot, position)?;
            self.center_editor = true;
        }
        let metadata = opened.metadata;
        self.views.entry(id).or_insert(StoredView {
            caret: view.input.state.caret.clone(),
            scroll_rows: view.input.state.scroll_rows,
            metadata,
        });
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        self.focus.focused = Some(view.input.id);
        self.splash = false;
        if let Some(path) = opened.recovery {
            self.notice = format!(
                "Recovered previous dirty state: {}",
                display_text(&path.to_string_lossy())
            );
        } else if let Some(warning) = opened.warning {
            self.notice = warning;
        }
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.editor = Some(view);
        self.changed()?;
        if let DocumentOrigin::File(path) = &origin {
            self.session_changes.push((
                self.sequence,
                SessionChange::Recent(Box::new(RecentFileV1 {
                    path: kode_workspace::session::relative_path(&self.project.root, path),
                    last_opened_epoch_millis: super::workspace::epoch_millis(),
                    last_modified_millis: None,
                    editor: None,
                    dirty: false,
                    viewer_type: "CODE".into(),
                    extra: Default::default(),
                })),
            ));
        }
        self.update_recents()?;
        Ok(true)
    }
    pub fn request_capture(&mut self) -> Result<()> {
        if !self.closing && self.clock.now_millis().saturating_sub(self.last_changed) < 3_000 {
            return Ok(());
        }
        if self.pending_capture.is_some() || self.sequence == self.captured_sequence {
            return Ok(());
        }
        self.cache_main_view()?;
        let mut views = self.views.clone();
        if let Some(dialog) = &self.search
            && let Some(view) = &dialog.editor
        {
            let id = view.document_id();
            let metadata = views.get(&id).and_then(|state| state.metadata.clone());
            views.insert(
                id,
                StoredView {
                    caret: view.input.state.caret.clone(),
                    scroll_rows: view.input.state.scroll_rows,
                    metadata,
                },
            );
        }
        let documents = self
            .documents
            .ids()
            .filter_map(|id| {
                self.documents.document(id).ok().map(|document| {
                    let view = views.get(&id);
                    CapturedDocument {
                        document: document.clone(),
                        caret: view.map_or_else(CaretState::default, |view| view.caret.clone()),
                        scroll_rows: view.map_or(0, |view| view.scroll_rows),
                        disk_identity: self.documents.disk_identity(id).cloned(),
                        metadata: view.and_then(|view| view.metadata.clone()),
                    }
                })
            })
            .collect();
        let active = self
            .rest
            .form
            .as_ref()
            .map(|form| DocumentOrigin::Virtual {
                owner: "rest-node".into(),
                key: form.path.encode(),
                label: "REST request".into(),
            })
            .or_else(|| {
                self.editor
                    .as_ref()
                    .and_then(|view| self.documents.document(view.document_id()).ok())
                    .map(|document| document.origin().clone())
                    .or_else(|| self.active_hex.clone().map(DocumentOrigin::File))
                    .or_else(|| {
                        self.image
                            .as_ref()
                            .map(|view| DocumentOrigin::File(view.path.clone()))
                    })
            });
        let tag = self.tag()?;
        self.pending_capture = Some(tag);
        self.work.push(AppWork::Capture {
            tag,
            sequence: self.sequence,
            project: self.project.clone(),
            base: self.persistence.snapshot(),
            documents,
            active,
            changes: self
                .session_changes
                .iter()
                .map(|(_, change)| change.clone())
                .collect(),
            sql: self
                .sql
                .persistence_enabled
                .then(|| (self.sql.manager.snapshot(), self.sql.documents.clone())),
            rest: self.rest.capture().map(Box::new),
        });
        Ok(())
    }
    pub fn complete_capture(
        &mut self,
        tag: JobTag,
        sequence: u64,
        result: Result<ProjectSessionV1>,
    ) -> Result<bool> {
        if self.pending_capture != Some(tag) {
            return Ok(false);
        }
        self.pending_capture = None;
        match result {
            Ok(session) => {
                self.captured_sequence = sequence;
                self.session_changes
                    .retain(|(changed, _)| *changed > sequence);
                self.persistence.changed(session, self.last_changed)?;
                self.update_recents()?;
            }
            Err(error) => self.notice = error.to_string(),
        }
        Ok(true)
    }
    pub fn drain_files(&mut self) -> Result<()> {
        for work in self.files.take_work() {
            match work {
                FilesWork::Refresh(job) => self.work.push(AppWork::Tree { picker: false, job }),
                FilesWork::Action { tag, action } => {
                    self.work.push(AppWork::FileAction { tag, action })
                }
                FilesWork::Open(path) => {
                    self.open(
                        OpenRequest {
                            path,
                            saved: None,
                            navigation: None,
                        },
                        OpenDestination::Main,
                    )?;
                }
                FilesWork::RemoveRecent(path) => {
                    self.changed()?;
                    self.session_changes
                        .push((self.sequence, SessionChange::RemoveRecent(path)));
                    self.update_recents()?;
                }
                FilesWork::ChangeWorkspace => {
                    let tag = self.tag()?;
                    self.pending_picker = Some(tag);
                    self.work.push(AppWork::Picker {
                        tag,
                        kind: PickerKind::Workspace,
                    });
                }
                FilesWork::ClipboardCopy(text) => {
                    let epoch = self.clipboard.remember(&text)?;
                    let tag = self.tag()?;
                    self.work.push(AppWork::Clipboard {
                        tag,
                        job: ClipboardJob::Publish(epoch),
                        names: false,
                    });
                }
                FilesWork::ClipboardRead { tag } => self.work.push(AppWork::Clipboard {
                    tag,
                    job: ClipboardJob::Read,
                    names: true,
                }),
            }
        }
        Ok(())
    }
    pub fn drain_ui(&mut self) -> Result<()> {
        self.drain_rest()?;
        self.database.drain_tree(&mut self.focus)?;
        for intent in self.database.take_intents() {
            use crate::panels::database::controller::Intent;
            use crate::widgets::field::FieldAction;
            match intent {
                Intent::Work(work) => self.work.push(AppWork::Database(work)),
                Intent::Console { source, sql } => self.open_database_console(source, sql)?,
                Intent::OpenSession(id) => {
                    if let Some(document) = self.sql_document(&id) {
                        self.open_database_session(document)?;
                    }
                }
                Intent::RemoveSession(id) => {
                    if let Some(document) = self.sql_document(&id) {
                        self.remove_database_session(document)?;
                    }
                }
                Intent::Clipboard { action, proof } => {
                    let tag = self.tag()?;
                    let job = match action {
                        FieldAction::Copy(text) => {
                            ClipboardJob::Publish(self.clipboard.remember(&text)?)
                        }
                        FieldAction::PasteRequested => ClipboardJob::Read,
                        _ => continue,
                    };
                    self.work
                        .push(AppWork::DatabaseClipboard { tag, job, proof });
                }
            }
        }
        if let Some(diff) = &mut self.diff {
            self.work
                .extend(diff.take_work().into_iter().map(AppWork::Diff));
        }
        self.work
            .extend(self.git.take_work().into_iter().map(AppWork::Git));
        self.work.extend(
            self.lsp_settings
                .take_work()
                .into_iter()
                .map(AppWork::LspSettings),
        );
        if let Some(view) = &mut self.image {
            self.work
                .extend(view.take_work().into_iter().map(AppWork::Image));
        }
        self.drain_files()?;
        self.work.extend(
            self.project_panel
                .take_work()
                .into_iter()
                .map(AppWork::Project),
        );
        if let Some(dialog) = &mut self.search {
            self.work
                .extend(dialog.take_work().into_iter().map(AppWork::ProjectSearch));
        }
        if let Some((kind, picker)) = &mut self.picker {
            for work in picker.take_work() {
                match work {
                    PickerWork::Refresh(job) => self.work.push(AppWork::Tree { picker: true, job }),
                    PickerWork::Confirmed(path) if *kind == PickerKind::Workspace => {
                        self.switch_requested = Some(path)
                    }
                    PickerWork::Confirmed(path) => {
                        let tag = JobTag {
                            workspace: self.generation,
                            request: RequestId::fresh()?,
                            document: None,
                        };
                        let action = if *kind == PickerKind::Source {
                            ConfigAction::AddSource(path)
                        } else {
                            ConfigAction::AddExclusion(path)
                        };
                        self.work.push(AppWork::Configure { tag, action });
                    }
                    PickerWork::Cancelled => {}
                }
            }
        }
        if self
            .picker
            .as_ref()
            .is_some_and(|(_, picker)| picker.finished)
        {
            self.picker = None;
        }
        if let Some(view) = &mut self.editor {
            let owner = view.owner();
            let work = view.take_work();
            let _ = view.host.take_events();
            for work in work {
                match work {
                    EditorWork::Execute(execution)
                        if execution.action
                            == crate::editor::host::ExecutionAction::SendRequest =>
                    {
                        self.execute_rest_snapshot(execution)?
                    }
                    work => self.work.push(AppWork::Editor { owner, work }),
                }
            }
        }
        Ok(())
    }
    pub fn layout(&self, area: Rect) -> AppGeometry {
        let status = Rect::new(
            area.x,
            area.bottom().saturating_sub(1),
            area.width,
            area.height.min(1),
        );
        let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let panes = split(body, Axis::Horizontal, self.splitter.size);
        let tabs_width = panes.first.width.min(9);
        let tabs = Rect::new(panes.first.x, panes.first.y, tabs_width, panes.first.height);
        let panel = Rect::new(
            tabs.right(),
            panes.first.y,
            panes.first.width.saturating_sub(tabs_width),
            panes.first.height,
        );
        let mut geometry = AppGeometry {
            status,
            tabs,
            panel,
            viewer: panes.second,
            body,
            ..Default::default()
        };
        if self.active_sql().is_some() {
            let outer = geometry.viewer;
            let columns = (outer.width / 11).clamp(1, 6);
            let toolbar_height = 6_u16.div_ceil(columns).min(outer.height.saturating_sub(4));
            geometry.sql_toolbar = Rect::new(outer.x, outer.y, outer.width, toolbar_height);
            let context_height = u16::from(outer.height > toolbar_height + 4);
            geometry.sql_context = Rect::new(
                outer.x,
                outer.y + toolbar_height,
                outer.width,
                context_height,
            );
            let available = outer.height.saturating_sub(toolbar_height + context_height);
            let results_height = if self
                .active_sql()
                .is_some_and(|id| self.sql.results.contains_key(id))
                && available >= 10
            {
                (available / 3).max(6)
            } else {
                0
            };
            geometry.sql_results = Rect::new(
                outer.x,
                outer.bottom() - results_height,
                outer.width,
                results_height,
            );
            geometry.viewer = Rect::new(
                outer.x,
                outer.y + toolbar_height + context_height,
                outer.width,
                available - results_height,
            );
        }
        geometry
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Result<Option<Position>> {
        let area = area.intersection(buffer.area);
        self.geometry = self.layout(area);
        let geometry = self.geometry;
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("content"));
        self.tabs.render(
            geometry.tabs,
            buffer,
            theme,
            self.focus.focused == Some(self.tabs_id),
        );
        let mut cursor = None;
        match self.tabs.selected {
            0 => cursor = self.files.render(geometry.panel, buffer, theme),
            1 => self
                .project_panel
                .render(geometry.panel, buffer, theme, &self.project),
            3 => self.lsp_settings.render(
                geometry.panel,
                buffer,
                theme,
                self.focus.focused == Some(self.lsp_settings.id),
            ),
            4 => {
                cursor =
                    self.git
                        .render(geometry.panel, buffer, theme, &self.documents, &self.focus)?
            }
            5 => self.database.render(
                geometry.panel,
                buffer,
                theme,
                self.focus.focused == Some(self.database.tree.id),
            ),
            6 => {
                if self.rest.persistence_enabled {
                    cursor = self.rest.sidebar.render(
                        geometry.panel,
                        buffer,
                        theme,
                        &self.rest.model,
                        self.focus.focused == Some(self.rest.sidebar.id),
                    )
                } else {
                    Paragraph::new(format!(
                        "REST unavailable: {}",
                        self.rest
                            .warning
                            .as_deref()
                            .unwrap_or("Invalid saved state")
                    ))
                    .wrap(Wrap { trim: false })
                    .render(geometry.panel, buffer);
                }
            }
            _ => {
                Paragraph::new("Files: open, rename, create, delete.\nProject: source roots and exclusions.\n\nCtrl+S save\nCtrl+F find/replace\nCtrl+Shift+F project search\nCtrl+Space completion\nF6 switch pane\nCtrl+Q quit\n\nGit: Space stage/unstage, a/A stage/unstage all, Tab message, Ctrl+Enter commit, Ctrl+Tab sections. b branches, t tag, T create/move tag, Enter expand history, m copy message.\n\nImage: Left/Right width, Enter mode, M advanced controls.\n\nHex: Shift+arrows select, Alt+C/X/V local bytes, Delete/Backspace edit memory. Binary saving unavailable.\n\nDatabases: Alt+6; n new, F2 edit, c connect/disconnect, r refresh, R restart service, s SQL console.\nConnection dialog: Alt+T test, Alt+D driver download, Ctrl+S save.\n\nREST: Alt+7; / filter, n request, f folder, m actions, i import, e export, r run/stop. Tab selects environments; n new, c copy, F2 rename, Space activate.\n\nUse mouse or Alt+1/2/3/4/5/6/7 to select Files/Project/Help/Settings/Git/Databases/REST.").wrap(Wrap { trim: false }).render(geometry.panel, buffer);
            }
        }
        self.splitter.render(geometry.body, buffer, theme);
        if let Some(diff) = &mut self.diff {
            let caret =
                diff.render(geometry.viewer, &self.documents, &self.focus, buffer, theme)?;
            if caret.is_some() {
                cursor = caret;
            }
        } else if self.rest.form.is_some() {
            if let Some(caret) = self.render_rest_editor(geometry.viewer, buffer, theme)? {
                cursor = Some(caret);
            }
        } else if let Some(view) = &mut self.editor {
            view.synchronize(&self.documents)?;
            if self.center_editor && !geometry.viewer.is_empty() {
                let document = self.documents.document(view.document_id())?;
                let snapshot = document.snapshot();
                let editor_geometry = view.geometry(&self.documents, geometry.viewer)?;
                let folds = view.visuals.folds.collapsed(&snapshot);
                let layout = view.input.layout(
                    document,
                    crate::editor::input::InputViewport {
                        geometry: editor_geometry,
                        folds: &folds,
                    },
                )?;
                let (row, _) = layout.position_to_cell(view.input.state.caret.cursor())?;
                view.input.state.scroll_rows = row
                    .saturating_sub(editor_geometry.text.height as usize / 2)
                    .min(
                        layout
                            .rows
                            .len()
                            .saturating_sub(editor_geometry.text.height as usize),
                    );
                self.center_editor = false;
            }
            let caret = view.render(
                &self.documents,
                &self.focus,
                geometry.viewer,
                buffer,
                theme,
                &[],
            )?;
            if caret.is_some() {
                cursor = caret;
            }
        } else if let Some(view) = &mut self.image {
            view.render(geometry.viewer, buffer, theme)?;
        } else if let Some(path) = &self.active_hex
            && let Some(view) = self.hexes.get_mut(path)
        {
            view.render(
                geometry.viewer,
                buffer,
                theme,
                self.focus.focused == Some(view.id),
            );
        } else {
            Paragraph::new(if self.splash { "Kode\n\nSelect a file to begin editing.\nSource scanning runs in the background.\nPress Enter to dismiss this screen." } else { "Select a file from the project." }).wrap(Wrap { trim: false }).render(geometry.viewer, buffer);
        }
        self.render_sql(buffer, theme)?;
        let status = if self.closing {
            "Saving workspace…".into()
        } else if self.active_hex.is_some() {
            format!(
                "Hex: edits in memory; no binary save | {}",
                display_text(&self.notice)
            )
        } else if !self.notice.is_empty() {
            display_text(&self.notice)
        } else {
            format!("{} files scanned | Ctrl+Q quit", self.scan.files.len())
        };
        let status = if self.intelligence_status.is_empty() {
            status
        } else {
            format!("{status} | {}", display_text(&self.intelligence_status))
        };
        draw_text(geometry.status, buffer, &status, theme.style("content"));
        if let Some(dialog) = &mut self.search {
            cursor = dialog.render(area, buffer, theme, &self.documents, &self.focus)?;
        }
        if let Some((_, picker)) = &mut self.picker {
            picker.render(area, buffer, theme);
            cursor = None;
        }
        if let Some(dialog) = &mut self.rescan {
            dialog.render(area, buffer, theme);
            cursor = None;
        }
        if self.git.has_modal() {
            cursor = self.git.render_modal(area, buffer, theme);
        }
        if self.database.has_modal() {
            cursor = self.database.render_modal(area, buffer, theme);
        }
        if let Some(dialog) = &mut self.sql.dialog {
            cursor = dialog.dialog.render(area, buffer, theme);
        }
        if let Some(menu) = &mut self.sql.menu {
            menu.render(area, buffer, theme);
            cursor = None;
        }
        if let Some(form) = &mut self.rest.form
            && form.has_modal()
        {
            cursor = form.render_modal(area, buffer, theme);
        }
        if self.rest.sidebar.has_modal() {
            cursor = self.rest.sidebar.render_modal(area, buffer, theme);
        }
        Ok(cursor)
    }
    pub fn open_database_console(&mut self, source: String, sql: String) -> Result<()> {
        if self
            .database
            .tree
            .sources
            .iter()
            .any(|definition| definition.id == source)
        {
            self.deactivate_rest_form();
            self.rest.pending_open = None;
            self.rest.ready_open = None;
            self.pending_open = None;
        }
        if !sql.is_empty()
            && let Some(view) = &mut self.editor
            && self.sql_sources.get(&view.document_id()) == Some(&source)
        {
            self.focus.focused = Some(view.input.id);
            view.dispatch(
                &crate::input::terminal::InputEvent::Paste(sql),
                &mut self.documents,
                &mut self.focus,
                self.geometry.viewer,
            )?;
            return self.changed();
        }
        self.cache_main_view()?;
        self.clear_diff()?;
        self.detach_image();
        self.active_hex = None;
        let mut definition = self
            .database
            .tree
            .sources
            .iter()
            .find(|ds| ds.id == source)
            .cloned()
            .ok_or_else(|| kode_core::DomainError::MissingResource("SQL datasource".into()))?;
        if let Some(session) = self
            .sql
            .connections
            .get(&source)
            .and_then(|snapshot| snapshot.session.as_ref())
        {
            definition.auto_commit = session.auto_commit;
        }
        let console = self.sql.manager.create(&definition)?;
        let key = console.state.id.clone();
        let text = if sql.is_empty() {
            console.state.buffer.clone()
        } else {
            format!("{}{}\n", console.state.buffer, sql.trim_end())
        };
        let descriptor =
            super::sql::descriptor(&self.project.root, key.clone(), console.state.title.clone());
        let id = self
            .documents
            .create_virtual(&descriptor, self.generation, &text)?;
        let mut view = EditorView::new(
            id,
            &self.documents,
            QueuedEditorHost::new(descriptor),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        view.input.state.caret.move_to(
            &self.documents.document(id)?.snapshot(),
            kode_core::ByteOffset(text.len()),
            false,
        )?;
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        self.focus.focused = Some(view.input.id);
        self.splash = false;
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.sql_sources.insert(id, source);
        self.sql.documents.insert(id, key);
        self.editor = Some(view);
        self.sync_database_sessions();
        self.changed()
    }
    pub fn sync_database_sessions(&mut self) {
        let sessions = self
            .sql
            .manager
            .all()
            .map(|console| crate::panels::database::tree::Session {
                id: console.state.id.clone(),
                source: console.state.data_source_id.clone(),
                title: console.state.title.clone(),
            })
            .collect();
        self.database.tree.set_sessions(sessions);
    }
    pub fn open_database_session(&mut self, id: DocumentId) -> Result<()> {
        if !self.sql_sources.contains_key(&id) {
            return Ok(());
        }
        self.deactivate_rest_form();
        self.rest.pending_open = None;
        self.rest.ready_open = None;
        self.cache_main_view()?;
        self.clear_diff()?;
        self.detach_image();
        self.active_hex = None;
        self.attach_sql_document(id)?;
        self.changed()
    }
    pub fn remove_database_session(&mut self, id: DocumentId) -> Result<()> {
        if !self.sql_sources.contains_key(&id) {
            return Ok(());
        }
        if self.documents.save_in_progress(id) {
            return Err(kode_core::DomainError::Unsupported(
                "Wait for this SQL session to finish saving".into(),
            ));
        }
        if let Some(key) = self.sql.documents.get(&id) {
            self.sql.manager.remove(key)?;
            self.sql.results.remove(key);
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|view| view.document_id() == id)
            && let Some(mut old) = self.editor.take()
        {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
            self.focus.focused = Some(self.database.tree.id);
        }
        self.documents.remove(id)?;
        self.sql_sources.remove(&id);
        self.sql.documents.remove(&id);
        self.views.remove(&id);
        self.sync_database_sessions();
        self.changed()
    }
}
fn origin_key(origin: &DocumentOrigin) -> String {
    match origin {
        DocumentOrigin::File(path) => path.to_string_lossy().into_owned(),
        DocumentOrigin::Virtual { key, .. } => key.clone(),
    }
}
