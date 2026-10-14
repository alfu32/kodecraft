//! Reusable document/view ownership and nonblocking host contracts.
use super::{
    folding::{EditorVisuals, FoldReport},
    input::{
        EditorEffect, EditorInput, EditorInputHooks, EditorState, HookContext, InputTarget,
        InputViewport, PasteTicket,
    },
    loading::{PreparedDocument, SaveController, SaveJob},
    minimap::Minimap,
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
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, RequestId, Result, Revision,
    Selection, TextRange, WorkspaceGeneration,
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
}
#[derive(Default)]
pub struct EditorDocuments {
    entries: HashMap<DocumentId, DocumentEntry>,
}
impl EditorDocuments {
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
        self.entries.insert(id, DocumentEntry { document, save });
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
    Changed(HostChange),
    Execute(HostExecution),
    Navigate(NavigationTarget),
    FieldLimitReached,
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
