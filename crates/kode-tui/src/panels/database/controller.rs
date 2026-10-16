//! Main-thread composition of the explorer and nested database dialogs.
use super::{
    connection_dialog::{self, ConnectionDialog, Draft},
    driver_install::{self, DriverInstallDialog},
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
    model::{DataSourceDefinition, DriverSpec, MetadataResult},
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
    Resolve {
        spec: DriverSpec,
        progress: watch::Sender<Progress>,
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
    Resolve,
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
            Operation::Resolve { .. } => Context::Resolve,
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
    Installed(BridgeResult<Installed>),
    Recents(Vec<String>),
}
pub struct PasteProof {
    pub dialog: u64,
    pub field: usize,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
pub enum Intent {
    Work(Box<Work>),
    Console {
        source: String,
        sql: String,
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
    pub menu: Option<ObjectMenu>,
    pub initialized: bool,
    registry: Arc<DriverRegistry>,
    urls: Arc<UrlCatalog>,
    intents: Vec<Intent>,
    pending: HashMap<JobTag, CancellationToken>,
    form_id: u64,
    modal_id: FocusId,
    previous: Option<Option<FocusId>>,
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
            menu: None,
            initialized: false,
            registry,
            urls,
            intents: vec![],
            pending: HashMap::new(),
            form_id: 0,
            modal_id: FocusId(RequestId::fresh()?.0),
            previous: None,
        })
    }
    pub fn configure(&mut self, registry: Arc<DriverRegistry>, urls: Arc<UrlCatalog>) {
        self.registry = registry;
        self.urls = urls;
    }
    pub fn has_modal(&self) -> bool {
        self.form.is_some() || self.install.is_some() || self.menu.is_some()
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
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<()> {
        if let Some(install) = &mut self.install {
            install.dispatch(event);
            let tag = install.tag;
            let action = install.take_action();
            match action {
                Some(driver_install::Action::Cancel) => {
                    if let Some(token) = self.pending.get(&tag) {
                        token.cancel();
                    }
                }
                Some(driver_install::Action::Dismiss) => self.install = None,
                None => {}
            }
        } else if let Some(menu) = &mut self.menu {
            menu.dispatch(event);
            if menu.finished {
                if let Some(sql) = menu.sql.take() {
                    self.intents.push(Intent::Console {
                        source: menu.source.clone(),
                        sql,
                    });
                }
                self.menu = None;
            }
        } else if let Some(form) = &mut self.form {
            form.dispatch(event);
            let actions = form.take_actions();
            for action in actions {
                match action {
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
            if matches!(event,InputEvent::Key(input)if input.key==Key::Character('R')&&!input.modifiers.ctrl&&!input.modifiers.alt&&!input.modifiers.meta)
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
        if tag.workspace != self.tree.generation {
            return;
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
        cursor
    }
}
