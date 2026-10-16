//! Bounded connection form. Captures explicit drafts; its owner performs all
//! persistence, credentials, testing and installation outside input/render.
use crate::{
    editor::display::display_text,
    focus::EventTarget,
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
    widgets::{
        controls::{button, draw_text},
        field::{FieldAction, TextField},
    },
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result};
use kode_database::{
    catalog::{DriverRegistry, UrlCatalog},
    credentials::Secret,
    model::{DataSourceDefinition, JdbcDriverArtifact, JdbcRepositoryType},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
use std::{collections::BTreeMap, sync::Arc};

const LABELS: [&str; 13] = [
    "Name",
    "JDBC URL",
    "Username",
    "Password (blank keeps saved)",
    "Catalog",
    "Schema",
    "Driver version",
    "Driver class (blank discovers)",
    "Maven coordinates",
    "Maven repository",
    "Local JAR(s)",
    "Properties (JSON)",
    "Credential references (JSON)",
];
const NAME: usize = 0;
const URL: usize = 1;
const USER: usize = 2;
const PASSWORD: usize = 3;
const CATALOG: usize = 4;
const SCHEMA: usize = 5;
const VERSION: usize = 6;
const CLASS: usize = 7;
const COORDS: usize = 8;
const REPOSITORY: usize = 9;
const JAR: usize = 10;
const PROPERTIES: usize = 11;
const REFERENCES: usize = 12;
const AUTO: usize = 14;
const READ_ONLY: usize = 15;
const BUTTONS: usize = 16;
const TOTAL: usize = 20;
pub struct Draft {
    pub definition: DataSourceDefinition,
    pub password: Option<Secret>,
}
pub enum Action {
    Test(Draft),
    Download(Draft),
    Save(Draft),
    Dismiss,
    Field(FieldAction),
    Suggestions(String),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Popup {
    Drivers,
    Urls,
}
pub struct ConnectionDialog {
    pub fields: Vec<TextField>,
    pub focus: usize,
    pub status: String,
    pub busy: Option<JobTag>,
    pub auto_commit: bool,
    pub read_only: bool,
    pub selected_driver: usize,
    driver_changed: bool,
    registry: Arc<DriverRegistry>,
    urls: Arc<UrlCatalog>,
    original: Option<DataSourceDefinition>,
    id: String,
    actions: Vec<Action>,
    popup: Option<Popup>,
    popup_selected: usize,
    recents: BTreeMap<String, Vec<String>>,
    url_choices: Vec<(String, String)>,
    field_hits: Vec<(usize, Rect)>,
    buttons: [Rect; 4],
    checkboxes: [Rect; 2],
    driver_hit: Rect,
    popup_hits: Vec<(usize, Rect)>,
    scroll: usize,
}
fn optional(s: &str) -> Option<String> {
    (!s.trim().is_empty()).then(|| s.trim().to_owned())
}
impl ConnectionDialog {
    pub fn new(
        registry: Arc<DriverRegistry>,
        urls: Arc<UrlCatalog>,
        existing: Option<DataSourceDefinition>,
    ) -> Result<Self> {
        if registry.definitions().is_empty() {
            return Err(kode_core::DomainError::Unsupported(
                "The JDBC driver registry is empty".into(),
            ));
        }
        let selected = existing
            .as_ref()
            .and_then(|ds| {
                registry
                    .definitions()
                    .iter()
                    .position(|driver| driver.id == ds.driver.id)
            })
            .unwrap_or_else(|| {
                if existing.is_some() {
                    registry
                        .definitions()
                        .iter()
                        .position(|d| d.id == "custom-jdbc")
                        .unwrap_or(0)
                } else {
                    0
                }
            });
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = existing
            .as_ref()
            .map(|ds| ds.id.clone())
            .unwrap_or(format!("database-{stamp}-{}", RequestId::fresh()?.0));
        let mut form = Self {
            fields: LABELS
                .iter()
                .map(|_| TextField::new(String::new()))
                .collect(),
            focus: 0,
            status: String::new(),
            busy: None,
            auto_commit: existing.as_ref().is_none_or(|ds| ds.auto_commit),
            read_only: existing.as_ref().is_some_and(|ds| ds.read_only),
            selected_driver: selected,
            driver_changed: false,
            registry,
            urls,
            original: existing.clone(),
            id,
            actions: vec![],
            popup: None,
            popup_selected: 0,
            recents: BTreeMap::new(),
            url_choices: vec![],
            field_hits: vec![],
            buttons: [Rect::default(); 4],
            checkboxes: [Rect::default(); 2],
            driver_hit: Rect::default(),
            popup_hits: vec![],
            scroll: 0,
        };
        for field in &mut form.fields {
            field.max_chars = 4096;
        }
        form.fields[URL].max_chars = 16384;
        form.fields[PASSWORD].max_chars = 16384;
        form.fields[PROPERTIES].max_chars = 65536;
        form.fields[REFERENCES].max_chars = 65536;
        form.fields[PASSWORD].masked = true;
        form.select_driver(selected, true);
        if let Some(ds) = existing {
            form.set(NAME, ds.name);
            form.set(URL, ds.jdbc_url);
            form.set(USER, ds.username);
            form.set(CATALOG, ds.default_catalog.unwrap_or_default());
            form.set(SCHEMA, ds.default_schema.unwrap_or_default());
            form.set(CLASS, ds.driver.driver_class);
            form.set(COORDS, ds.driver.coordinates.clone().unwrap_or_default());
            form.set(JAR, ds.driver.jar_path.unwrap_or_default());
            form.set(
                VERSION,
                ds.driver
                    .artifact
                    .as_ref()
                    .and_then(|a| a.version.clone())
                    .or_else(|| {
                        ds.driver
                            .coordinates
                            .as_ref()
                            .and_then(|s| s.split(':').nth(2).map(str::to_owned))
                    })
                    .unwrap_or_default(),
            );
            form.set(
                REPOSITORY,
                ds.driver
                    .artifact
                    .and_then(|a| a.repository)
                    .unwrap_or_default(),
            );
            form.set(
                PROPERTIES,
                serde_json::to_string(&ds.properties).expect("property map"),
            );
            form.set(
                REFERENCES,
                serde_json::to_string(&ds.credential_properties).expect("reference map"),
            );
        } else {
            form.set(PROPERTIES, "{}".into());
            form.set(REFERENCES, "{}".into());
        }
        Ok(form)
    }
    fn set(&mut self, index: usize, value: String) {
        self.fields[index].cursor = value.len();
        self.fields[index].anchor = None;
        self.fields[index].value = value;
    }
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    pub fn selected_id(&self) -> &str {
        &self.registry.definitions()[self.selected_driver].id
    }
    pub fn select_driver(&mut self, index: usize, initial: bool) {
        if index >= self.registry.definitions().len() {
            return;
        }
        if !initial && index != self.selected_driver {
            self.driver_changed = true;
        }
        self.selected_driver = index;
        let driver = self.registry.definitions()[index].clone();
        if let Some(spec) = self.registry.spec_for(&driver.id) {
            self.set(CLASS, spec.driver_class);
            self.set(COORDS, spec.coordinates.unwrap_or_default());
            self.set(
                REPOSITORY,
                spec.artifact
                    .as_ref()
                    .and_then(|a| a.repository.clone())
                    .unwrap_or_default(),
            );
            self.set(JAR, spec.jar_path.unwrap_or_default());
            self.set(
                VERSION,
                spec.artifact.and_then(|a| a.version).unwrap_or_default(),
            );
        }
        if initial || self.fields[URL].value.is_empty() {
            self.set(URL, driver.default_jdbc_url_template);
        }
        self.popup = None;
    }
    pub fn set_recents(&mut self, driver: &str, values: Vec<String>) {
        self.recents.insert(
            driver.into(),
            values
                .into_iter()
                .filter(|url| !kode_database::security::contains_secret(url))
                .take(30)
                .collect(),
        );
        if self.popup == Some(Popup::Urls) && self.selected_id() == driver {
            self.build_urls();
        }
    }
    fn build_urls(&mut self) {
        self.url_choices = self
            .recents
            .get(self.selected_id())
            .into_iter()
            .flatten()
            .map(|url| (format!("Recents: {url}"), url.clone()))
            .collect();
        for template in self.urls.templates(self.selected_id()) {
            self.url_choices.push((
                format!("Standard — {}: {}", template.label, template.template),
                template.template.clone(),
            ));
        }
        self.popup_selected = self
            .popup_selected
            .min(self.url_choices.len().saturating_sub(1));
    }
    fn open_urls(&mut self) {
        self.build_urls();
        self.popup = Some(Popup::Urls);
        self.popup_selected = 0;
        self.actions
            .push(Action::Suggestions(self.selected_id().into()));
    }
    pub fn draft(&self) -> std::result::Result<Draft, String> {
        let mut spec = if self.original.as_ref().is_some_and(|ds| {
            ds.driver.id == self.selected_id()
                || (!self.driver_changed && self.registry.by_id(&ds.driver.id).is_none())
        }) {
            self.original.as_ref().unwrap().driver.clone()
        } else {
            self.registry
                .spec_for(self.selected_id())
                .ok_or("Choose a driver")?
        };
        let properties: BTreeMap<String, String> =
            serde_json::from_str(&self.fields[PROPERTIES].value)
                .map_err(|_| "Properties must be a JSON object of string values")?;
        let references: BTreeMap<String, String> =
            serde_json::from_str(&self.fields[REFERENCES].value)
                .map_err(|_| "Credential references must be a JSON object of string values")?;
        if properties.len() > 256 || references.len() > 256 {
            return Err("Use at most 256 properties and credential references".into());
        }
        let coordinates = optional(&self.fields[COORDS].value);
        let repository = optional(&self.fields[REPOSITORY].value);
        let local = optional(&self.fields[JAR].value);
        if coordinates != spec.coordinates
            || repository != spec.artifact.as_ref().and_then(|a| a.repository.clone())
        {
            spec.artifact = coordinates
                .as_deref()
                .and_then(JdbcDriverArtifact::from_coordinates);
            if let Some(artifact) = &mut spec.artifact {
                artifact.repository = repository.clone();
                artifact.repository_type = if repository.is_some() {
                    JdbcRepositoryType::MavenCustom
                } else {
                    JdbcRepositoryType::MavenCentral
                };
            }
        }
        spec.coordinates = coordinates;
        spec.jar_path = local.clone();
        spec.driver_class = self.fields[CLASS].value.trim().into();
        if spec.id == "custom-jdbc" {
            spec.provider_id = Some(format!("custom-{}", self.id));
        }
        if local.is_some() {
            spec.artifact.get_or_insert_default().repository_type = JdbcRepositoryType::LocalFile;
        }
        if let Some(version) = optional(&self.fields[VERSION].value) {
            if let Some(artifact) = &mut spec.artifact {
                artifact.version = Some(version.clone());
            } else if let Some(coordinates) = &spec.coordinates {
                let parts: Vec<_> = coordinates.split(':').collect();
                if parts.len() >= 2 {
                    spec.coordinates = Some(format!(
                        "{}:{}:{}{}",
                        parts[0],
                        parts[1],
                        version,
                        parts
                            .get(3)
                            .map_or(String::new(), |classifier| format!(":{classifier}"))
                    ));
                }
            }
        }
        if !self.fields[URL]
            .value
            .to_ascii_lowercase()
            .starts_with("jdbc:")
        {
            return Err("JDBC URL must start with jdbc:".into());
        }
        let password = (!self.fields[PASSWORD].value.is_empty())
            .then(|| Secret::new(self.fields[PASSWORD].value.clone()));
        let definition = DataSourceDefinition {
            id: self.id.clone(),
            name: optional(&self.fields[NAME].value).unwrap_or_else(|| {
                self.registry.definitions()[self.selected_driver]
                    .database_family
                    .clone()
            }),
            driver: spec,
            jdbc_url: self.fields[URL].value.clone(),
            username: self.fields[USER].value.clone(),
            credential_reference: if password.is_some() {
                Some(format!("{}.password", self.id))
            } else {
                self.original
                    .as_ref()
                    .and_then(|ds| ds.credential_reference.clone())
            },
            properties,
            credential_properties: references,
            default_catalog: optional(&self.fields[CATALOG].value),
            default_schema: optional(&self.fields[SCHEMA].value),
            auto_commit: self.auto_commit,
            read_only: self.read_only,
            extra: self
                .original
                .as_ref()
                .map(|ds| ds.extra.clone())
                .unwrap_or_default(),
        };
        serde_json::to_value(&definition).map_err(
            |_| "Credentials belong in Password or private references; check public URL/properties",
        )?;
        Ok(Draft {
            definition,
            password,
        })
    }
    fn activate(&mut self, index: usize) {
        if index == 3 {
            self.actions.push(Action::Dismiss);
            return;
        }
        if self.busy.is_some() {
            return;
        }
        match self.draft() {
            Ok(draft) => self.actions.push(match index {
                0 => Action::Test(draft),
                1 => Action::Download(draft),
                _ => Action::Save(draft),
            }),
            Err(message) => self.status = message,
        }
    }
    pub fn complete(&mut self, tag: JobTag, message: String) -> bool {
        if self.busy != Some(tag) {
            return false;
        }
        self.busy = None;
        self.status = message;
        true
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Option<Position> {
        self.field_hits.clear();
        self.popup_hits.clear();
        self.buttons = [Rect::default(); 4];
        self.checkboxes = [Rect::default(); 2];
        self.driver_hit = Rect::default();
        let modal = centered(area.intersection(buffer.area), 86, 34);
        if modal.is_empty() {
            return None;
        }
        Clear.render(modal, buffer);
        Block::default()
            .borders(Borders::ALL)
            .title(if self.original.is_some() {
                "Edit database connection"
            } else {
                "New database connection"
            })
            .style(theme.style("project-search-dialog"))
            .render(modal, buffer);
        let content = modal.inner(ratatui::layout::Margin::new(
            u16::from(modal.width > 2),
            u16::from(modal.height > 2),
        ));
        if content.is_empty() {
            return None;
        }
        if content.height <= 4 && self.popup.is_none() {
            let hit = Rect::new(content.x, content.y, content.width, 1);
            if self.focus > 0 && self.focus < AUTO {
                self.field_hits.push((self.focus - 1, hit));
                return self.fields[self.focus - 1].render(hit, buffer, theme, true);
            }
            if self.focus >= BUTTONS {
                let index = self.focus - BUTTONS;
                self.buttons[index] = hit;
                draw_text(
                    hit,
                    buffer,
                    ["Test", "Download", "Save", "Cancel"][index],
                    theme.style("file-entry:selected"),
                );
            } else if self.focus == 0 {
                self.driver_hit = hit;
                draw_text(
                    hit,
                    buffer,
                    &display_text(self.selected_id()),
                    theme.style("file-entry:selected"),
                );
            } else {
                self.checkboxes[self.focus - AUTO] = hit;
                draw_text(
                    hit,
                    buffer,
                    if self.focus == AUTO {
                        "Autocommit"
                    } else {
                        "Read only"
                    },
                    theme.style("file-entry:selected"),
                );
            }
            return None;
        }
        if let Some(popup) = self.popup {
            let labels: Vec<String> = if popup == Popup::Drivers {
                self.registry
                    .definitions()
                    .iter()
                    .map(|d| format!("{} ({})", d.display_name, d.id))
                    .collect()
            } else {
                self.url_choices.iter().map(|c| c.0.clone()).collect()
            };
            let visible = content.height as usize;
            let start = self
                .popup_selected
                .saturating_sub(visible.saturating_sub(1));
            for (index, label) in labels.iter().enumerate().skip(start).take(visible) {
                let hit = Rect::new(
                    content.x,
                    content.y + (index - start) as u16,
                    content.width,
                    1,
                );
                draw_text(
                    hit,
                    buffer,
                    &display_text(label),
                    theme.style(if index == self.popup_selected {
                        "file-entry:selected"
                    } else {
                        "content"
                    }),
                );
                self.popup_hits.push((index, hit));
            }
            return None;
        }
        let vertical = content.width < 40;
        let footer = if vertical { 4 } else { 1 };
        let status_rows = u16::from(content.height > footer + 1);
        let body_height = content.height.saturating_sub(footer + status_rows);
        let focus_row = if self.focus == 0 {
            0
        } else if self.focus < AUTO {
            self.focus * 2
        } else {
            27 + self.focus.saturating_sub(AUTO)
        };
        if self.focus < BUTTONS && body_height > 0 {
            if focus_row < self.scroll {
                self.scroll = focus_row;
            }
            if focus_row >= self.scroll + body_height as usize {
                self.scroll = focus_row + 1 - body_height as usize;
            }
        }
        let row = |offset: usize| -> Option<Rect> {
            (offset >= self.scroll && offset < self.scroll + body_height as usize).then(|| {
                Rect::new(
                    content.x,
                    content.y + (offset - self.scroll) as u16,
                    content.width,
                    1,
                )
            })
        };
        if let Some(hit) = row(0) {
            self.driver_hit = hit;
            let driver = &self.registry.definitions()[self.selected_driver];
            draw_text(
                hit,
                buffer,
                &display_text(&format!(
                    "Connection type: {} ({}) ▾",
                    driver.display_name, driver.id
                )),
                theme.style(if self.focus == 0 {
                    "file-entry:selected"
                } else {
                    "content"
                }),
            );
        }
        let mut cursor = None;
        for (index, label) in LABELS.iter().enumerate() {
            if let Some(hit) = row(index * 2 + 1) {
                draw_text(hit, buffer, label, theme.style("content"));
            }
            if let Some(hit) = row(index * 2 + 2) {
                self.field_hits.push((index, hit));
                if let Some(position) =
                    self.fields[index].render(hit, buffer, theme, self.focus == index + 1)
                {
                    cursor = Some(position);
                }
            }
        }
        for (index, label, checked) in [
            (0, "Autocommit", self.auto_commit),
            (1, "Read only (driver hint)", self.read_only),
        ] {
            if let Some(hit) = row(27 + index) {
                self.checkboxes[index] = hit;
                draw_text(
                    hit,
                    buffer,
                    &format!("[{}] {label}", if checked { 'x' } else { ' ' }),
                    theme.style(if self.focus == AUTO + index {
                        "file-entry:selected"
                    } else {
                        "content"
                    }),
                );
            }
        }
        if status_rows > 0 {
            draw_text(
                Rect::new(content.x, content.bottom() - footer - 1, content.width, 1),
                buffer,
                &display_text(&self.status),
                theme.style("content"),
            );
        }
        let labels = [" Test ", " Download ", " Save ", " Cancel "];
        if vertical {
            for (index, label) in labels.iter().enumerate() {
                if index < (content.height as usize) {
                    let y = content.bottom() - content.height.min(4) + index as u16;
                    if y < content.bottom() {
                        self.buttons[index] = Rect::new(content.x, y, content.width, 1);
                        draw_text(
                            self.buttons[index],
                            buffer,
                            label,
                            theme.style(if self.focus == BUTTONS + index {
                                "file-entry:selected"
                            } else {
                                "lsp-button"
                            }),
                        );
                    }
                }
            }
        } else {
            let width = content.width / 4;
            for (index, label) in labels.iter().enumerate() {
                self.buttons[index] = Rect::new(
                    content.x + index as u16 * width,
                    content.bottom() - 1,
                    width.saturating_sub(1),
                    1,
                );
                draw_text(
                    self.buttons[index],
                    buffer,
                    label,
                    theme.style(if self.focus == BUTTONS + index {
                        "file-entry:selected"
                    } else {
                        "lsp-button"
                    }),
                );
            }
        }
        cursor
    }
    fn choose_popup(&mut self, index: usize) {
        match self.popup {
            Some(Popup::Drivers) => self.select_driver(index, false),
            Some(Popup::Urls) => {
                if let Some((_, value)) = self.url_choices.get(index) {
                    self.set(URL, value.clone());
                }
                self.popup = None;
                self.focus = URL + 1;
            }
            None => {}
        }
    }
}
impl EventTarget for ConnectionDialog {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        if let Some(popup) = self.popup {
            if let InputEvent::Key(input) = event {
                let count = if popup == Popup::Drivers {
                    self.registry.definitions().len()
                } else {
                    self.url_choices.len()
                };
                match input.key {
                    Key::Escape => self.popup = None,
                    Key::Up => self.popup_selected = self.popup_selected.saturating_sub(1),
                    Key::Down => {
                        self.popup_selected = (self.popup_selected + 1).min(count.saturating_sub(1))
                    }
                    Key::Home => self.popup_selected = 0,
                    Key::End => self.popup_selected = count.saturating_sub(1),
                    Key::Enter => self.choose_popup(self.popup_selected),
                    _ => {}
                }
            }
            if let InputEvent::Pointer(pointer) = event {
                match pointer.action {
                    PointerAction::Scroll { vertical, .. } => {
                        let count = if popup == Popup::Drivers {
                            self.registry.definitions().len()
                        } else {
                            self.url_choices.len()
                        };
                        self.popup_selected = self
                            .popup_selected
                            .saturating_add_signed(vertical as isize)
                            .min(count.saturating_sub(1));
                    }
                    PointerAction::Down(MouseButton::Left) => {
                        if let Some((index, _)) = self
                            .popup_hits
                            .iter()
                            .find(|(_, hit)| hit.contains(Position::new(pointer.x, pointer.y)))
                        {
                            self.choose_popup(*index);
                        }
                    }
                    _ => {}
                }
            }
            return true;
        }
        if let InputEvent::Key(input) = event {
            if input.key == Key::Escape {
                self.actions.push(Action::Dismiss);
                return true;
            }
            if input.modifiers.ctrl && input.key == Key::Character('s') {
                self.activate(2);
                return true;
            }
            if input.modifiers.alt && input.key == Key::Character('t') {
                self.activate(0);
                return true;
            }
            if input.modifiers.alt && input.key == Key::Character('d') {
                self.activate(1);
                return true;
            }
            match input.key {
                Key::Tab => {
                    self.focus =
                        (self.focus + if input.modifiers.shift { TOTAL - 1 } else { 1 }) % TOTAL;
                    return true;
                }
                Key::Up => {
                    self.focus = self.focus.saturating_sub(1);
                    return true;
                }
                Key::Down if self.focus == URL + 1 => {
                    self.open_urls();
                    return true;
                }
                Key::Down => {
                    self.focus = (self.focus + 1).min(TOTAL - 1);
                    return true;
                }
                Key::Enter | Key::Character(' ') if self.focus == 0 => {
                    self.popup = Some(Popup::Drivers);
                    self.popup_selected = self.selected_driver;
                    return true;
                }
                Key::Enter | Key::Character(' ')
                    if self.focus == AUTO || self.focus == READ_ONLY =>
                {
                    if self.focus == AUTO {
                        self.auto_commit = !self.auto_commit;
                    } else {
                        self.read_only = !self.read_only;
                    }
                    return true;
                }
                Key::Enter if self.focus >= BUTTONS => {
                    self.activate(self.focus - BUTTONS);
                    return true;
                }
                _ => {}
            }
        }
        if let Some(index) = self
            .buttons
            .iter()
            .enumerate()
            .find_map(|(i, hit)| button(event, *hit, self.focus == BUTTONS + i).then_some(i))
        {
            self.focus = BUTTONS + index;
            self.activate(index);
            return true;
        }
        if button(event, self.driver_hit, false) {
            self.focus = 0;
            self.popup = Some(Popup::Drivers);
            self.popup_selected = self.selected_driver;
            return true;
        }
        if let Some(index) = self
            .checkboxes
            .iter()
            .position(|hit| button(event, *hit, false))
        {
            self.focus = AUTO + index;
            if index == 0 {
                self.auto_commit = !self.auto_commit;
            } else {
                self.read_only = !self.read_only;
            }
            return true;
        }
        if let InputEvent::Pointer(pointer) = event {
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                for (index, hit) in &self.field_hits {
                    if self.fields[*index].hit_test(*hit, pointer.x, pointer.y) {
                        self.focus = index + 1;
                        return true;
                    }
                }
            }
        } else if self.focus > 0
            && self.focus < AUTO
            && let Some(action) = self.fields[self.focus - 1].handle(event)
        {
            self.actions.push(Action::Field(action));
        }
        true
    }
}
