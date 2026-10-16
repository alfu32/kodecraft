mod actions;
pub mod choice_dialog;
pub mod entry_dialog;
mod input;
mod render;
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::InputEvent,
    theme::Theme,
    widgets::{
        dialog::TextDialog,
        field::{FieldAction, TextField},
    },
};
use choice_dialog::ChoiceDialog;
use kode_core::{RequestId, Result};
use kode_rest::{
    import::{Import, Kind, Notice},
    model::{NodeKind, NodePath, TreeNode, Workspace},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Open(NodePath),
    Toggle(NodePath),
    Menu(NodePath),
    AddRequest(NodePath),
    AddFolder(NodePath),
    Run(NodePath),
    Rename(NodePath),
    Copy(NodePath),
    MoveUp(NodePath),
    MoveDown(NodePath),
    MoveTo(NodePath),
    MoveInto { from: NodePath, to: NodePath },
    Delete(NodePath),
    DeleteConfirmed(NodePath),
    ImportPostman,
    ImportPostmanConfirmed,
    ImportOpenApi(NodePath),
    Export,
    Notices,
    NewEnvironment,
    CopyEnvironment(String),
    RenameEnvironment(String),
    ActivateEnvironment(String),
    RemoveEnvironment(String),
    OpenEnvironment(String),
    EnvironmentMenu(String),
}
pub enum Intent {
    TreeChanged(TreeChange),
    Open(NodePath),
    Environment(String),
    CopiedEnvironment {
        source: String,
        target: String,
    },
    EnvironmentRemoved(String),
    Run(NodePath),
    InspectNotices,
    Import {
        nonce: RequestId,
        kind: Kind,
        path: PathBuf,
        parent: NodePath,
        expected: Arc<Value>,
    },
    Export {
        nonce: RequestId,
        path: PathBuf,
        collection: Arc<Value>,
    },
    Clipboard {
        action: FieldAction,
        proof: Option<PasteProof>,
    },
}
#[derive(Clone)]
pub enum TreeChange {
    Inserted(NodePath),
    Removed(NodePath),
    Moved { from: NodePath, to: NodePath },
    Replaced,
}
impl TreeChange {
    pub fn rebase(&self, path: &NodePath) -> Option<NodePath> {
        match self {
            Self::Inserted(inserted) => Some(path.after_insert(inserted)),
            Self::Removed(removed) => path.after_remove(removed),
            Self::Moved { from, to } if path.starts_with(from) => {
                let mut moved = to.clone();
                moved.0.extend_from_slice(&path.0[from.0.len()..]);
                Some(moved)
            }
            Self::Moved { from, to } => path.after_remove(from).map(|path| path.after_insert(to)),
            Self::Replaced => None,
        }
    }
}

