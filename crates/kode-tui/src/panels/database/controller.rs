//! Main-thread composition of the explorer and nested database dialogs.
use super::{
    connection_dialog::{self, ConnectionDialog, Draft},
    driver_install::{self, DriverInstallDialog},
    java_runtime::{self, JavaRuntimeDialog},
    object_menu::ObjectMenu,
    tree::{self, DatabaseTree, Load},
};
use crate::{
    focus::{EventTarget, FocusId, FocusState},
    input::terminal::{InputEvent, Key},
    theme::Theme,
    widgets::field::FieldAction,
};
use kode_core::{JobTag, RequestId, Result, WorkspaceGeneration};
use kode_database::{
    bridge::client::Result as BridgeResult,
    catalog::{DriverRegistry, UrlCatalog},
    connections::TestResult,
    drivers::{Installed, Progress},
    model::{
        DataSourceDefinition, DatabaseObject, DriverSpec, MetadataRequest, MetadataResult,
        MetadataSourceDocument,
    },
};
use kode_platform::jobs::CancellationToken;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::watch;

pub enum Operation {
    Initialize,
    Restart,
    Save(Draft),
    Delete(String),
    Connect(DataSourceDefinition),
    Close(String),
    Test(Draft),
    Metadata(Load),
    Source {
        source: String,
        object: DatabaseObject,
        request: MetadataRequest,
    },
    Resolve {
        spec: DriverSpec,
        progress: watch::Sender<Progress>,
    },
    JavaRuntime {
        progress: watch::Sender<kode_database::java_runtime::Progress>,
    },
    Recents(String),
}
pub struct Work {
    pub tag: JobTag,
    pub cancel: CancellationToken,
    pub operation: Operation,
}
pub enum Context {
    Initialize,
    Restart,
    Save,
    Delete,
    Connect,
    Close,
    Test,
    Metadata(Load),
    Source {
        source: String,
        object: DatabaseObject,
        request: MetadataRequest,
    },
    Resolve,
    JavaRuntime,
    Recents(String),
}
impl Work {
    pub fn context(&self) -> Context {
        match &self.operation {
            Operation::Initialize => Context::Initialize,
            Operation::Restart => Context::Restart,
            Operation::Save(_) => Context::Save,
            Operation::Delete(_) => Context::Delete,
            Operation::Connect(_) => Context::Connect,
            Operation::Close(_) => Context::Close,
            Operation::Test(_) => Context::Test,
            Operation::Metadata(load) => Context::Metadata(load.clone()),
            Operation::Source {
                source,
                object,
                request,
            } => Context::Source {
                source: source.clone(),
                object: object.clone(),
                request: request.clone(),
            },
            Operation::Resolve { .. } => Context::Resolve,
            Operation::JavaRuntime { .. } => Context::JavaRuntime,
            Operation::Recents(driver) => Context::Recents(driver.clone()),
        }
    }
}
pub enum Report {
    Restarted,
    Sources {
        sources: Vec<DataSourceDefinition>,
        warning: Option<String>,
    },
    Connected(Option<String>),
    Closed,
    Test {
        result: TestResult,
        warning: Option<String>,
    },
    Metadata(Arc<MetadataResult>),
    Source(Arc<kode_database::metadata::Source>),
    Installed(BridgeResult<Installed>),
    Recents(Vec<String>),
    JavaRuntime(String),
}
pub struct PasteProof {
    pub dialog: u64,
    pub field: usize,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
pub enum Intent {
    Browse {
        request: crate::widgets::file_picker::Request,
        proof: connection_dialog::PathProof,
    },
    Details {
        title: String,
        text: String,
    },
    Work(Box<Work>),
    Console {
        source: String,
        sql: String,
    },
    Source {
        title: String,
        documents: Vec<MetadataSourceDocument>,
    },
    OpenSession(String),
    RemoveSession(String),
    Clipboard {
        action: FieldAction,
        proof: Option<PasteProof>,
    },
}
pub struct DatabasePanel {
    pub tree: DatabaseTree,
    pub form: Option<ConnectionDialog>,
    pub install: Option<DriverInstallDialog>,
    pub java_runtime: Option<JavaRuntimeDialog>,
    pub java_required: bool,
    java_hit: Rect,
    pub menu: Option<ObjectMenu>,
    pub initialized: bool,
    registry: Arc<DriverRegistry>,
    urls: Arc<UrlCatalog>,
    intents: Vec<Intent>,
    pending: HashMap<JobTag, CancellationToken>,
    labels: HashMap<JobTag, String>,
    last_detail: Option<(String, String)>,
    form_id: u64,
    modal_id: FocusId,
    previous: Option<Option<FocusId>>,
    source_request: Option<JobTag>,
}
impl Drop for DatabasePanel {
    fn drop(&mut self) {
        for token in self.pending.values() {
            token.cancel();
        }
    }
}
impl DatabasePanel {
    pub fn new(generation: WorkspaceGeneration) -> Result<Self> {
        let registry = Arc::new(DriverRegistry::compiled()?);
        let urls = Arc::new(UrlCatalog::compiled(&registry)?);
        Ok(Self {
            tree: DatabaseTree::new(generation)?,
            form: None,
            install: None,
            java_runtime: None,
            java_required: false,
            java_hit: Rect::default(),
            menu: None,
            initialized: false,
            registry,
            urls,
            intents: vec![],
            pending: HashMap::new(),
            labels: HashMap::new(),
            last_detail: None,
            form_id: 0,
            modal_id: FocusId(RequestId::fresh()?.0),
            previous: None,
            source_request: None,
        })
    }
    pub fn configure(&mut self, registry: Arc<DriverRegistry>, urls: Arc<UrlCatalog>) {
        self.registry = registry;
        self.urls = urls;
    }
    pub fn has_modal(&self) -> bool {
        self.form.is_some()
            || self.install.is_some()
            || self.menu.is_some()
            || self
                .java_runtime
                .as_ref()
                .is_some_and(|dialog| !dialog.hidden)
    }
    pub fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }
    fn tag(&self) -> Result<JobTag> {
        Ok(JobTag {
            workspace: self.tree.generation,
            request: RequestId::fresh()?,
            document: None,
        })
    }
    fn work(&mut self, operation: Operation) -> Result<JobTag> {
        let tag = self.tag()?;
        let cancel = CancellationToken::new();
        let label = match &operation {
            Operation::Initialize => "Load datasource configuration".into(),
            Operation::Restart => "Restart JDBC helper".into(),
            Operation::Save(draft) => format!("Save datasource: {}", draft.definition.name),
            Operation::Test(draft) => format!("Test connection: {}", draft.definition.name),
            Operation::Connect(source) => format!("Connect: {}", source.name),
            Operation::Close(source) => format!("Disconnect: {source}"),
            Operation::Delete(source) => format!("Delete datasource: {source}"),
            Operation::Resolve { spec, .. } => {
                format!("Install JDBC driver: {}", spec.display_name)
            }
            Operation::Metadata(load) => format!("Metadata: {}", load.key.source()),
            Operation::Source { object, .. } => {
                format!("Package source: {}", object.qualified_name)
            }
            Operation::JavaRuntime { .. } => "Install optional Java runtime".into(),
            Operation::Recents(driver) => format!("JDBC URL suggestions: {driver}"),
        };
        self.labels.insert(tag, label);
        if matches!(&operation, Operation::Source { .. }) {
            if let Some(previous) = self.source_request.replace(tag)
                && let Some(token) = self.pending.get(&previous)
            {
                token.cancel();
            }
            self.tree.notice = "Loading package source… Esc cancels".into();
        }
        self.pending.insert(tag, cancel.clone());
        self.intents.push(Intent::Work(Box::new(Work {
            tag,
            cancel,
            operation,
        })));
        Ok(tag)
    }
    pub fn initialize(&mut self) -> Result<()> {
        if !self.initialized {
            self.initialized = true;
            self.work(Operation::Initialize)?;
        }
        Ok(())
    }
    fn focus_modal(&mut self, focus: &mut FocusState) {
        if self.previous.is_none() {
            self.previous = Some(focus.focused);
        }
        focus.focused = Some(self.modal_id);
        focus.captured = None;
    }
    fn restore_focus(&mut self, focus: &mut FocusState) {
        if !self.has_modal()
            && let Some(previous) = self.previous.take()
        {
            focus.focused = previous;
            focus.captured = None;
        }
    }
    pub fn edit(
        &mut self,
        existing: Option<DataSourceDefinition>,
        focus: &mut FocusState,
    ) -> Result<()> {
        self.form = Some(ConnectionDialog::new(
            self.registry.clone(),
            self.urls.clone(),
            existing,
        )?);
        self.form_id = RequestId::fresh()?.0;
        self.focus_modal(focus);
        Ok(())
    }
    pub fn paste(&mut self, proof: PasteProof, text: String) -> bool {
        if proof.dialog != self.form_id {
            return false;
        }
        let Some(form) = &mut self.form else {
            return false;
        };
        if form.focus != proof.field + 1 {
            return false;
        }
        let Some(field) = form.fields.get_mut(proof.field) else {
            return false;
        };
        if field.value != proof.value
            || field.cursor != proof.cursor
            || field.anchor != proof.anchor
        {
            return false;
        }
        field.handle(&InputEvent::Paste(text));
        true
    }
    fn field(&mut self, action: FieldAction) {
        match action {
            FieldAction::Copy(_) => self.intents.push(Intent::Clipboard {
                action,
                proof: None,
            }),
            FieldAction::PasteRequested => {
                if let Some(form) = &self.form
                    && form.focus > 0
                    && let Some(field) = form.fields.get(form.focus - 1)
                {
                    self.intents.push(Intent::Clipboard {
                        action,
                        proof: Some(PasteProof {
                            dialog: self.form_id,
                            field: form.focus - 1,
                            value: field.value.clone(),
                            cursor: field.cursor,
                            anchor: field.anchor,
                        }),
                    });
                }
            }
            FieldAction::LimitReached => {
                if let Some(form) = &mut self.form {
                    form.status = "Field size limit reached".into();
                }
            }
            FieldAction::Changed => {}
        }
    }
    fn details(&mut self) {
        let detail = if let Some(dialog) = &self.java_runtime
            && !dialog.hidden
        {
            Some(("Optional Java runtime".into(), dialog.message.clone()))
        } else if let Some(install) = &self.install {
            Some((
                format!(
                    "Install JDBC driver: {} / request {}",
                    install.driver_name, install.tag.request.0
                ),
                install.message.clone(),
            ))
        } else if let Some(form) = &self.form {
            self.last_detail
                .as_ref()
                .filter(|(_, text)| text == &form.status)
                .cloned()
                .or_else(|| {
                    Some((
                        format!("Datasource: {}", form.fields[0].value),
                        form.status.clone(),
                    ))
                })
        } else {
            self.tree
                .diagnostic()
                .map(|(title, text)| {
                    self.last_detail
                        .as_ref()
                        .filter(|(_, last)| last == &text)
                        .cloned()
                        .unwrap_or((title, text))
                })
                .or_else(|| self.last_detail.clone())
                .or_else(|| {
                    (!self.tree.notice.is_empty())
                        .then(|| ("Database service".into(), self.tree.notice.clone()))
                })
        };
        if let Some((title, text)) = detail {
            self.intents.push(Intent::Details { title, text });
        }
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<()> {
        if self.source_request.is_some()
            && !self.has_modal()
            && matches!(event,InputEvent::Key(input) if input.key==Key::Escape && !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta)
        {
            if let Some(tag) = self.source_request.take()
                && let Some(token) = self.pending.get(&tag)
            {
                token.cancel();
            }
            self.tree.notice = "Package source request cancelled".into();
            return Ok(());
        }
        if matches!(event,InputEvent::Key(input) if input.key==Key::Function(4) && !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta)
        {
            self.details();
            return Ok(());
        }

        let java_visible = self
            .java_runtime
            .as_ref()
            .is_some_and(|dialog| !dialog.hidden);
        if !java_visible
            && self.install.is_none()
            && self.menu.is_none()
            && (self.java_required || self.java_runtime.is_some())
            && (matches!(event, InputEvent::Key(input) if input.key == Key::Function(5) && crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Database, *input).is_some())
                || crate::widgets::controls::button(event, self.java_hit, false))
        {
            self.java_runtime
                .get_or_insert_with(JavaRuntimeDialog::default)
                .hidden = false;
            self.focus_modal(focus);
            return Ok(());
        }
        if java_visible {
            let dialog = self.java_runtime.as_mut().expect("visible Java dialog");
            dialog.dispatch(event);
            match dialog.take_action() {
                Some(java_runtime::Action::Download) => {
                    let (send, receive) =
                        watch::channel(kode_database::java_runtime::Progress::default());
                    let tag = self.work(Operation::JavaRuntime { progress: send })?;
                    self.java_runtime
                        .as_mut()
                        .expect("Java dialog")
                        .start(tag, receive);
                }
                Some(java_runtime::Action::Cancel) => {
                    if let Some(tag) = dialog.tag
                        && let Some(cancel) = self.pending.get(&tag)
                    {
                        cancel.cancel();
                    }
                }
                Some(java_runtime::Action::Hide) => {
                    dialog.hidden = true;
                }
                Some(java_runtime::Action::Details) => self.details(),
                None => {}
            }
        } else if let Some(install) = &mut self.install {
            install.dispatch(event);
            let tag = install.tag;
            let action = install.take_action();
            match action {
                Some(driver_install::Action::Cancel) => {
                    if let Some(token) = self.pending.get(&tag) {
                        token.cancel();
                    }
                }
                Some(driver_install::Action::Details) => self.details(),
                Some(driver_install::Action::Dismiss) => self.install = None,
                None => {}
            }
        } else if let Some(menu) = &mut self.menu {
            menu.dispatch(event);
            if menu.finished {
                let mut menu = self.menu.take().expect("finished object menu");
                if menu.source_requested {
                    if let Some(request) = DatabaseTree::request_for(&menu.object) {
                        self.work(Operation::Source {
                            source: menu.source,
                            object: menu.object,
                            request,
                        })?;
                    }
                } else if let Some(sql) = menu.sql.take() {
                    self.intents.push(Intent::Console {
                        source: menu.source.clone(),
                        sql,
                    });
                }
            }
        } else if let Some(form) = &mut self.form {
            form.dispatch(event);
            let actions = form.take_actions();
            for action in actions {
                match action {
                    connection_dialog::Action::Browse => {
                        if let Some(form) = &self.form
                            && let Some((request, proof)) = form.path_request()?
                        {
                            self.intents.push(Intent::Browse { request, proof });
                        }
                    }
                    connection_dialog::Action::JavaRuntime => {
                        self.java_runtime
                            .get_or_insert_with(JavaRuntimeDialog::default)
                            .hidden = false;
                    }
                    connection_dialog::Action::Details => self.details(),
                    connection_dialog::Action::Dismiss => {
                        if let Some(tag) = self.form.as_ref().and_then(|form| form.busy)
                            && let Some(cancel) = self.pending.get(&tag)
                        {
                            cancel.cancel();
                        }
                        self.form = None;
                    }
                    connection_dialog::Action::Save(draft) => {
                        let tag = self.work(Operation::Save(draft))?;
                        if let Some(form) = &mut self.form {
                            form.busy = Some(tag);
                            form.status = "Saving datasource…".into();
                        }
                    }
                    connection_dialog::Action::Test(draft) => {
                        let tag = self.work(Operation::Test(draft))?;
                        if let Some(form) = &mut self.form {
                            form.busy = Some(tag);
                            form.status = "Testing connection…".into();
                        }
                    }
                    connection_dialog::Action::Download(draft) => {
                        let name = draft.definition.driver.display_name.clone();
                        let (send, receive) = watch::channel(Progress::default());
                        let tag = self.work(Operation::Resolve {
                            spec: draft.definition.driver,
                            progress: send,
                        })?;
                        self.install = Some(DriverInstallDialog::new(tag, name, receive));
                    }
                    connection_dialog::Action::Suggestions(driver) => {
                        self.work(Operation::Recents(driver))?;
                    }
                    connection_dialog::Action::Field(action) => self.field(action),
                }
            }
        } else {
            if matches!(event,InputEvent::Key(input)if input.key==Key::Character('R')&&!input.modifiers.ctrl&&!input.modifiers.alt&&!input.modifiers.meta && crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Database,*input).is_some())
            {
                self.work(Operation::Restart)?;
                self.tree.notice = "Restarting database service…".into();
                return Ok(());
            }
            if matches!(event,InputEvent::Key(input)if input.key==Key::Character('r')&&self.tree.sources.is_empty()&&!input.modifiers.ctrl&&!input.modifiers.alt&&!input.modifiers.meta)
            {
                self.initialized = false;
                self.initialize()?;
                return Ok(());
            }
            self.tree.dispatch(event);
            self.drain_tree(focus)?;
        }
        self.restore_focus(focus);
        Ok(())
    }
    pub fn drain_tree(&mut self, focus: &mut FocusState) -> Result<()> {
        for action in self.tree.take_actions() {
            match action {
                tree::Action::Details => self.details(),
                tree::Action::New => self.edit(None, focus)?,
                tree::Action::Edit(source) => {
                    if let Some(definition) =
                        self.tree.sources.iter().find(|ds| ds.id == source).cloned()
                    {
                        self.edit(Some(definition), focus)?;
                    }
                }
                tree::Action::Delete(source) => {
                    self.work(Operation::Delete(source))?;
                }
                tree::Action::Connection(source) => {
                    if let Some(definition) =
                        self.tree.sources.iter().find(|ds| ds.id == source).cloned()
                    {
                        self.work(Operation::Connect(definition))?;
                    }
                }
                tree::Action::Console(source) => self.intents.push(Intent::Console {
                    source,
                    sql: String::new(),
                }),
                tree::Action::OpenSession(id) => self.intents.push(Intent::OpenSession(id)),
                tree::Action::RemoveSession(id) => self.intents.push(Intent::RemoveSession(id)),
                tree::Action::Object { source, mut object } => {
                    if let Some(ds) = self.tree.sources.iter().find(|ds| ds.id == source) {
                        object
                            .attributes
                            .entry("driverId".into())
                            .or_insert(ds.driver.id.clone());
                    }
                    self.menu = Some(ObjectMenu::new(source, object));
                    self.focus_modal(focus);
                }
                tree::Action::Load(load) => {
                    self.labels
                        .insert(load.tag, format!("Metadata: {}", load.key.source()));
                    self.pending.insert(load.tag, load.cancel.clone());
                    self.intents.push(Intent::Work(Box::new(Work {
                        tag: load.tag,
                        cancel: load.cancel.clone(),
                        operation: Operation::Metadata(load),
                    })));
                }
            }
        }
        Ok(())
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        context: Context,
        result: std::result::Result<Report, String>,
        current: bool,
        focus: &mut FocusState,
    ) {
        self.pending.remove(&tag);
        let label = self
            .labels
            .remove(&tag)
            .unwrap_or_else(|| "Database operation".into());
        if matches!(&context, Context::Source { .. }) {
            if self.source_request != Some(tag) {
                return;
            }
            self.source_request = None;
            if !current {
                self.tree.notice =
                    "Package source context changed; reopen it from the current connection".into();
                return;
            }
        }
        if tag.workspace != self.tree.generation {
            return;
        }
        if let Err(error) = &result {
            self.last_detail = Some((
                format!("{label} / request {}", tag.request.0),
                error.clone(),
            ));
        }
        if let Ok(Report::Installed(Err(error))) = &result {
            self.last_detail = Some((
                format!("{label} / request {}", tag.request.0),
                error.to_string(),
            ));
        }
        if let Context::Metadata(load) = context {
            self.tree.complete(
                &load,
                result.and_then(|report| match report {
                    Report::Metadata(result) => Ok(result),
                    _ => Err("Invalid metadata completion".into()),
                }),
                current,
            );
            return;
        }
        match result {
            Ok(Report::Source(result)) => {
                if let Context::Source { object, .. } = context {
                    let selected = self
                        .tree
                        .lines
                        .get(self.tree.selected)
                        .and_then(|line| line.object.as_ref());
                    if !self.has_modal()
                        && focus.focused == Some(self.tree.id)
                        && selected.is_some_and(|node| node.id == object.id)
                    {
                        self.intents.push(Intent::Source {
                            title: object.qualified_name,
                            documents: result.result.documents.clone(),
                        });
                        self.tree.notice = "Package source loaded".into();
                    } else {
                        self.tree.notice =
                            "Package source context changed; reopen it from the selected object"
                                .into();
                    }
                }
            }
            Ok(Report::JavaRuntime(message)) => {
                if let Some(dialog) = &mut self.java_runtime {
                    dialog.complete(tag, Ok(message.clone()));
                }
                self.tree.notice = message;
            }
            Ok(Report::Restarted) => {
                self.tree.notice = "Database service restarted; connect explicitly".into()
            }
            Ok(Report::Sources { sources, warning }) => {
                self.tree.replace_sources(sources);
                self.tree.notice = warning.unwrap_or_else(|| match context {
                    Context::Save => "Datasource saved".into(),
                    Context::Delete => "Datasource removed".into(),
                    _ => "Datasource configuration loaded".into(),
                });
                if matches!(context, Context::Save)
                    && self
                        .form
                        .as_ref()
                        .is_some_and(|form| form.busy == Some(tag))
                {
                    self.form = None;
                }
            }
            Ok(Report::Connected(warning)) => {
                self.tree.notice = warning.unwrap_or_else(|| "Connected".into());
            }
            Ok(Report::Closed) => self.tree.notice = "Disconnected".into(),
            Ok(Report::Test { result, warning }) => {
                if let Some(form) = &mut self.form {
                    form.complete(
                        tag,
                        warning.unwrap_or_else(|| {
                            format!(
                                "Success: {} {} ({} ms)",
                                result.product_name, result.product_version, result.latency_ms
                            )
                        }),
                    );
                }
            }
            Ok(Report::Installed(result)) => {
                if let Some(install) = &mut self.install
                    && install.complete(tag, result)
                    && let Some(form) = &mut self.form
                {
                    form.status = install.message.clone();
                    if let Some(installed) = &install.installed {
                        form.fields[6].value = installed.resolved_version.clone();
                        form.fields[6].cursor = form.fields[6].value.len();
                        form.fields[6].anchor = None;
                    }
                }
            }
            Ok(Report::Recents(values)) => {
                if let Context::Recents(driver) = context
                    && let Some(form) = &mut self.form
                {
                    form.set_recents(&driver, values);
                }
            }
            Ok(Report::Metadata(_)) => {}
            Err(error) => {
                if matches!(context, Context::JavaRuntime)
                    && let Some(dialog) = &mut self.java_runtime
                {
                    dialog.complete(tag, Err(error.clone()));
                }
                if matches!(context, Context::Resolve)
                    && let Some(install) = &mut self.install
                {
                    install.complete(
                        tag,
                        Err(kode_database::bridge::client::BridgeError::new(
                            kode_database::bridge::client::Failure::Unavailable,
                            "Driver operation could not be completed",
                        )),
                    );
                    install.message = error.clone();
                    if let Some(form) = &mut self.form {
                        form.status = error.clone();
                    }
                }
                if matches!(context, Context::Initialize) {
                    self.initialized = false;
                }
                if let Some(form) = &mut self.form {
                    if !form.complete(tag, error.clone()) {
                        self.tree.notice = error;
                    }
                } else {
                    self.tree.notice = error;
                }
            }
        }
        self.restore_focus(focus);
    }
    pub fn cancel_all(&self) {
        for token in self.pending.values() {
            token.cancel();
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        self.tree.render(area, buffer, theme, focused);
        self.java_hit = Rect::default();
        if (self.java_required || self.java_runtime.is_some()) && area.height > 0 {
            let width = area.width.min(14);
            self.java_hit = Rect::new(area.right() - width, area.y, width, 1);
            crate::widgets::controls::draw_text(
                self.java_hit,
                buffer,
                "[F5 Get Java]",
                theme.style("lsp-button"),
            );
        }
    }
    pub fn render_modal(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Option<Position> {
        let cursor = self
            .form
            .as_mut()
            .and_then(|form| form.render(area, buffer, theme));
        if let Some(menu) = &mut self.menu {
            menu.render(area, buffer, theme);
        }
        if let Some(install) = &mut self.install {
            install.render(area, buffer, theme);
            return None;
        }
        if let Some(dialog) = &mut self.java_runtime
            && !dialog.hidden
        {
            dialog.render(area, buffer, theme);
            return None;
        }
        cursor
    }
}
