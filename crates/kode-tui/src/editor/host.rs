//! Reusable document/view ownership and nonblocking host contracts.
use super::{
    folding::{EditorVisuals, FoldReport},
    input::{
        EditorEffect, EditorInput, EditorInputHooks, EditorState, HookContext, InputTarget,
        InputViewport, PasteTicket,
    },
    loading::{PreparedDocument, SaveController, SaveJob},
    minimap::Minimap,
    overlays::{OverlayCache, OverlayJob, OverlayReport},
    popups::{
        completion::{CompletionJob, CompletionPopup, HostCompletionProvider},
        definition::{NavigationController, NavigationEffect, NavigationJob, NavigationResult},
    },
    render::{EditorGeometry, EditorHeader, OverlaySpan, RenderOptions, render_editor},
    search_bar::{FieldPasteTicket, SearchBar, SearchEffect},
};
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key},
    theme::Theme,
};
use kode_core::services::Clipboard;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, RequestId, Result,
    Revision, Selection, TextRange, WorkspaceGeneration,
    commands::{EditKind, EditorCommand},
    document::{Document, EditDelta, TextSnapshot},
    editor::search::SearchReport,
    intelligence::{CompletionItem, IntelligenceProvider, NavigationTarget},
    services::DiskIdentity,
};
use kode_platform::{clipboard::ClipboardService, jobs::Clock};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRole {
    File,
    CommitMessage,
    ProjectSearch,
    Sql,
    RestBody,
    RestHeaders,
    Environment,
    Response,
    RawRequest,
    Historical,
}
impl HostRole {
    fn requires_read_only(self) -> bool {
        matches!(self, Self::Response | Self::RawRequest | Self::Historical)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    File,
    Owner(String),
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionAction {
    SqlSelectionOrStatement,
    SqlScript,
    SendRequest,
}
#[derive(Debug, Clone)]
pub struct HostDescriptor {
    pub owner: String,
    pub key: String,
    pub label: String,
    pub role: HostRole,
    pub read_only: bool,
    pub save_target: SaveTarget,
    pub language: Option<String>,
    pub mime: Option<String>,
    pub grammar: String,
    pub execute: Option<ExecutionAction>,
    pub auto_show_external: bool,
    pub project_root: PathBuf,
}
impl HostDescriptor {
    pub fn new(
        role: HostRole,
        owner: String,
        key: String,
        label: String,
        project_root: PathBuf,
    ) -> Self {
        Self {
            owner,
            key,
            label,
            role,
            read_only: role.requires_read_only(),
            save_target: if matches!(role, HostRole::File | HostRole::ProjectSearch) {
                SaveTarget::File
            } else {
                SaveTarget::None
            },
            language: None,
            mime: None,
            grammar: String::new(),
            execute: match role {
                HostRole::Sql => Some(ExecutionAction::SqlSelectionOrStatement),
                HostRole::RestBody | HostRole::RestHeaders => Some(ExecutionAction::SendRequest),
                _ => None,
            },
            auto_show_external: false,
            project_root,
        }
    }
    pub fn is_read_only(&self) -> bool {
        self.read_only || self.role.requires_read_only()
    }
}
#[derive(Debug, Clone)]
pub struct HostChange {
    pub snapshot: TextSnapshot,
    pub deltas: Option<Vec<EditDelta>>,
}
#[derive(Debug, Clone)]
pub struct HostExecution {
    pub action: ExecutionAction,
    pub snapshot: TextSnapshot,
    pub selection: Selection,
}
#[derive(Debug, Clone, Copy)]
pub struct TransientHighlight {
    pub revision: Revision,
    pub range: TextRange,
}

/// Callbacks are main-thread reducers: update memory or enqueue effects only.
/// The owning feature executes disk/network/database work outside this interface.
pub trait EditorHost {
    fn descriptor(&self) -> &HostDescriptor;
    fn completion_provider(&self) -> Option<Arc<dyn HostCompletionProvider>>;
    fn intelligence_provider(&self) -> Option<Arc<dyn IntelligenceProvider>>;
    fn on_change(&mut self, change: HostChange);
    fn on_navigate(&mut self, target: NavigationTarget);
    fn on_execute(&mut self, execution: HostExecution);
    fn warning_lines(&self) -> &[LineIndex];
    fn transient_highlight(&self) -> Option<TransientHighlight>;
}
#[derive(Debug, Clone)]
pub enum HostEvent {
    Changed(HostChange),
    Navigate(NavigationTarget),
    Execute(HostExecution),
}
/// Useful production adapter when a feature reducer consumes queued host events.
pub struct QueuedEditorHost {
    pub descriptor: HostDescriptor,
    pub external: Option<Arc<dyn HostCompletionProvider>>,
    pub intelligence: Option<Arc<dyn IntelligenceProvider>>,
    pub warnings: Vec<LineIndex>,
    pub transient: Option<TransientHighlight>,
    events: Vec<HostEvent>,
}
impl QueuedEditorHost {
    pub fn new(descriptor: HostDescriptor) -> Self {
        Self {
            descriptor,
            external: None,
            intelligence: None,
            warnings: vec![],
            transient: None,
            events: vec![],
        }
    }
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        std::mem::take(&mut self.events)
    }
}
impl EditorHost for QueuedEditorHost {
    fn descriptor(&self) -> &HostDescriptor {
        &self.descriptor
    }
    fn completion_provider(&self) -> Option<Arc<dyn HostCompletionProvider>> {
        self.external.clone()
    }
    fn intelligence_provider(&self) -> Option<Arc<dyn IntelligenceProvider>> {
        self.intelligence.clone()
    }
    fn on_change(&mut self, change: HostChange) {
        if let Some(HostEvent::Changed(previous)) = self.events.last_mut()
            && previous.snapshot.id == change.snapshot.id
        {
            *previous = HostChange {
                snapshot: change.snapshot,
                deltas: None,
            };
            return;
        }
        self.events.push(HostEvent::Changed(change));
    }
    fn on_navigate(&mut self, target: NavigationTarget) {
        self.events.push(HostEvent::Navigate(target));
    }
    fn on_execute(&mut self, execution: HostExecution) {
        self.events.push(HostEvent::Execute(execution));
    }
    fn warning_lines(&self) -> &[LineIndex] {
        &self.warnings
    }
    fn transient_highlight(&self) -> Option<TransientHighlight> {
        self.transient
    }
}

#[derive(Debug, Clone)]
pub struct OwnerSaveJob {
    pub key: String,
    pub tag: JobTag,
    pub snapshot: TextSnapshot,
}
enum DocumentSave {
    File(SaveController),
    Owner {
        key: String,
        active: Option<OwnerSaveJob>,
        queued: Option<TextSnapshot>,
        error: Option<DomainError>,
    },
    None,
}
struct DocumentEntry {
    document: Document,
    save: DocumentSave,
    save_owner: kode_platform::jobs::OwnerId,
}
#[derive(Default)]
pub struct EditorDocuments {
    entries: HashMap<DocumentId, DocumentEntry>,
    file_mutations: HashMap<JobTag, Vec<std::path::PathBuf>>,
}
impl EditorDocuments {
    pub fn save_owner(&self, id: DocumentId) -> Result<kode_platform::jobs::OwnerId> {
        self.entries
            .get(&id)
            .map(|entry| entry.save_owner)
            .ok_or_else(|| DomainError::MissingResource(format!("document {}", id.0)))
    }
    pub fn document(&self, id: DocumentId) -> Result<&Document> {
        self.entries
            .get(&id)
            .map(|entry| &entry.document)
            .ok_or_else(|| DomainError::MissingResource(format!("document {}", id.0)))
    }
    fn entry_mut(&mut self, id: DocumentId) -> Result<&mut DocumentEntry> {
        self.entries
            .get_mut(&id)
            .ok_or_else(|| DomainError::MissingResource(format!("document {}", id.0)))
    }
    pub fn ids(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.entries.keys().copied()
    }
    /// Run before scheduling a tree mutation; the application keeps saves for
    /// affected paths serialized until that mutation has completed.
    pub fn check_file_action(&self, path: &std::path::Path) -> Result<()> {
        if self.entries.values().any(|entry| matches!(entry.document.origin(), DocumentOrigin::File(origin) if origin.starts_with(path)) && self.save_in_progress(entry.document.id())) {
            return Err(DomainError::Unsupported("wait for the active save before changing this path".into()));
        }
        Ok(())
    }
    /// Reserve affected paths until the critical action completion arrives.
    /// Editing remains possible; a new physical save waits for reconciliation.
    pub fn reserve_file_action(
        &mut self,
        tag: JobTag,
        paths: Vec<std::path::PathBuf>,
    ) -> Result<()> {
        for path in &paths {
            self.check_file_action(path)?;
            if self
                .file_mutations
                .values()
                .flatten()
                .any(|other| path.starts_with(other) || other.starts_with(path))
            {
                return Err(DomainError::Unsupported(
                    "a file operation is already changing this path".into(),
                ));
            }
        }
        if self.file_mutations.contains_key(&tag) {
            return Err(DomainError::Unsupported("duplicate file operation".into()));
        }
        self.file_mutations.insert(tag, paths);
        Ok(())
    }
    pub fn finish_file_action(
        &mut self,
        tag: JobTag,
        change: Option<&kode_workspace::tree::FileChange>,
    ) -> Result<Vec<DocumentId>> {
        if !self.file_mutations.contains_key(&tag) {
            return Ok(vec![]);
        }
        let result = if let Some(kode_workspace::tree::FileChange::Renamed { from, to }) = change {
            self.reconcile_file_move(from, to)
        } else {
            Ok(vec![])
        };
        self.file_mutations.remove(&tag);
        result
    }
    /// Apply only a successful filesystem move. Deleted files retain their dirty
    /// documents; the existing disk conflict guard prevents silent recreation.
    pub fn reconcile_file_move(
        &mut self,
        from: &std::path::Path,
        to: &std::path::Path,
    ) -> Result<Vec<DocumentId>> {
        self.check_file_action(from)?;
        let mut moved = vec![];
        for entry in self.entries.values_mut() {
            let DocumentOrigin::File(path) = entry.document.origin() else {
                continue;
            };
            let Some(path) = kode_workspace::tree::moved_path(path, from, to) else {
                continue;
            };
            entry.document.retarget_file(path.clone())?;
            if let DocumentSave::File(save) = &mut entry.save {
                save.retarget_file(&path)?;
            }
            moved.push(entry.document.id());
        }
        Ok(moved)
    }
    pub fn find_origin(&self, origin: &DocumentOrigin) -> Option<DocumentId> {
        self.entries
            .values()
            .find(|entry| match (entry.document.origin(), origin) {
                (
                    DocumentOrigin::Virtual {
                        owner: a, key: ak, ..
                    },
                    DocumentOrigin::Virtual {
                        owner: b, key: bk, ..
                    },
                ) => a == b && ak == bk,
                (a, b) => a == b,
            })
            .map(|entry| entry.document.id())
    }
    pub fn insert(
        &mut self,
        mut document: Document,
        descriptor: &HostDescriptor,
        workspace: WorkspaceGeneration,
        disk: Option<DiskIdentity>,
    ) -> Result<DocumentId> {
        let id = document.id();
        if self.entries.contains_key(&id) {
            return Err(DomainError::Unsupported(
                "document identity already registered".into(),
            ));
        }
        document.set_read_only(document.is_read_only() || descriptor.is_read_only());
        let save = if document.is_read_only() {
            DocumentSave::None
        } else {
            match &descriptor.save_target {
                SaveTarget::File if matches!(document.origin(), DocumentOrigin::File(_)) => {
                    DocumentSave::File(SaveController::new(workspace, disk))
                }
                SaveTarget::File => return Err(DomainError::NoSaveTarget),
                SaveTarget::Owner(key) => DocumentSave::Owner {
                    key: key.clone(),
                    active: None,
                    queued: None,
                    error: None,
                },
                SaveTarget::None => DocumentSave::None,
            }
        };
        self.entries.insert(
            id,
            DocumentEntry {
                document,
                save,
                save_owner: kode_platform::jobs::OwnerId(RequestId::fresh()?.0),
            },
        );
        Ok(id)
    }
    pub fn insert_file(
        &mut self,
        prepared: PreparedDocument,
        descriptor: &HostDescriptor,
        workspace: WorkspaceGeneration,
    ) -> Result<DocumentId> {
        if !matches!(prepared.document.origin(), DocumentOrigin::File(_)) {
            return Err(DomainError::NoSaveTarget);
        }
        self.insert(
            prepared.document,
            descriptor,
            workspace,
            prepared.disk_identity,
        )
    }
    pub fn create_virtual(
        &mut self,
        descriptor: &HostDescriptor,
        workspace: WorkspaceGeneration,
        text: &str,
    ) -> Result<DocumentId> {
        let document = Document::new(
            DocumentId::fresh()?,
            DocumentOrigin::Virtual {
                owner: descriptor.owner.clone(),
                key: descriptor.key.clone(),
                label: descriptor.label.clone(),
            },
            descriptor.language.clone(),
            text,
        );
        self.insert(document, descriptor, workspace, None)
    }
    pub fn save_error(&self, id: DocumentId) -> Option<&DomainError> {
        self.entries.get(&id).and_then(|entry| match &entry.save {
            DocumentSave::File(save) => save.error.as_ref(),
            DocumentSave::Owner { error, .. } => error.as_ref(),
            DocumentSave::None => None,
        })
    }
    pub fn disk_identity(&self, id: DocumentId) -> Option<&DiskIdentity> {
        match &self.entries.get(&id)?.save {
            DocumentSave::File(save) => save.disk_identity.as_ref(),
            _ => None,
        }
    }
    pub fn save_in_progress(&self, id: DocumentId) -> bool {
        self.entries
            .get(&id)
            .is_some_and(|entry| match &entry.save {
                DocumentSave::File(save) => save.is_saving(),
                DocumentSave::Owner { active, .. } => active.is_some(),
                DocumentSave::None => false,
            })
    }
    /// A view can detach without removing its document. Physical saves and dirty
    /// recovery remain owned here until the composition layer resolves close.
    pub fn remove(&mut self, id: DocumentId) -> Result<Option<Document>> {
        if self.save_in_progress(id) {
            return Err(DomainError::Unsupported(
                "document still has an active save".into(),
            ));
        }
        Ok(self.entries.remove(&id).map(|entry| entry.document))
    }
    fn request_save(
        &mut self,
        id: DocumentId,
        workspace: WorkspaceGeneration,
    ) -> Result<Option<EditorWork>> {
        if let DocumentOrigin::File(path) = self.document(id)?.origin()
            && self
                .file_mutations
                .values()
                .flatten()
                .any(|affected| path.starts_with(affected))
        {
            return Err(DomainError::Unsupported(
                "wait for the file operation before saving".into(),
            ));
        }
        let entry = self.entry_mut(id)?;
        if entry.document.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        match &mut entry.save {
            DocumentSave::File(save) => {
                Ok(save.request(&entry.document)?.map(EditorWork::SaveFile))
            }
            DocumentSave::Owner {
                key,
                active,
                queued,
                error,
            } => {
                *error = None;
                let snapshot = entry.document.snapshot();
                if active.is_some() {
                    *queued = Some(snapshot);
                    return Ok(None);
                }
                let job = OwnerSaveJob {
                    key: key.clone(),
                    tag: fresh_tag(workspace, &snapshot)?,
                    snapshot,
                };
                *active = Some(job.clone());
                Ok(Some(EditorWork::SaveOwner(job)))
            }
            DocumentSave::None => Err(DomainError::NoSaveTarget),
        }
    }
    pub fn complete_file_save(
        &mut self,
        id: DocumentId,
        tag: JobTag,
        result: Result<DiskIdentity>,
    ) -> Result<Option<EditorWork>> {
        let entry = self.entry_mut(id)?;
        let DocumentSave::File(save) = &mut entry.save else {
            return Err(DomainError::NoSaveTarget);
        };
        Ok(save
            .complete(tag, result, &mut entry.document)?
            .map(EditorWork::SaveFile))
    }
    pub fn complete_owner_save(
        &mut self,
        id: DocumentId,
        tag: JobTag,
        result: Result<()>,
    ) -> Result<Option<EditorWork>> {
        let entry = self.entry_mut(id)?;
        let DocumentSave::Owner {
            key,
            active,
            queued,
            error,
        } = &mut entry.save
        else {
            return Err(DomainError::NoSaveTarget);
        };
        if active.as_ref().is_none_or(|job| job.tag != tag) {
            return Ok(None);
        }
        let saved = active.take().expect("save checked");
        match result {
            Ok(()) => {
                entry.document.acknowledge_save(&saved.snapshot)?;
                if let Some(snapshot) = queued.take() {
                    let job = OwnerSaveJob {
                        key: key.clone(),
                        tag: fresh_tag(tag.workspace, &snapshot)?,
                        snapshot,
                    };
                    *active = Some(job.clone());
                    return Ok(Some(EditorWork::SaveOwner(job)));
                }
            }
            Err(failure) => {
                *error = Some(failure);
                *queued = None;
            }
        }
        Ok(None)
    }
}
fn fresh_tag(workspace: WorkspaceGeneration, snapshot: &TextSnapshot) -> Result<JobTag> {
    Ok(JobTag {
        workspace,
        request: RequestId::fresh()?,
        document: Some((snapshot.id, snapshot.revision)),
    })
}
#[derive(Debug, Clone)]
pub enum PasteTarget {
    Editor(PasteTicket),
    Search(FieldPasteTicket),
}
#[derive(Debug)]
pub enum EditorWork {
    SaveFile(SaveJob),
    SaveOwner(OwnerSaveJob),
    ClipboardPublish {
        epoch: u64,
    },
    ClipboardRead(PasteTarget),
    Search {
        tag: JobTag,
        snapshot: TextSnapshot,
        query: String,
    },
    Folds {
        tag: JobTag,
        snapshot: TextSnapshot,
    },
    Completion(CompletionJob),
    Navigation(NavigationJob),
    Overlays(OverlayJob),
    Changed(HostChange),
    Execute(HostExecution),
    Navigate(NavigationTarget),
    FieldLimitReached,
}

#[derive(Debug, Clone, Copy)]
pub enum ClipboardJob {
    Read,
    Publish(u64),
}
#[derive(Debug)]
pub enum ClipboardResult {
    Read(String),
    Published { epoch: u64, native: bool },
}
pub fn schedule_clipboard(
    scheduler: &mut dyn kode_platform::jobs::WorkerScheduler<ClipboardResult>,
    owner: kode_platform::jobs::OwnerId,
    tag: JobTag,
    job: ClipboardJob,
    clipboard: Arc<ClipboardService>,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        kode_platform::jobs::Delivery::Cancellable,
        Box::new(move |cancel| {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            match job {
                ClipboardJob::Read => clipboard.read().map(ClipboardResult::Read),
                ClipboardJob::Publish(epoch) => Ok(ClipboardResult::Published {
                    epoch,
                    native: clipboard.flush_native(epoch),
                }),
            }
        }),
    )
}