#[derive(Clone)]
pub struct PasteProof {
    nonce: Option<RequestId>,
    epoch: u64,
    field: String,
    cursor: usize,
    anchor: Option<usize>,
}
#[derive(Clone)]
enum Purpose {
    Rename(NodePath),
    Import(Kind, NodePath),
    Export,
    NewEnvironment,
    CopyEnvironment(String),
    RenameEnvironment(String),
}
enum Dialog {
    Input {
        nonce: RequestId,
        purpose: Purpose,
        dialog: TextDialog,
    },
    Choice(ChoiceDialog<Action>),
    Confirm {
        action: Action,
        dialog: TextDialog,
    },
}
struct Modal {
    dialog: Dialog,
    expected: Arc<Value>,
    previous: Option<FocusId>,
}
#[derive(Default, Clone, Copy)]
pub struct Geometry {
    pub area: Rect,
    pub filter: Rect,
    pub tree: Rect,
    pub divider: Rect,
    pub environments: Rect,
}
#[derive(Clone)]
struct Hit {
    area: Rect,
    action: Action,
}
pub struct Sidebar {
    pub id: FocusId,
    pub cursor: NodePath,
    pub environment: Option<String>,
    pub filtering: bool,
    pub filter: TextField,
    pub message: String,
    pub geometry: Geometry,
    pub visible: Vec<TreeNode>,
    pub notices: Vec<Notice>,
    expanded: BTreeSet<NodePath>,
    scroll: usize,
    environment_scroll: usize,
    environment_focus: bool,
    follow_cursor: bool,
    follow_environment: bool,
    clipboard_epoch: u64,
    dragging: bool,
    hits: Vec<Hit>,
    modal: Option<Modal>,
    cache: Option<Arc<Value>>,
    dirty: bool,
    intents: Vec<Intent>,
    pending_file: Option<RequestId>,
}
impl Sidebar {
    pub fn new(model: &Workspace) -> Result<Self> {
        let mut filter = TextField::new(String::new());
        filter.max_chars = 1024;
        let cursor = model.selected_path();
        let mut expanded = BTreeSet::from([NodePath::default()]);
        for length in 1..cursor.0.len() {
            expanded.insert(NodePath(cursor.0[..length].to_vec()));
        }
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            cursor,
            environment: model
                .active_environment()
                .map(|environment| environment.id.clone()),
            filtering: false,
            filter,
            message: String::new(),
            geometry: Geometry::default(),
            visible: vec![],
            notices: vec![],
            expanded,
            scroll: 0,
            environment_scroll: 0,
            environment_focus: false,
            follow_cursor: true,
            follow_environment: true,
            clipboard_epoch: 0,
            dragging: false,
            hits: vec![],
            modal: None,
            cache: None,
            dirty: true,
            intents: vec![],
            pending_file: None,
        })
    }
    pub fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
    pub fn dismiss(&mut self, focus: &mut FocusState) {
        self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
        if let Some(modal) = self.modal.take() {
            focus.focused = modal.previous;
        }
        self.dragging = false;
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
    }
    fn expand_ancestors(&mut self, path: &NodePath) {
        for length in 0..path.0.len() {
            self.expanded.insert(NodePath(path.0[..length].to_vec()));
        }
        self.dirty = true;
    }
    fn inserted(&mut self, path: &NodePath) {
        self.intents
            .push(Intent::TreeChanged(TreeChange::Inserted(path.clone())));
        self.expanded = self
            .expanded
            .iter()
            .map(|item| item.after_insert(path))
            .collect();
        self.cursor = self.cursor.after_insert(path);
        self.dirty = true;
    }
    fn removed(&mut self, path: &NodePath, model: &Workspace) {
        self.intents
            .push(Intent::TreeChanged(TreeChange::Removed(path.clone())));
        self.expanded = self
            .expanded
            .iter()
            .filter_map(|item| item.after_remove(path))
            .collect();
        self.cursor = self
            .cursor
            .after_remove(path)
            .unwrap_or_else(|| model.selected_path());
        self.dirty = true;
    }
    fn moved(&mut self, from: &NodePath, to: &NodePath) {
        self.intents.push(Intent::TreeChanged(TreeChange::Moved {
            from: from.clone(),
            to: to.clone(),
        }));
        let rebase = |path: &NodePath| {
            if path.starts_with(from) {
                let mut moved = to.clone();
                moved.0.extend_from_slice(&path.0[from.0.len()..]);
                moved
            } else {
                path.after_remove(from)
                    .expect("outside removed subtree")
                    .after_insert(to)
            }
        };
        self.expanded = self.expanded.iter().map(&rebase).collect();
        self.cursor = rebase(&self.cursor);
        self.expand_ancestors(to);
    }
    fn refresh(&mut self, model: &Workspace) {
        let collection = model.export_collection();
        if !self.dirty
            && self
                .cache
                .as_ref()
                .is_some_and(|cache| Arc::ptr_eq(cache, &collection))
        {
            return;
        }
        let all = model.tree();
        let query = self.filter.value.trim().to_lowercase();
        self.visible = if query.is_empty() {
            all.into_iter()
                .filter(|node| {
                    (0..node.path.0.len()).all(|depth| {
                        self.expanded
                            .contains(&NodePath(node.path.0[..depth].to_vec()))
                    })
                })
                .collect()
        } else {
            let mut matching = BTreeSet::new();
            for node in &all {
                let url = model
                    .node(&node.path)
                    .and_then(|node| node.get("request"))
                    .and_then(|request| request.get("url"))
                    .map(Value::to_string)
                    .unwrap_or_default();
                if node.name.to_lowercase().contains(&query)
                    || node
                        .method
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                    || url.to_lowercase().contains(&query)
                {
                    for length in 0..=node.path.0.len() {
                        matching.insert(NodePath(node.path.0[..length].to_vec()));
                    }
                }
            }
            all.into_iter()
                .filter(|node| matching.contains(&node.path))
                .collect()
        };
        if !self.visible.iter().any(|node| node.path == self.cursor) {
            self.cursor = self
                .visible
                .first()
                .map(|node| node.path.clone())
                .unwrap_or_default();
        }
        self.cache = Some(collection);
        self.dirty = false;
    }
    pub fn import_finished(
        &mut self,
        nonce: RequestId,
        expected: &Arc<Value>,
        imported: Import,
        parent: &NodePath,
        model: &mut Workspace,
    ) -> Result<bool> {
        if self.pending_file != Some(nonce) {
            return Ok(false);
        }
        self.pending_file = None;
        if !Arc::ptr_eq(expected, &model.export_collection()) {
            self.message =
                "Collection changed during import; retry against the current folder".into();
            return Ok(false);
        }
        let kind = imported.kind;
        let paths = match imported.apply(model, parent) {
            Ok(paths) => paths,
            Err(error) => {
                self.message = error.to_string();
                return Err(error);
            }
        };
        if kind == Kind::Postman && parent.is_root() {
            self.intents.push(Intent::TreeChanged(TreeChange::Replaced));
            self.expanded = BTreeSet::from([NodePath::default()]);
            self.cursor = NodePath::default();
        } else {
            for path in paths {
                // Imports append to this parent. Existing indices and runtime
                // item keys cannot shift; avoid rebasing a large run's keys for
                // every imported sibling.
                self.expand_ancestors(&path);
            }
        }
        self.notices = imported.notices;
        self.message = format!(
            "Imported collection; {} conversion notice(s)",
            self.notices.len()
        );
        self.cache = None;
        self.dirty = true;
        Ok(true)
    }
    pub fn file_finished(&mut self, nonce: RequestId, message: String) {
        if self.pending_file == Some(nonce) {
            self.pending_file = None;
            self.message = message;
        }
    }
    pub fn paste(&mut self, proof: PasteProof, text: String, focus: &FocusState) {
        if proof.epoch != self.clipboard_epoch || focus.focused != Some(self.id) {
            return;
        }
        let field = if let Some(nonce) = proof.nonce {
            match self.modal.as_mut().map(|modal| &mut modal.dialog) {
                Some(Dialog::Input {
                    nonce: current,
                    dialog,
                    ..
                }) if *current == nonce && dialog.focus == 0 => Some(&mut dialog.field),
                _ => None,
            }
        } else if self.filtering && self.modal.is_none() {
            Some(&mut self.filter)
        } else {
            None
        };
        if let Some(field) = field
            && field.value == proof.field
            && field.cursor == proof.cursor
            && field.anchor == proof.anchor
        {
            if field.handle(&InputEvent::Paste(text)) == Some(FieldAction::LimitReached) {
                self.message = "Field character limit reached".into();
            }
            self.dirty = true;
        }
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        model: &Workspace,
        focused: bool,
    ) -> Option<Position> {
        self.render_panel(area, buffer, theme, model, focused)
    }
    pub fn render_modal(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Option<Position> {
        match self.modal.as_mut().map(|modal| &mut modal.dialog) {
            Some(Dialog::Input { dialog, .. }) | Some(Dialog::Confirm { dialog, .. }) => {
                dialog.render(area, buffer, theme)
            }
            Some(Dialog::Choice(dialog)) => {
                dialog.render(area, buffer, theme);
                None
            }
            None => None,
        }
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<bool> {
        self.dispatch_input(event, model, focus)
    }
}
