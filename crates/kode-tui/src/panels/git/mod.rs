//! Application-thread Git controls and shared editor composition.
pub mod diff;
mod input;
pub mod read;
mod render;
use crate::{
    app::events::ResultGate,
    editor::host::{
        ClipboardJob, ClipboardResult, EditorDocuments, EditorView, EditorWork, HostDescriptor,
        HostRole, QueuedEditorHost,
    },
    focus::{FocusId, FocusState},
    theme::Theme,
    widgets::dialog::TextDialog,
};
use kode_core::{
    DocumentId, DomainError, JobTag, RequestId, Result, Revision, TextRange, WorkspaceGeneration,
    document::TextSnapshot,
};
use kode_git::{
    model::*,
    read::{ReadJob, ReadOutput, ReadRequest},
    write::{WriteJob, WriteReceipt, WriteRequest},
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock, OwnerId},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
};

#[derive(Debug, Clone)]
pub enum ReadKind {
    Discover,
    Snapshot,
    Files(String),
    Diff,
}
impl ReadKind {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::Discover => "git-discover",
            Self::Snapshot => "git-refresh",
            Self::Files(_) => "git-files",
            Self::Diff => "git-diff",
        }
    }
}
pub enum GitCommand {
    Init,
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Commit(TextSnapshot),
    Checkout(String),
    Tag {
        name: String,
        revision: String,
        move_existing: bool,
    },
}
impl GitCommand {
    pub fn changes_worktree(&self) -> bool {
        matches!(self, Self::Checkout(_))
    }
    pub fn run(self, root: PathBuf, cancel: &CancellationToken) -> Result<WriteReceipt> {
        self.run_protecting(root, cancel, &[])
    }
    pub fn run_protecting(
        self,
        root: PathBuf,
        cancel: &CancellationToken,
        protected_paths: &[PathBuf],
    ) -> Result<WriteReceipt> {
        let request = match self {
            Self::Init => WriteRequest::Init,
            Self::Stage(paths) => WriteRequest::Stage(paths),
            Self::Unstage(paths) => WriteRequest::Unstage(paths),
            Self::Commit(snapshot) => {
                if snapshot.len_bytes() > 128 * 1024 {
                    return Err(DomainError::Git {
                        operation: "validate commit message".into(),
                        message: "commit message exceeds 128 KiB".into(),
                    });
                }
                WriteRequest::Commit {
                    message: snapshot.text().to_string(),
                }
            }
            Self::Checkout(branch) => WriteRequest::Checkout { branch },
            Self::Tag {
                name,
                revision,
                move_existing,
            } => WriteRequest::Tag {
                name,
                revision,
                move_existing,
            },
        };
        WriteJob { root, request }.run_protecting(cancel, protected_paths)
    }
}
pub enum GitWork {
    Read {
        tag: JobTag,
        kind: ReadKind,
        job: ReadJob,
    },
    Write {
        tag: JobTag,
        root: PathBuf,
        command: GitCommand,
    },
    Editor {
        owner: OwnerId,
        work: EditorWork,
    },
    Clipboard {
        tag: JobTag,
        job: ClipboardJob,
        read: bool,
    },
}
#[derive(Debug, Default, Clone, Copy)]
pub struct GitGeometry {
    pub area: Rect,
    pub toolbar: Rect,
    pub status: Rect,
    pub first_divider: Rect,
    pub editor: Rect,
    pub commit: Rect,
    pub second_divider: Rect,
    pub history: Rect,
    pub notice: Rect,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Status,
    Message,
    History,
}
enum Menu {
    Branches {
        snapshot: Arc<RepositorySnapshot>,
        selected: usize,
        scroll: usize,
    },
    Tag {
        dialog: TextDialog,
        revision: String,
        move_existing: bool,
    },
}
struct FieldPaste {
    tag: JobTag,
    modal: RequestId,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
#[derive(Clone)]
struct HistoryRow {
    commit: String,
    path: Option<PathBuf>,
}
pub struct GitPanel {
    pub read: read::GitReadPanel,
    pub commit_editor: Option<EditorView<QueuedEditorHost>>,
    pub history_id: FocusId,
    pub message: String,
    pub write_busy: Option<JobTag>,
    checkout_pending: bool,
    pub geometry: GitGeometry,
    init_button: Rect,
    pub latest_diff: Option<Arc<kode_git::diff::DiffPlan>>,
    pub repo_available: Option<bool>,
    pub selected_commit: Option<String>,
    pub history_selected: usize,
    expanded: HashSet<String>,
    files: HashMap<String, Listing<PathBuf>>,
    pending_files: Option<(String, JobTag)>,
    waiting_files: VecDeque<String>,
    discovery: Option<JobTag>,
    diff_tag: Option<JobTag>,
    refresh_requested: bool,
    stale: bool,
    part: Part,
    menu: Option<Menu>,
    modal_id: Option<RequestId>,
    modal_previous_focus: Option<FocusId>,
    modal_viewport: Rect,
    paste: Option<FieldPaste>,
    commit_arm: bool,
    divider_drag: Option<bool>,
    status_height: Option<u16>,
    message_height: Option<u16>,
    pending_message: Option<(DocumentId, Revision)>,
    gate: ResultGate,
    work: VecDeque<GitWork>,
    clipboard: Arc<ClipboardService>,
    clock: Arc<dyn Clock>,
    last_refresh: u64,
}
impl GitPanel {
    pub fn new(
        root: PathBuf,
        workspace: WorkspaceGeneration,
        documents: &mut EditorDocuments,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let mut descriptor = HostDescriptor::new(
            HostRole::CommitMessage,
            "git".into(),
            "commit-message".into(),
            "Commit message".into(),
            root.clone(),
        );
        descriptor.language = Some("plain-text".into());
        descriptor.mime = Some("text/plain".into());
        let document = documents.create_virtual(&descriptor, workspace, "")?;
        let editor = EditorView::new(
            document,
            documents,
            QueuedEditorHost::new(descriptor),
            workspace,
            clipboard.clone(),
            clock.clone(),
        )?;
        Ok(Self {
            read: read::GitReadPanel::new(root, workspace)?,
            commit_editor: Some(editor),
            history_id: FocusId(RequestId::fresh()?.0),
            message: String::new(),
            write_busy: None,
            checkout_pending: false,
            geometry: Default::default(),
            init_button: Rect::default(),
            latest_diff: None,
            repo_available: None,
            selected_commit: None,
            history_selected: 0,
            expanded: HashSet::new(),
            files: HashMap::new(),
            pending_files: None,
            waiting_files: VecDeque::new(),
            discovery: None,
            diff_tag: None,
            refresh_requested: true,
            stale: false,
            part: Part::Status,
            menu: None,
            modal_id: None,
            modal_previous_focus: None,
            modal_viewport: Rect::default(),
            paste: None,
            commit_arm: false,
            divider_drag: None,
            status_height: None,
            message_height: None,
            pending_message: None,
            gate: ResultGate::new(workspace),
            work: VecDeque::new(),
            clipboard,
            clock,
            last_refresh: 0,
        })
    }
    pub fn id(&self) -> FocusId {
        self.read.id
    }
    pub fn owner(&self) -> OwnerId {
        OwnerId(self.id().0)
    }
    pub fn focus_id(&self) -> FocusId {
        match self.part {
            Part::Status => self.id(),
            Part::Message => self
                .commit_editor
                .as_ref()
                .map_or(self.id(), |view| view.input.id),
            Part::History => self.history_id,
        }
    }
    pub fn contains_focus(&self, focus: &FocusState) -> bool {
        focus.focused == Some(self.id())
            || focus.focused == Some(self.history_id)
            || self
                .commit_editor
                .as_ref()
                .is_some_and(|view| focus.focused == Some(view.input.id))
    }
    pub fn has_modal(&self) -> bool {
        self.menu.is_some()
    }
    pub fn commit_files(&self, commit: &str) -> Option<&Listing<PathBuf>> {
        self.files.get(commit)
    }
    pub fn has_capture(&self, focus: &FocusState) -> bool {
        self.divider_drag.is_some()
            || self
                .commit_editor
                .as_ref()
                .is_some_and(|view| view.input.is_dragging(focus))
    }
    pub fn has_popup(&self) -> bool {
        self.commit_editor.as_ref().is_some_and(|view| {
            !view.completion.entries.is_empty()
                || view.navigation.definition.is_some()
                || view.navigation.usages.is_some()
        })
    }
    pub fn dismiss_popups(&mut self) {
        if let Some(view) = &mut self.commit_editor {
            view.completion.dismiss();
            view.navigation.dismiss();
        }
    }
    pub fn checkout_pending(&self) -> bool {
        self.checkout_pending
    }
    pub fn deactivate(
        &mut self,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<()> {
        self.dismiss_popups();
        self.commit_arm = false;
        self.divider_drag = None;
        if focus.captured == Some(self.id()) {
            focus.captured = None;
        }
        if let Some(view) = &mut self.commit_editor {
            view.dispatch(
                &crate::input::terminal::InputEvent::Focus(false),
                documents,
                focus,
                self.geometry.editor,
            )?;
            view.dispatch(
                &crate::input::terminal::InputEvent::Focus(true),
                documents,
                focus,
                self.geometry.editor,
            )?;
        }
        Ok(())
    }
    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }
    fn can_write(&self) -> bool {
        self.repo_available == Some(true)
            && self.write_busy.is_none()
            && self.read.busy.is_none()
            && self.discovery.is_none()
            && !self.stale
            && self.read.error.is_none()
    }
    pub fn take_work(&mut self) -> Vec<GitWork> {
        self.start_reads();
        if let Some(view) = &mut self.commit_editor {
            let owner = view.owner();
            self.work.extend(
                view.take_work()
                    .into_iter()
                    .map(|work| GitWork::Editor { owner, work }),
            );
        }
        self.work.drain(..).collect()
    }
    fn start_reads(&mut self) {
        if self.write_busy.is_some() {
            return;
        }
        if self.refresh_requested && self.read.busy.is_none() && self.discovery.is_none() {
            self.refresh_requested = false;
            self.last_refresh = self.clock.now_millis();
            if self.repo_available == Some(true) {
                if let Some((tag, job)) = self.read.begin_refresh() {
                    self.work.push_back(GitWork::Read {
                        tag,
                        kind: ReadKind::Snapshot,
                        job,
                    });
                }
            } else {
                let tag = self.gate.request("git-discover".into(), None);
                self.discovery = Some(tag);
                self.work.push_back(GitWork::Read {
                    tag,
                    kind: ReadKind::Discover,
                    job: ReadJob::new(self.read.root.clone(), ReadRequest::Discover),
                });
            }
        }
        if self.pending_files.is_none()
            && let Some(commit) = self.waiting_files.pop_front()
        {
            let tag = self.gate.request("git-files".into(), None);
            self.pending_files = Some((commit.clone(), tag));
            let mut job = ReadJob::new(
                self.read.root.clone(),
                ReadRequest::Files {
                    revision: commit.clone(),
                },
            );
            let used: usize = self.files.values().map(|files| files.entries.len()).sum();
            job.limits.entries = 20_000usize.saturating_sub(used);
            self.work.push_back(GitWork::Read {
                tag,
                kind: ReadKind::Files(commit.clone()),
                job,
            });
        }
    }
    pub fn complete_read(
        &mut self,
        tag: JobTag,
        kind: ReadKind,
        result: Result<ReadOutput>,
    ) -> bool {
        match kind {
            ReadKind::Snapshot => {
                let failure = result.is_err();
                if !self.read.complete_refresh(tag, result) {
                    return false;
                }
                self.stale = failure;
                if failure {
                    self.repo_available = None;
                }
                if let Some(snapshot) = &self.read.snapshot {
                    let hashes: HashSet<_> = snapshot
                        .commits
                        .entries
                        .iter()
                        .map(|commit| commit.hash.clone())
                        .collect();
                    self.expanded.retain(|hash| hashes.contains(hash));
                    self.files.retain(|hash, _| hashes.contains(hash));
                    if self
                        .selected_commit
                        .as_ref()
                        .is_some_and(|hash| !hashes.contains(hash))
                    {
                        self.selected_commit = None;
                    }
                    self.history_selected = self
                        .history_selected
                        .min(self.history_rows().len().saturating_sub(1));
                }
            }
            ReadKind::Discover => {
                if self.discovery != Some(tag) || !self.gate.accepts("git-discover", tag) {
                    return false;
                }
                self.discovery = None;
                match result {
                    Ok(ReadOutput::Location(Some(_))) => {
                        self.repo_available = Some(true);
                        self.stale = false;
                        self.request_refresh();
                    }
                    Ok(ReadOutput::Location(None)) => {
                        self.repo_available = Some(false);
                        self.read.error = None;
                        self.read.snapshot = None;
                        self.stale = false;
                    }
                    Err(error) => {
                        self.read.error = Some(error.to_string());
                        self.stale = true;
                    }
                    _ => {
                        self.read.error = Some("Wrong Git discovery result".into());
                        self.stale = true;
                    }
                }
            }
            ReadKind::Files(commit) => {
                if self.pending_files.as_ref() != Some(&(commit.clone(), tag))
                    || !self.gate.accepts("git-files", tag)
                {
                    return false;
                }
                self.pending_files = None;
                match result {
                    Ok(ReadOutput::Files(mut files)) => {
                        if self.expanded.contains(&commit) {
                            let used: usize =
                                self.files.values().map(|files| files.entries.len()).sum();
                            let limit = 20_000usize.saturating_sub(used);
                            if files.entries.len() > limit {
                                files.entries.truncate(limit);
                                files.truncated = true;
                            }
                            self.files.insert(commit, files);
                        }
                    }
                    Err(DomainError::Cancelled) => {}
                    Err(error) => self.message = error.to_string(),
                    _ => self.message = "Wrong Git file listing result".into(),
                }
            }
            ReadKind::Diff => {
                if self.diff_tag != Some(tag) || !self.gate.accepts("git-diff", tag) {
                    return false;
                }
                self.diff_tag = None;
                match result {
                    Err(DomainError::Cancelled) => {}
                    Err(error) => self.message = error.to_string(),
                    _ => self.message = "Wrong Git diff result".into(),
                }
            }
        }
        self.start_reads();
        true
    }
    fn queue_write(&mut self, command: GitCommand) {
        if self.write_busy.is_some()
            || (!matches!(command, GitCommand::Init) && !self.can_write())
            || matches!(command, GitCommand::Init)
                && (self.repo_available != Some(false) || self.discovery.is_some() || self.stale)
        {
            return;
        }
        let tag = self.gate.request("git-write".into(), None);
        self.pending_message = if let GitCommand::Commit(snapshot) = &command {
            Some((snapshot.id, snapshot.revision))
        } else {
            None
        };
        self.write_busy = Some(tag);
        self.checkout_pending = command.changes_worktree();
        self.message = "Working…".into();
        self.work.push_back(GitWork::Write {
            tag,
            root: self.read.root.clone(),
            command,
        });
    }
    pub fn complete_write(
        &mut self,
        tag: JobTag,
        result: Result<WriteReceipt>,
        documents: &mut EditorDocuments,
    ) -> Result<bool> {
        if self.write_busy != Some(tag) || !self.gate.accepts("git-write", tag) {
            return Ok(false);
        }
        self.write_busy = None;
        self.checkout_pending = false;
        match result {
            Ok(receipt) => {
                self.message = receipt.message;
                if !receipt.warnings.is_empty() {
                    self.message
                        .push_str(&format!(" | {}", receipt.warnings.join(" | ")));
                }
                self.repo_available = Some(true);
                if let Some((id, revision)) = self.pending_message.take()
                    && let Some(view) = &mut self.commit_editor
                {
                    let snapshot = documents.document(view.document_id())?.snapshot();
                    if (snapshot.id, snapshot.revision) == (id, revision) {
                        view.programmatic_replace(
                            TextRange::new(
                                kode_core::ByteOffset(0),
                                kode_core::ByteOffset(snapshot.len_bytes()),
                            )?,
                            "",
                            documents,
                            self.geometry.editor,
                        )?;
                    } else {
                        self.message.push_str(" | Current message kept");
                    }
                }
            }
            Err(error) => {
                self.message = error.to_string();
                self.pending_message = None;
            }
        }
        self.request_refresh();
        self.start_reads();
        Ok(true)
    }
    fn commit(&mut self, documents: &EditorDocuments) -> Result<()> {
        if let Some(view) = &self.commit_editor {
            self.queue_write(GitCommand::Commit(
                documents.document(view.document_id())?.snapshot(),
            ));
        }
        Ok(())
    }
    fn toggle_status(&mut self) {
        if let Some(entry) = self
            .read
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.status.entries.get(self.read.selected_status))
        {
            let command = if entry.staged() {
                GitCommand::Unstage(vec![entry.path.clone()])
            } else {
                GitCommand::Stage(vec![entry.path.clone()])
            };
            self.queue_write(command);
        }
    }
    fn history_rows(&self) -> Vec<HistoryRow> {
        let mut rows = vec![];
        if let Some(snapshot) = &self.read.snapshot {
            for commit in &snapshot.commits.entries {
                rows.push(HistoryRow {
                    commit: commit.hash.clone(),
                    path: None,
                });
                if self.expanded.contains(&commit.hash)
                    && let Some(files) = self.files.get(&commit.hash)
                {
                    rows.extend(
                        files
                            .entries
                            .iter()
                            .take(20_000usize.saturating_sub(rows.len()))
                            .map(|path| HistoryRow {
                                commit: commit.hash.clone(),
                                path: Some(path.clone()),
                            }),
                    );
                }
                if rows.len() >= 20_000 {
                    break;
                }
            }
        }
        rows
    }
    fn toggle_commit(&mut self, commit: String) {
        if self.expanded.remove(&commit) {
            self.files.remove(&commit);
            self.waiting_files.retain(|waiting| waiting != &commit);
            return;
        }
        if self.expanded.len() >= 32 {
            self.message = "Close an expanded commit before opening another".into();
            return;
        }
        self.expanded.insert(commit.clone());
        if !self.files.contains_key(&commit)
            && !self.waiting_files.contains(&commit)
            && self
                .pending_files
                .as_ref()
                .is_none_or(|(pending, _)| pending != &commit)
        {
            self.waiting_files.push_back(commit);
            self.start_reads();
        }
    }
    pub fn queue_diff(&mut self, request: ReadRequest) {
        let tag = self.gate.request("git-diff".into(), None);
        self.diff_tag = Some(tag);
        self.work.push_back(GitWork::Read {
            tag,
            kind: ReadKind::Diff,
            job: ReadJob::new(self.read.root.clone(), request),
        });
    }
    pub fn cancel_diff(&mut self) {
        self.diff_tag = None;
        self.gate.dismiss("git-diff");
    }
    pub fn complete_diff(
        &mut self,
        tag: JobTag,
        result: Result<Arc<kode_git::diff::DiffPlan>>,
    ) -> bool {
        if self.diff_tag != Some(tag) || !self.gate.accepts("git-diff", tag) {
            return false;
        }
        self.diff_tag = None;
        match result {
            Ok(plan) => {
                self.latest_diff = Some(plan);
                true
            }
            Err(DomainError::Cancelled) => false,
            Err(error) => {
                self.message = error.to_string();
                false
            }
        }
    }
    pub fn tick(
        &mut self,
        documents: &EditorDocuments,
        focus: &FocusState,
        active: bool,
    ) -> Result<()> {
        if let Some(view) = &mut self.commit_editor {
            view.tick(documents, focus, self.geometry.editor)?;
        }
        if active && self.clock.now_millis().saturating_sub(self.last_refresh) >= 4_000 {
            self.request_refresh();
        }
        if active {
            self.start_reads();
        }
        Ok(())
    }
    pub fn complete_clipboard(
        &mut self,
        tag: JobTag,
        result: Result<ClipboardResult>,
        focus: &FocusState,
    ) {
        let Some(paste) = self.paste.take() else {
            return;
        };
        if paste.tag != tag {
            self.paste = Some(paste);
            return;
        }
        if self.modal_id != Some(paste.modal) || !self.contains_focus(focus) {
            return;
        }
        if let Some(Menu::Tag { dialog, .. }) = &mut self.menu
            && (
                dialog.field.value.as_str(),
                dialog.field.cursor,
                dialog.field.anchor,
            ) == (paste.value.as_str(), paste.cursor, paste.anchor)
        {
            match result {
                Ok(ClipboardResult::Read(text)) => {
                    dialog
                        .field
                        .handle(&crate::input::terminal::InputEvent::Paste(text));
                }
                Err(error) => self.message = error.to_string(),
                _ => {}
            }
        }
    }
}