struct Hooks<'a> {
    search: &'a mut SearchBar,
    visuals: &'a mut EditorVisuals,
    completion: &'a mut CompletionPopup,
    navigation: &'a mut NavigationController,
}
impl EditorInputHooks for Hooks<'_> {
    fn collapsed_folds(
        &self,
        snapshot: &TextSnapshot,
    ) -> Option<Vec<super::layout::CollapsedFold>> {
        self.visuals.collapsed_folds(snapshot)
    }
    fn reveal_caret(&mut self, snapshot: &TextSnapshot, position: ByteOffset) -> Result<bool> {
        self.visuals.reveal_caret(snapshot, position)
    }
    fn context_changed(&mut self, snapshot: &TextSnapshot, state: &EditorState) {
        self.completion.context_changed(snapshot, state);
        self.navigation.context_changed(snapshot, state);
    }
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        if matches!(event, InputEvent::Focus(false)) {
            self.search
                .dispatch(target, event, document, state, context)?;
            self.completion
                .dispatch(target, event, document, state, context)?;
            self.navigation
                .dispatch(target, event, document, state, context)?;
            return Ok(false);
        }
        if target == InputTarget::Popup {
            self.search
                .dispatch(target, event, document, state, context)?;
            if self
                .completion
                .dispatch(target, event, document, state, context)?
            {
                return Ok(true);
            }
            return self
                .navigation
                .dispatch(target, event, document, state, context);
        }
        match target {
            InputTarget::Search => self
                .search
                .dispatch(target, event, document, state, context),
            InputTarget::Navigation => self
                .navigation
                .dispatch(target, event, document, state, context),
            _ => self
                .visuals
                .dispatch(target, event, document, state, context),
        }
    }
}

