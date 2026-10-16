//! Request controls and dialogs. The application owns embedded editor buffers,
//! workers and execution; this reducer edits only the selected model subtree.
mod input;
mod render;
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::InputEvent,
    panels::rest::sidebar::{
        TreeChange,
        entry_dialog::{EntryDialog, PasteProof as EntryProof},
    },
    theme::Theme,
    widgets::field::{FieldAction, TextField},
};
use kode_core::{DomainError, RequestId, Result};
use kode_rest::{
    model::{
        NodePath, Workspace,
        editing::{self, Field, Rows},
    },
    resolve::{Auth, HeaderValue, Variable},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Overview,
    Params,
    Auth,
    Headers,
    Body,
    Variables,
    Settings,
    Raw,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BufferKind {
    Headers,
    RawBody,
    GraphqlQuery,
    GraphqlVariables,
    RawRequest,
}
#[derive(Clone, PartialEq, Eq)]
pub enum Action {
    Focus(Field),
    Tab(Tab),
    Send,
    Reveal,
    Auth(Option<&'static str>),
    Body(&'static str),
    Setting(&'static str),
    Cookies,
    Notices,
    Acknowledge,
    Buffer(BufferKind),
    Add(Rows),
    Edit(Rows, usize),
    Toggle(Rows, usize),
    Remove(Rows, usize),
}
pub enum Intent {
    Send,
    ClearCookies,
    Notices,
    Buffer(BufferKind),
    Changed,
    Clipboard {
        action: FieldAction,
        proof: Option<PasteProof>,
    },
}
#[derive(Clone)]
pub enum PasteTarget {
    Field {
        field: Field,
        value: String,
        cursor: usize,
        anchor: Option<usize>,
    },
    Entry(EntryProof),
}
#[derive(Clone)]
pub struct PasteProof {
    key: String,
    epoch: u64,
    target: PasteTarget,
}
pub struct Prepared {
    pub fields: BTreeMap<Field, String>,
    pub request: bool,
}
impl Prepared {
    /// Blocking preparation: do not serialize structured descriptions on render.
    pub fn from_model(model: &Workspace, path: &NodePath) -> Result<Self> {
        let node = model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?;
        let request = editing::is_request(node, path);
        let mut fields = BTreeMap::new();
        fields.insert(
            Field::Name,
            if path.is_root() {
                editing::text(node.get("info").and_then(|info| info.get("name")))
            } else {
                editing::text(node.get("name"))
            },
        );
        fields.insert(
            Field::Description,
            editing::description(if path.is_root() {
                node.get("info").and_then(|info| info.get("description"))
            } else {
                node.get("description")
            }),
        );
        let request_value = node.get("request");
        fields.insert(
            Field::Method,
            request_value
                .and_then(|request| request.get("method"))
                .and_then(Value::as_str)
                .unwrap_or("GET")
                .into(),
        );
        fields.insert(
            Field::Url,
            request_value
                .map(|request| {
                    if let Some(url) = request.as_str() {
                        url.into()
                    } else {
                        request.get("url").map(editing::raw_url).unwrap_or_default()
                    }
                })
                .unwrap_or_default(),
        );
        fields.insert(
            Field::File,
            editing::text(request_value.and_then(|request| request.pointer("/body/file/src"))),
        );
        fields.insert(
            Field::Timeout,
            node.pointer("/protocolProfileBehavior/requestTimeout")
                .map(|value| editing::text(Some(value)))
                .unwrap_or_else(|| "30000".into()),
        );
        Ok(Self { fields, request })
    }
}
#[derive(Default)]
pub struct Derived {
    pub auth: Option<Auth>,
    pub headers: Vec<HeaderValue>,
    pub variables: Vec<Variable>,
    pub error: Option<String>,
}
enum Row {
    Info(String),
    Field(&'static str, Field),
    Buttons(Vec<(String, Action)>),
    Entry {
        kind: Rows,
        index: usize,
        label: String,
        enabled: bool,
    },
    Embed(&'static str, BufferKind),
}
#[derive(Clone)]
struct Hit {
    area: Rect,
    action: Action,
}
struct RowCache {
    collection: Weak<Value>,
    tab: Tab,
    reveal: bool,
    rows: Arc<Vec<Row>>,
}
struct Modal {
    dialog: EntryDialog,
    kind: Rows,
    index: usize,
    expected: Arc<Value>,
    previous: Option<FocusId>,
}
#[derive(Clone, Default)]
pub struct Geometry {
    pub area: Rect,
    pub content: Rect,
    pub editor: Rect,
    pub tabs: Vec<(Tab, Rect)>,
}
pub struct Form {
    pub id: FocusId,
    pub key: String,
    pub path: NodePath,
    pub tab: Tab,
    pub fields: BTreeMap<Field, TextField>,
    pub geometry: Geometry,
    derived: Derived,
    pub message: String,
    pub closed: bool,
    pub reveal: bool,
    request: bool,
    focused: Option<Field>,
    selection: usize,
    scroll: usize,
    follow: bool,
    epoch: u64,
    hits: Vec<Hit>,
    controls: Vec<(usize, Action)>,
    modal: Option<Modal>,
    intents: Vec<Intent>,
    row_cache: Option<RowCache>,
}
impl Form {
    pub fn new(path: NodePath, key: String, prepared: Prepared) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            key,
            path,
            tab: if prepared.request {
                Tab::Params
            } else {
                Tab::Overview
            },
            fields: prepared
                .fields
                .into_iter()
                .map(|(field, text)| (field, TextField::new(text)))
                .collect(),
            geometry: Geometry::default(),
            derived: Derived::default(),
            message: String::new(),
            closed: false,
            reveal: false,
            request: prepared.request,
            focused: None,
            selection: 0,
            scroll: 0,
            follow: true,
            epoch: 0,
            hits: Vec::new(),
            controls: Vec::new(),
            modal: None,
            intents: Vec::new(),
            row_cache: None,
        })
    }
    pub fn tabs(&self) -> &'static [Tab] {
        if self.request {
            &[
                Tab::Params,
                Tab::Auth,
                Tab::Headers,
                Tab::Body,
                Tab::Variables,
                Tab::Settings,
                Tab::Raw,
            ]
        } else {
            &[Tab::Overview, Tab::Auth, Tab::Headers, Tab::Variables]
        }
    }
    pub fn set_derived(&mut self, derived: Derived) {
        self.derived = derived;
        self.row_cache = None;
    }
    pub fn derived(&self) -> &Derived {
        &self.derived
    }
    pub fn refresh_fields(&mut self, prepared: Prepared) {
        for (field, value) in prepared.fields {
            if self.focused != Some(field)
                && self.fields.get(&field).is_none_or(|old| old.value != value)
            {
                self.fields.insert(field, TextField::new(value));
            }
        }
        self.request = prepared.request;
        if !self.tabs().contains(&self.tab) {
            self.tab = if self.request {
                Tab::Params
            } else {
                Tab::Overview
            };
        }
        self.row_cache = None;
    }
    fn cached_rows(&mut self, model: &Workspace) -> Result<Arc<Vec<Row>>> {
        let collection = Arc::downgrade(&model.export_collection());
        if let Some(cache) = &self.row_cache
            && Weak::ptr_eq(&cache.collection, &collection)
            && cache.tab == self.tab
            && cache.reveal == self.reveal
        {
            return Ok(cache.rows.clone());
        }
        let rows = Arc::new(self.rows(model)?);
        self.row_cache = Some(RowCache {
            collection,
            tab: self.tab,
            reveal: self.reveal,
            rows: rows.clone(),
        });
        Ok(rows)
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
    pub fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }
    pub fn invalidate(&mut self, focus: &mut FocusState) {
        self.epoch = self.epoch.saturating_add(1);
        self.focused = None;
        if let Some(modal) = self.modal.take() {
            focus.focused = modal.previous;
        }
    }
    pub fn rebase(&mut self, change: &TreeChange, focus: &mut FocusState) {
        let path = match change {
            TreeChange::Inserted(path) => Some(self.path.after_insert(path)),
            TreeChange::Removed(path) => self.path.after_remove(path),
            TreeChange::Moved { from, to } if self.path.starts_with(from) => {
                let mut path = to.clone();
                path.0.extend_from_slice(&self.path.0[from.0.len()..]);
                Some(path)
            }
            TreeChange::Moved { from, to } => self
                .path
                .after_remove(from)
                .map(|path| path.after_insert(to)),
            TreeChange::Replaced => None,
        };
        if let Some(path) = path {
            self.path = path;
        } else {
            self.invalidate(focus);
            self.closed = true;
        }
    }
    fn mask(&self, key: &str, value: &str, secret: bool) -> String {
        if !self.reveal
            && (secret
                || ["token", "password", "secret"]
                    .iter()
                    .any(|word| key.to_ascii_lowercase().contains(word)))
        {
            "********".into()
        } else {
            value.into()
        }
    }
    fn rows(&self, model: &Workspace) -> Result<Vec<Row>> {
        let node = model
            .node(&self.path)
            .ok_or_else(|| invalid("REST item no longer exists"))?;
        let mut rows = Vec::new();
        if node.get("x-kode-import-incomplete") == Some(&Value::Bool(true)) {
            rows.push(Row::Info(
                "Import incomplete: review notices and correct this request before sending".into(),
            ));
            rows.push(Row::Buttons(vec![
                ("Import notices".into(), Action::Notices),
                ("Reviewed: allow execution".into(), Action::Acknowledge),
            ]));
        }
        if let Some(error) = &self.derived.error {
            rows.push(Row::Info(error.clone()));
        }
        match self.tab {
            Tab::Overview => {
                rows.push(Row::Field("Name", Field::Name));
                rows.push(Row::Field("Description", Field::Description));
                rows.push(Row::Info(if self.path.is_root() {
                    "Postman collection v2.1".into()
                } else {
                    format!(
                        "Folder items: {}",
                        node.get("item")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len)
                    )
                }));
            }
            Tab::Params => {
                rows.push(Row::Info(
                    "Query parameters: toggle sent/disabled rows".into(),
                ));
                self.entry_rows(model, Rows::Query, &mut rows)?;
                rows.push(Row::Buttons(vec![(
                    "+ Query".into(),
                    Action::Add(Rows::Query),
                )]));
                rows.push(Row::Info("Path variables".into()));
                self.entry_rows(model, Rows::Path, &mut rows)?;
                rows.push(Row::Buttons(vec![(
                    "+ Path variable".into(),
                    Action::Add(Rows::Path),
                )]));
            }
            Tab::Auth => {
                let own = if self.request {
                    node.get("request").and_then(|request| request.get("auth"))
                } else {
                    node.get("auth")
                };
                rows.push(Row::Info(format!(
                    "Auth: {}",
                    own.and_then(|auth| auth.get("type"))
                        .and_then(Value::as_str)
                        .unwrap_or("inherit")
                )));
                rows.push(Row::Buttons(vec![
                    ("Inherit".into(), Action::Auth(None)),
                    ("No auth".into(), Action::Auth(Some("noauth"))),
                    ("Bearer".into(), Action::Auth(Some("bearer"))),
                    ("Basic".into(), Action::Auth(Some("basic"))),
                    ("API key".into(), Action::Auth(Some("apikey"))),
                ]));
                self.entry_rows(model, Rows::Auth, &mut rows)?;
                rows.push(Row::Buttons(vec![(
                    if self.reveal {
                        "Hide secrets"
                    } else {
                        "Reveal secrets"
                    }
                    .into(),
                    Action::Reveal,
                )]));
                if own.is_none()
                    && let Some(auth) = &self.derived.auth
                {
                    rows.push(Row::Info(format!(
                        "Effective: {} from {}",
                        auth.kind, auth.source
                    )));
                }
            }
            Tab::Headers => {
                rows.push(Row::Info("Inherited headers (read-only)".into()));
                for header in self
                    .derived
                    .headers
                    .iter()
                    .filter(|header| !header.editable)
                {
                    rows.push(Row::Info(format!(
                        "{} {}: {} ({})",
                        if header.disabled { "[off]" } else { "[on]" },
                        header.name,
                        header.value,
                        header.source
                    )));
                }
                rows.push(Row::Embed(
                    "Local headers: Name: Value; # disables a row",
                    BufferKind::Headers,
                ));
            }
            Tab::Body => {
                let mode = node
                    .pointer("/request/body/mode")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                rows.push(Row::Info(format!("Body mode: {mode}")));
                rows.push(Row::Buttons(
                    ["none", "raw", "urlencoded", "formdata", "file", "graphql"]
                        .into_iter()
                        .map(|mode| (mode.into(), Action::Body(mode)))
                        .collect(),
                ));
                match mode {
                    "raw" => rows.push(Row::Embed("Raw request body", BufferKind::RawBody)),
                    "graphql" => {
                        rows.push(Row::Buttons(vec![
                            ("Query".into(), Action::Buffer(BufferKind::GraphqlQuery)),
                            (
                                "Variables".into(),
                                Action::Buffer(BufferKind::GraphqlVariables),
                            ),
                        ]));
                        rows.push(Row::Embed(
                            "GraphQL query / variables",
                            BufferKind::GraphqlQuery,
                        ));
                    }
                    "file" => rows.push(Row::Field("File", Field::File)),
                    "urlencoded" | "formdata" => {
                        let kind = if mode == "formdata" {
                            Rows::FormData
                        } else {
                            Rows::UrlEncoded
                        };
                        self.entry_rows(model, kind, &mut rows)?;
                        rows.push(Row::Buttons(vec![("+ Field".into(), Action::Add(kind))]));
                    }
                    "none" => rows.push(Row::Info("No request body".into())),
                    _ => rows.push(Row::Info(
                        "Unsupported body mode: choose a supported mode before sending".into(),
                    )),
                }
            }
            Tab::Variables => {
                rows.push(Row::Buttons(vec![
                    ("+ Variable".into(), Action::Add(Rows::Variable)),
                    (
                        if self.reveal {
                            "Hide secrets"
                        } else {
                            "Reveal secrets"
                        }
                        .into(),
                        Action::Reveal,
                    ),
                ]));
                self.entry_rows(model, Rows::Variable, &mut rows)?;
                let locals = editing::rows(node, &self.path, Rows::Variable)?;
                for variable in &self.derived.variables {
                    if !locals.iter().any(|entry| {
                        entry.get("key").and_then(Value::as_str) == Some(variable.key.as_str())
                    }) {
                        rows.push(Row::Info(format!(
                            "{} = {} ({}, {})",
                            variable.key,
                            self.mask(&variable.key, &variable.value, variable.secret),
                            variable.kind,
                            variable.source
                        )));
                    }
                }
            }
            Tab::Settings => {
                let settings = node.get("protocolProfileBehavior");
                for (key, label) in [
                    ("disableRedirects", "Follow redirects"),
                    ("disableSslVerification", "TLS certificate validation"),
                ] {
                    let off = settings
                        .and_then(|settings| settings.get(key))
                        .is_some_and(|value| value == true || value == "true");
                    rows.push(Row::Buttons(vec![(
                        format!("{label}: {}", if off { "off" } else { "on" }),
                        Action::Setting(key),
                    )]));
                }
                rows.push(Row::Field("Timeout ms", Field::Timeout));
                rows.push(Row::Buttons(vec![(
                    "Clear cookies".into(),
                    Action::Cookies,
                )]));
                rows.push(Row::Info(
                    "Cookie jar belongs to this workspace and is not persisted".into(),
                ));
            }
            Tab::Raw => rows.push(Row::Embed(
                "Resolved logical request: variables, auth and headers applied",
                BufferKind::RawRequest,
            )),
        }
        Ok(rows)
    }
    fn entry_rows(&self, model: &Workspace, kind: Rows, rows: &mut Vec<Row>) -> Result<()> {
        let node = model
            .node(&self.path)
            .ok_or_else(|| invalid("REST item no longer exists"))?;
        for (index, entry) in editing::rows(node, &self.path, kind)?.iter().enumerate() {
            if !entry.is_object() {
                rows.push(Row::Info("Unrecognized row retained in document".into()));
                continue;
            }
            let value = editing::entry_value(entry, kind);
            let secret = entry.get("secret") == Some(&Value::Bool(true))
                || value.kind == "secret"
                || kind == Rows::Auth
                    && matches!(value.key.as_str(), "value" | "password" | "token");
            rows.push(Row::Entry {
                kind,
                index,
                label: format!(
                    "{} = {}{}",
                    value.key,
                    self.mask(&value.key, &value.value, secret),
                    if kind == Rows::Variable {
                        format!(" ({})", value.kind)
                    } else if kind == Rows::FormData {
                        format!(" [{}]", value.description)
                    } else {
                        String::new()
                    }
                ),
                enabled: value.enabled,
            });
        }
        Ok(())
    }
    pub fn perform(
        &mut self,
        action: Action,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<()> {
        self.activate(action, model, focus)
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<bool> {
        self.dispatch_input(event, model, focus)
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        model: &Workspace,
        focused: bool,
    ) -> Result<Option<Position>> {
        self.render_form(area, buffer, theme, model, focused)
    }
    pub fn render_modal(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Option<Position> {
        self.modal
            .as_mut()
            .and_then(|modal| modal.dialog.render(area, buffer, theme))
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST form",
        message: message.into(),
    }
}