/// One view with independent caret/focus/search/popups over a registry document.
/// Multiple views share edits and the document's serialized save controller.
pub struct EditorView<H: EditorHost> {
    /// Composite previews can supply their own header/line-number columns while
    /// retaining the common editor's input, search, selection and mutation guard.
    pub chrome: bool,
    pub host: H,
    pub input: EditorInput,
    pub search: SearchBar,
    pub visuals: EditorVisuals,
    pub completion: CompletionPopup,
    pub navigation: NavigationController,
    pub overlays: OverlayCache,
    pub error: Option<DomainError>,
    workspace: WorkspaceGeneration,
    clipboard: Arc<ClipboardService>,
    fold_tag: Option<JobTag>,
    work: Vec<EditorWork>,
    observed_revision: Revision,
    detached: bool,
}
impl<H: EditorHost> EditorView<H> {
    pub fn new(
        document: DocumentId,
        documents: &EditorDocuments,
        host: H,
        workspace: WorkspaceGeneration,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let source = documents.document(document)?;
        if host.descriptor().is_read_only() && !source.is_read_only() {
            return Err(DomainError::Unsupported(
                "read-only host requires a read-only registry document".into(),
            ));
        }
        let mut navigation =
            NavigationController::new(workspace, host.descriptor().project_root.clone(), clock);
        navigation.enabled = host.intelligence_provider().is_some();
        let mut completion = CompletionPopup::new(workspace);
        completion.auto_show_external = host.descriptor().auto_show_external;
        let mut view = Self {
            chrome: true,
            host,
            input: EditorInput::new(FocusId(RequestId::fresh()?.0), document),
            search: SearchBar::new(workspace),
            visuals: EditorVisuals::default(),
            completion,
            navigation,
            overlays: OverlayCache::default(),
            error: None,
            workspace,
            clipboard,
            fold_tag: None,
            work: vec![],
            observed_revision: source.revision(),
            detached: false,
        };
        view.request_folds(&source.snapshot())?;
        Ok(view)
    }
    pub fn document_id(&self) -> DocumentId {
        self.input.document
    }
    pub fn owner(&self) -> kode_platform::jobs::OwnerId {
        kode_platform::jobs::OwnerId(self.input.id.0)
    }
    pub fn take_work(&mut self) -> Vec<EditorWork> {
        std::mem::take(&mut self.work)
    }
    pub fn geometry(&self, documents: &EditorDocuments, area: Rect) -> Result<EditorGeometry> {
        if !self.chrome {
            let search = self.search.preferred_height().min(area.height);
            return Ok(EditorGeometry {
                header: Rect::default(),
                gutter: Rect::default(),
                minimap: Rect::default(),
                search: Rect::new(area.x, area.y, area.width, search),
                text: Rect::new(area.x, area.y + search, area.width, area.height - search),
            });
        }
        Ok(EditorGeometry::new(
            area,
            documents
                .document(self.document_id())?
                .snapshot()
                .len_lines(),
            self.search.preferred_height(),
        ))
    }
    fn request_folds(&mut self, snapshot: &TextSnapshot) -> Result<()> {
        let tag = fresh_tag(self.workspace, snapshot)?;
        self.fold_tag = Some(tag);
        self.work
            .retain(|work| !matches!(work, EditorWork::Folds { .. }));
        self.work.push(EditorWork::Folds {
            tag,
            snapshot: snapshot.clone(),
        });
        Ok(())
    }
    fn copied(&mut self, text: &str) -> Result<()> {
        let epoch = self.clipboard.remember(text)?;
        self.work.push(EditorWork::ClipboardPublish { epoch });
        Ok(())
    }
    fn changed(&mut self, snapshot: TextSnapshot, deltas: Option<Vec<EditDelta>>) -> Result<()> {
        self.observed_revision = snapshot.revision;
        self.overlays.changed(&snapshot, deltas.as_deref());
        self.navigation
            .set_declaration_spans(&snapshot, self.overlays.declarations())?;
        let change = HostChange {
            snapshot: snapshot.clone(),
            deltas,
        };
        self.host.on_change(change.clone());
        self.work.push(EditorWork::Changed(change));
        self.search.document_changed(&snapshot);
        self.visuals.folds.invalidate();
        self.request_folds(&snapshot)?;
        self.completion.auto_show_external = self.host.descriptor().auto_show_external;
        if let Some(job) = self.completion.on_edit(
            &snapshot,
            &self.input.state,
            self.host.completion_provider().is_some(),
        )? {
            self.work.push(EditorWork::Completion(job));
        }
        Ok(())
    }
    fn effects(
        &mut self,
        effects: Vec<EditorEffect>,
        documents: &mut EditorDocuments,
    ) -> Result<()> {
        if effects
            .iter()
            .any(|effect| matches!(effect, EditorEffect::DismissCompletion))
        {
            self.completion.dismiss();
        }
        for effect in effects {
            match effect {
                EditorEffect::Copy(text) => self.copied(&text)?,
                EditorEffect::ReadClipboard(ticket) => self
                    .work
                    .push(EditorWork::ClipboardRead(PasteTarget::Editor(ticket))),
                EditorEffect::Save => {
                    if let Some(work) =
                        documents.request_save(self.document_id(), self.workspace)?
                    {
                        self.work.push(work);
                    }
                }
                EditorEffect::OpenSearch(selected) => self.search.open(
                    &documents.document(self.document_id())?.snapshot(),
                    &selected,
                ),
                EditorEffect::RequestCompletion => {
                    self.navigation.dismiss();
                    let job = self.completion.begin(
                        &documents.document(self.document_id())?.snapshot(),
                        &self.input.state,
                    )?;
                    self.work.push(EditorWork::Completion(job));
                }
                EditorEffect::Changed { deltas, .. } => {
                    self.changed(documents.document(self.document_id())?.snapshot(), deltas)?
                }
                _ => {}
            }
        }
        self.drain_controls()
    }
    fn drain_controls(&mut self) -> Result<()> {
        for effect in self.search.take_effects() {
            match effect {
                SearchEffect::Scan {
                    tag,
                    snapshot,
                    query,
                } => self.work.push(EditorWork::Search {
                    tag,
                    snapshot,
                    query,
                }),
                SearchEffect::Copy(text) => self.copied(&text)?,
                SearchEffect::ReadClipboard(ticket) => self
                    .work
                    .push(EditorWork::ClipboardRead(PasteTarget::Search(ticket))),
                SearchEffect::FieldLimitReached => self.work.push(EditorWork::FieldLimitReached),
            }
        }
        for effect in self.navigation.take_effects() {
            match effect {
                NavigationEffect::Run(job) => {
                    self.completion.dismiss();
                    self.work.push(EditorWork::Navigation(job));
                }
                NavigationEffect::Navigate(target) => {
                    self.host.on_navigate(target.clone());
                    self.work.push(EditorWork::Navigate(target));
                }
            }
        }
        Ok(())
    }
    fn reconcile(&mut self, documents: &EditorDocuments, area: Rect, reveal: bool) -> Result<()> {
        let geometry = self.geometry(documents, area)?;
        let mut hooks = Hooks {
            search: &mut self.search,
            visuals: &mut self.visuals,
            completion: &mut self.completion,
            navigation: &mut self.navigation,
        };
        self.input.reconcile(
            documents.document(self.input.document)?,
            InputViewport {
                geometry,
                folds: &[],
            },
            &mut hooks,
            reveal,
        )
    }
    /// The application broadcasts document changes to every attached view before
    /// painting. Input also synchronizes, so stale carets cannot split new UTF-8.
    pub fn synchronize(&mut self, documents: &EditorDocuments) -> Result<()> {
        if self.detached {
            return Ok(());
        }
        let snapshot = documents.document(self.document_id())?.snapshot();
        if snapshot.revision != self.observed_revision {
            self.input
                .state
                .caret
                .restore_clamped(&snapshot, self.input.state.caret.selection);
            self.completion
                .context_changed(&snapshot, &self.input.state);
            self.navigation
                .context_changed(&snapshot, &self.input.state);
            self.changed(snapshot, None)?;
            self.drain_controls()?;
        }
        Ok(())
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
        area: Rect,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        self.synchronize(documents)?;
        let geometry = self.geometry(documents, area)?;
        self.navigation.enabled = self.host.intelligence_provider().is_some();
        let outcome = {
            let entry = documents.entry_mut(self.document_id())?;
            let mut hooks = Hooks {
                search: &mut self.search,
                visuals: &mut self.visuals,
                completion: &mut self.completion,
                navigation: &mut self.navigation,
            };
            self.input.dispatch(
                event,
                &mut entry.document,
                focus,
                InputViewport {
                    geometry,
                    folds: &[],
                },
                &mut hooks,
            )?
        };
        let mut handled = outcome.handled;
        self.effects(outcome.effects, documents)?;
        if !handled
            && self.input.has_focus(focus)
            && !self.input.state.search_has_focus
            && let Some(action) = self.host.descriptor().execute
            && matches!(event,InputEvent::Key(input) if crate::input::shortcuts::command_key(if action==ExecutionAction::SendRequest {crate::input::shortcuts::Scope::Request}else{crate::input::shortcuts::Scope::Sql},*input).is_some() && input.key == Key::Enter && input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta)
        {
            let action = if action == ExecutionAction::SqlSelectionOrStatement
                && matches!(event,InputEvent::Key(input) if input.modifiers.shift)
            {
                ExecutionAction::SqlScript
            } else {
                action
            };
            self.execute(action, documents)?;
            handled = true;
        }
        self.reconcile(documents, area, false)?;
        Ok(handled)
    }
    pub fn command(
        &mut self,
        command: EditorCommand,
        documents: &mut EditorDocuments,
        area: Rect,
    ) -> Result<()> {
        if self.detached {
            return Err(DomainError::Cancelled);
        }
        self.synchronize(documents)?;
        let geometry = self.geometry(documents, area)?;
        let outcome = self.input.command(
            command,
            &mut documents.entry_mut(self.document_id())?.document,
            InputViewport {
                geometry,
                folds: &[],
            },
        )?;
        self.effects(outcome.effects, documents)?;
        self.reconcile(documents, area, true)
    }
    pub fn programmatic_replace(
        &mut self,
        range: TextRange,
        text: &str,
        documents: &mut EditorDocuments,
        area: Rect,
    ) -> Result<()> {
        self.command(
            EditorCommand::Replace {
                range,
                text: text.into(),
                kind: EditKind::Programmatic,
            },
            documents,
            area,
        )
    }
    pub fn execute(&mut self, action: ExecutionAction, documents: &EditorDocuments) -> Result<()> {
        let supported = match action {
            ExecutionAction::SqlSelectionOrStatement | ExecutionAction::SqlScript => {
                self.host.descriptor().role == HostRole::Sql
            }
            ExecutionAction::SendRequest => {
                matches!(
                    self.host.descriptor().role,
                    HostRole::RestBody | HostRole::RestHeaders
                ) || (self.host.descriptor().role == HostRole::RawRequest
                    && self.host.descriptor().owner == "rest-request")
            }
        };
        if !supported {
            return Err(DomainError::Unsupported(
                "execution is not owned by this editor host".into(),
            ));
        }
        let execution = HostExecution {
            action,
            snapshot: documents.document(self.document_id())?.snapshot(),
            selection: self.input.state.caret.selection,
        };
        self.host.on_execute(execution.clone());
        self.work.push(EditorWork::Execute(execution));
        Ok(())
    }
    pub fn tick(
        &mut self,
        documents: &EditorDocuments,
        focus: &FocusState,
        area: Rect,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        self.synchronize(documents)?;
        let snapshot = documents.document(self.document_id())?.snapshot();
        let geometry = self.geometry(documents, area)?;
        let folds = self.visuals.folds.collapsed(&snapshot);
        let outcome = self.input.tick_drag(
            documents.document(self.document_id())?,
            focus,
            InputViewport {
                geometry,
                folds: &folds,
            },
        )?;
        self.navigation
            .context_changed(&snapshot, &self.input.state);
        self.completion
            .context_changed(&snapshot, &self.input.state);
        self.navigation.tick(&snapshot, &self.input.state);
        let (visible, coverage) = {
            let layout = self.input.layout(
                documents.document(self.document_id())?,
                InputViewport {
                    geometry,
                    folds: &folds,
                },
            )?;
            let mut rows =
                layout.visible_rows(self.input.state.scroll_rows, geometry.text.height as usize);
            let first = rows.next();
            let last = rows.last().or(first);
            let start = first.map_or(ByteOffset(0), |(_, row)| row.source.start);
            let end = last.map_or(start, |(_, row)| row.source.end);
            let first_line = first.map_or(0, |(_, row)| row.line.0).saturating_sub(24);
            let last_line = last
                .map_or(first_line, |(_, row)| row.fold_end.unwrap_or(row.line).0)
                .saturating_add(25);
            let coverage_end = if last_line < snapshot.len_lines() {
                snapshot.line_start(LineIndex(last_line))?
            } else {
                ByteOffset(snapshot.len_bytes())
            };
            (
                TextRange { start, end },
                TextRange {
                    start: snapshot.line_start(LineIndex(first_line))?,
                    end: coverage_end,
                },
            )
        };
        if let Some(job) = self.overlays.request(
            &snapshot,
            visible,
            coverage,
            self.workspace,
            self.host
                .descriptor()
                .language
                .clone()
                .or_else(|| snapshot.language.clone())
                .unwrap_or_default(),
        )? {
            self.work
                .retain(|work| !matches!(work, EditorWork::Overlays(_)));
            self.work.push(EditorWork::Overlays(job));
        }
        self.drain_controls()?;
        Ok(outcome.repaint)
    }
    pub fn complete_overlays(
        &mut self,
        tag: JobTag,
        result: Result<OverlayReport>,
        documents: &EditorDocuments,
    ) -> Result<bool> {
        if self.detached || tag.workspace != self.workspace {
            return Ok(false);
        }
        let snapshot = documents.document(self.document_id())?.snapshot();
        let accepted = self.overlays.complete(tag, result, &snapshot)?;
        if accepted {
            self.navigation
                .set_declaration_spans(&snapshot, self.overlays.declarations())?;
        }
        Ok(accepted)
    }
    pub fn complete_folds(
        &mut self,
        tag: JobTag,
        result: Result<FoldReport>,
        documents: &EditorDocuments,
        area: Rect,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        let snapshot = documents.document(self.document_id())?.snapshot();
        if self.fold_tag != Some(tag) || tag.document != Some((snapshot.id, snapshot.revision)) {
            return Ok(false);
        }
        self.fold_tag = None;
        match result {
            Ok(report) => {
                if !self.visuals.folds.publish(report, &snapshot) {
                    return Ok(false);
                }
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        self.reconcile(documents, area, false)?;
        Ok(true)
    }
    pub fn complete_search(
        &mut self,
        tag: JobTag,
        result: Result<SearchReport>,
        documents: &mut EditorDocuments,
        area: Rect,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        let (before, selection) = {
            let doc = documents.document(self.document_id())?;
            (doc.revision(), self.input.state.caret.selection)
        };
        let accepted = self.search.complete_scan(
            tag,
            result,
            &mut documents.entry_mut(self.document_id())?.document,
            &mut self.input.state,
        )?;
        let snapshot = documents.document(self.document_id())?.snapshot();
        let changed = snapshot.revision != before;
        if changed {
            self.changed(snapshot, None)?;
        }
        self.drain_controls()?;
        self.reconcile(
            documents,
            area,
            changed || selection != self.input.state.caret.selection,
        )?;
        Ok(accepted)
    }
    pub fn complete_completion(
        &mut self,
        tag: JobTag,
        result: Result<Vec<CompletionItem>>,
        documents: &EditorDocuments,
        focus: &FocusState,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        self.completion.complete(
            tag,
            result,
            &documents.document(self.document_id())?.snapshot(),
            &self.input.state,
            self.input.has_focus(focus),
        )
    }
    pub fn complete_navigation(
        &mut self,
        tag: JobTag,
        result: Result<NavigationResult>,
        documents: &EditorDocuments,
        owner_active: bool,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        let accepted = self.navigation.complete(
            tag,
            result,
            &documents.document(self.document_id())?.snapshot(),
            &self.input.state,
            owner_active,
        );
        self.drain_controls()?;
        Ok(accepted)
    }
    pub fn complete_clipboard(
        &mut self,
        target: &PasteTarget,
        result: Result<String>,
        documents: &mut EditorDocuments,
        focus: &FocusState,
        area: Rect,
    ) -> Result<bool> {
        if self.detached {
            return Ok(false);
        }
        let text = result?;
        let geometry = self.geometry(documents, area)?;
        let accepted = match target {
            PasteTarget::Editor(ticket) => {
                let outcome = self.input.complete_paste(
                    ticket,
                    &text,
                    &mut documents.entry_mut(self.document_id())?.document,
                    focus,
                    InputViewport {
                        geometry,
                        folds: &[],
                    },
                )?;
                let accepted = outcome.handled;
                self.effects(outcome.effects, documents)?;
                accepted
            }
            PasteTarget::Search(ticket) => self.search.complete_paste(
                ticket,
                &text,
                documents.document(self.document_id())?,
                &self.input.state,
                self.input.has_focus(focus),
            )?,
        };
        self.drain_controls()?;
        self.reconcile(
            documents,
            area,
            accepted && matches!(target, PasteTarget::Editor(_)),
        )?;
        Ok(accepted)
    }
    pub fn intelligence_changed(&mut self) {
        self.completion.dismiss();
        self.navigation.dismiss();
        self.overlays.invalidate();
    }
    pub fn detach(&mut self, focus: &mut FocusState) {
        self.detached = true;
        self.search.close(&mut self.input.state);
        self.completion.dismiss();
        self.navigation.dismiss();
        self.fold_tag = None;
        self.work.retain(|work| {
            matches!(
                work,
                EditorWork::SaveFile(_)
                    | EditorWork::SaveOwner(_)
                    | EditorWork::ClipboardPublish { .. }
            )
        });
        if focus.focused == Some(self.input.id) {
            focus.focused = None;
        }
        if focus.captured == Some(self.input.id) {
            focus.captured = None;
        }
    }
    pub fn render(
        &mut self,
        documents: &EditorDocuments,
        focus: &FocusState,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        extra_overlays: &[OverlaySpan],
    ) -> Result<Option<Position>> {
        if self.detached {
            return Ok(None);
        }
        let document = documents.document(self.document_id())?;
        let snapshot = document.snapshot();
        self.input
            .state
            .caret
            .restore_clamped(&snapshot, self.input.state.caret.selection);
        let geometry = self.geometry(documents, area)?;
        let folds = self.visuals.folds.collapsed(&snapshot);
        let layout = self.input.layout(
            document,
            InputViewport {
                geometry,
                folds: &folds,
            },
        )?;
        let mut overlays = self.overlays.overlays(theme);
        if let Some(range) = self.navigation.hover_range(&snapshot) {
            overlays.push(OverlaySpan {
                range,
                priority: 26,
                style: ratatui::style::Style::default()
                    .add_modifier(ratatui::style::Modifier::UNDERLINED),
            });
        }
        overlays.extend(self.search.overlays(&snapshot, theme));
        if let Some(highlight) = self.host.transient_highlight()
            && highlight.revision == snapshot.revision
        {
            snapshot.slice(highlight.range)?;
            overlays.push(OverlaySpan {
                range: highlight.range,
                priority: 40,
                style: theme.style("code-search-active"),
            });
        }
        overlays.extend(extra_overlays.iter().map(|overlay| OverlaySpan {
            range: overlay.range,
            priority: overlay.priority,
            style: overlay.style,
        }));
        let starts: Vec<_> = self
            .visuals
            .folds
            .regions()
            .map(|region| region.start)
            .collect();
        let header = EditorHeader::for_document(
            document,
            self.host.descriptor().mime.clone(),
            self.overlays.label(&self.host.descriptor().grammar),
        );
        let state = self.input.render_state(focus);
        let caret = render_editor(
            &layout,
            buffer,
            theme,
            RenderOptions {
                geometry,
                header: &header,
                state: &state,
                overlays: &overlays,
                warning_lines: self.host.warning_lines(),
                fold_starts: &starts,
            },
        );
        let preview = Minimap::prepare(
            &snapshot,
            geometry.minimap.width as usize,
            geometry.minimap.height as usize,
            self.input.state.caret.cursor(),
        )?;
        preview.render(geometry.minimap, buffer, theme);
        let search_caret = self.search.render(
            geometry.search,
            buffer,
            theme,
            self.input.has_focus(focus) && self.input.state.search_has_focus,
        );
        let context = HookContext {
            geometry,
            layout: &layout,
        };
        self.completion
            .render(context, &self.input.state, buffer, theme);
        self.navigation
            .render(context, &self.input.state, buffer, theme);
        Ok(
            if !self.input.has_focus(focus)
                || !self.completion.entries.is_empty()
                || self.navigation.definition.is_some()
                || self.navigation.usages.is_some()
            {
                None
            } else if self.input.state.search_has_focus {
                search_caret
            } else {
                caret
            },
        )
    }
}
