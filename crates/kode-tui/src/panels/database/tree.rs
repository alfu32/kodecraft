//! Application-thread explorer reducer. Work is tagged and scheduled by its
//! owner; input/render never calls JDBC, starts Java or reads configuration.
use crate::{
    editor::display::display_text,
    focus::{EventTarget, FocusId},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result, WorkspaceGeneration};
use kode_database::{
    connections::{Phase, Snapshot},
    model::{
        DataSourceDefinition, DatabaseObject, DatabaseObjectType, MetadataRequest,
        MetadataRequestKind, MetadataResult,
    },
};
use kode_platform::jobs::CancellationToken;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeKey {
    Source(String),
    Sessions(String),
    Session { source: String, id: String },
    Object { source: String, id: String },
}
impl NodeKey {
    pub fn source(&self) -> &str {
        match self {
            Self::Source(source)
            | Self::Sessions(source)
            | Self::Session { source, .. }
            | Self::Object { source, .. } => source,
        }
    }
}
#[derive(Clone)]
pub struct Load {
    pub tag: JobTag,
    pub key: NodeKey,
    pub request: MetadataRequest,
    pub refresh: bool,
    pub cancel: CancellationToken,
}
#[derive(Clone)]
pub enum Action {
    New,
    Edit(String),
    Delete(String),
    Connection(String),
    Console(String),
    OpenSession(String),
    RemoveSession(String),
    Object {
        source: String,
        object: DatabaseObject,
    },
    Load(Load),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub source: String,
    pub title: String,
}
#[derive(Debug, Clone)]
pub struct Line {
    pub key: NodeKey,
    pub depth: usize,
    pub label: String,
    pub expandable: bool,
    pub object: Option<DatabaseObject>,
    pub message: bool,
}
pub struct DatabaseTree {
    pub id: FocusId,
    pub generation: WorkspaceGeneration,
    pub sources: Vec<DataSourceDefinition>,
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub scroll: usize,
    pub notice: String,
    pub lines: Vec<Line>,
    statuses: BTreeMap<String, Snapshot>,
    expanded: BTreeSet<NodeKey>,
    children: BTreeMap<NodeKey, Arc<MetadataResult>>,
    retained: BTreeMap<NodeKey, usize>,
    pending: BTreeMap<NodeKey, Load>,
    errors: BTreeMap<NodeKey, String>,
    actions: VecDeque<Action>,
    area: Rect,
    new_button: Rect,
    refresh_button: Rect,
    edit_hits: Vec<(Rect, String)>,
    session_hits: Vec<(Rect, Action)>,
}
impl Drop for DatabaseTree {
    fn drop(&mut self) {
        for load in self.pending.values() {
            load.cancel.cancel();
        }
    }
}
impl DatabaseTree {
    pub fn new(generation: WorkspaceGeneration) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            generation,
            sources: vec![],
            sessions: vec![],
            selected: 0,
            scroll: 0,
            notice: String::new(),
            lines: vec![],
            statuses: BTreeMap::new(),
            expanded: BTreeSet::new(),
            children: BTreeMap::new(),
            retained: BTreeMap::new(),
            pending: BTreeMap::new(),
            errors: BTreeMap::new(),
            actions: VecDeque::new(),
            area: Rect::default(),
            new_button: Rect::default(),
            refresh_button: Rect::default(),
            edit_hits: vec![],
            session_hits: vec![],
        })
    }
    pub fn take_actions(&mut self) -> Vec<Action> {
        self.actions.drain(..).collect()
    }
    pub fn set_sessions(&mut self, sessions: Vec<Session>) {
        if self.sessions != sessions {
            self.sessions = sessions;
            self.rebuild();
        }
    }
    pub fn replace_sources(&mut self, sources: Vec<DataSourceDefinition>) {
        let ids: BTreeSet<_> = sources.iter().map(|s| s.id.as_str()).collect();
        let retired: Vec<_> = self
            .sources
            .iter()
            .filter(|s| {
                !ids.contains(s.id.as_str())
                    || sources.iter().any(|next| next.id == s.id && next != *s)
            })
            .map(|s| s.id.clone())
            .collect();
        for id in retired {
            self.invalidate_source(&id);
            self.statuses.remove(&id);
            self.expanded.remove(&NodeKey::Source(id));
        }
        self.sources = sources;
        self.rebuild();
    }
    pub fn set_status(&mut self, source: &str, status: Snapshot) {
        if self.statuses.get(source).is_some_and(|old| {
            old.revision == status.revision
                && old.phase == status.phase
                && old
                    .session
                    .as_ref()
                    .map(|s| (&s.connection_id, &s.helper_epoch))
                    == status
                        .session
                        .as_ref()
                        .map(|s| (&s.connection_id, &s.helper_epoch))
        }) {
            return;
        }
        let connected = status.phase == Phase::Connected
            && self
                .statuses
                .get(source)
                .is_none_or(|old| old.phase != Phase::Connected);
        let changed = self.statuses.get(source).is_some_and(|old| {
            old.revision != status.revision
                || old.phase != status.phase
                || old
                    .session
                    .as_ref()
                    .map(|s| (&s.connection_id, &s.helper_epoch))
                    != status
                        .session
                        .as_ref()
                        .map(|s| (&s.connection_id, &s.helper_epoch))
        });
        if changed {
            self.invalidate_source(source);
            if status.phase == Phase::Connected {
                self.errors.insert(
                    NodeKey::Source(source.into()),
                    "Metadata changed; refresh to load again".into(),
                );
            }
        }
        self.statuses.insert(source.into(), status);
        self.rebuild();
        if connected {
            let key = NodeKey::Source(source.into());
            self.expanded.insert(key.clone());
            if let Some(line) = self.lines.iter().find(|line| line.key == key).cloned() {
                self.load(&line, true);
                self.rebuild();
            }
        }
    }
    pub fn invalidate_source(&mut self, source: &str) {
        self.children.retain(|key, _| key.source() != source);
        self.retained.retain(|key, _| key.source() != source);
        self.errors.retain(|key, _| key.source() != source);
        self.expanded.retain(|key| {
            key.source() != source || matches!(key, NodeKey::Source(_) | NodeKey::Sessions(_))
        });
        self.pending.retain(|key, load| {
            if key.source() == source {
                load.cancel.cancel();
                false
            } else {
                true
            }
        });
        self.actions
            .retain(|action| !matches!(action,Action::Load(load)if load.key.source()==source));
    }
    pub fn complete(
        &mut self,
        load: &Load,
        result: std::result::Result<Arc<MetadataResult>, String>,
        current: bool,
    ) -> bool {
        if load.tag.workspace != self.generation
            || self
                .pending
                .get(&load.key)
                .is_none_or(|pending| pending.tag != load.tag)
        {
            return false;
        }
        self.pending.remove(&load.key);
        if load.cancel.is_cancelled() {
            return false;
        }
        match result {
            Ok(result) if current => {
                let size = result
                    .objects
                    .iter()
                    .map(|object| {
                        512 + object.id.len()
                            + object.name.len()
                            + object.qualified_name.len()
                            + object.catalog.as_ref().map_or(0, String::len)
                            + object.schema.as_ref().map_or(0, String::len)
                            + object
                                .attributes
                                .iter()
                                .map(|(k, v)| k.len() + v.len())
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                    + result.message.as_ref().map_or(0, String::len);
                let objects = self
                    .children
                    .values()
                    .map(|result| result.objects.len())
                    .sum::<usize>();
                if self.children.len() >= 256
                    || objects + result.objects.len() > 50000
                    || self.retained.values().sum::<usize>() + size > 32 * 1024 * 1024
                {
                    self.errors.insert(
                        load.key.clone(),
                        "Metadata display capacity reached; collapse other branches and refresh"
                            .into(),
                    );
                    self.rebuild();
                    return true;
                }
                self.notice = result
                    .message
                    .clone()
                    .unwrap_or_else(|| format!("Loaded {} metadata items", result.objects.len()));
                self.children.insert(load.key.clone(), result);
                self.retained.insert(load.key.clone(), size);
            }
            Ok(_) => {
                self.errors.insert(
                    load.key.clone(),
                    "Metadata changed; refresh to load again".into(),
                );
            }
            Err(message) => {
                self.errors.insert(load.key.clone(), message);
            }
        }
        self.rebuild();
        true
    }
    pub fn request_for(object: &DatabaseObject) -> Option<MetadataRequest> {
        use DatabaseObjectType::*;
        let mut request = MetadataRequest {
            kind: MetadataRequestKind::Object,
            catalog: object.catalog.clone(),
            schema: object.schema.clone(),
            category: None,
            object_name: None,
            object_type: None,
        };
        match object.object_type {
            Catalog => {
                request.kind = MetadataRequestKind::Catalog;
                request.catalog = Some(
                    object
                        .catalog
                        .clone()
                        .unwrap_or_else(|| object.name.clone()),
                );
            }
            Schema => {
                request.kind = MetadataRequestKind::Schema;
                request.schema = Some(object.schema.clone().unwrap_or_else(|| object.name.clone()));
            }
            Category => {
                request.kind = MetadataRequestKind::Category;
                request.category = match object.attributes.get("categoryType")?.as_str() {
                    "TABLE" => Some(Table),
                    "VIEW" => Some(View),
                    "PROCEDURE" => Some(Procedure),
                    "FUNCTION" => Some(Function),
                    "PACKAGE" => Some(Package),
                    "TYPE" => Some(Type),
                    _ => return None,
                };
            }
            _ => {
                request.object_name = Some(object.name.clone());
                request.object_type = Some(object.object_type);
            }
        }
        Some(request)
    }
    fn rebuild(&mut self) {
        let selected = self.lines.get(self.selected).map(|line| line.key.clone());
        let mut lines = vec![];
        for source in &self.sources {
            let key = NodeKey::Source(source.id.clone());
            let status = self
                .statuses
                .get(&source.id)
                .map_or(Phase::Disconnected, |s| s.phase);
            lines.push(Line {
                key: key.clone(),
                depth: 0,
                label: format!(
                    "{} [{}]",
                    source.name,
                    match status {
                        Phase::Disconnected => "offline",
                        Phase::Connecting => "connecting",
                        Phase::Connected => "connected",
                        Phase::Closing => "disconnecting",
                        Phase::Error => "error",
                    }
                ),
                expandable: true,
                object: None,
                message: false,
            });
            if self.expanded.contains(&key) {
                let sessions: Vec<_> = self
                    .sessions
                    .iter()
                    .filter(|s| s.source == source.id)
                    .collect();
                let folder = NodeKey::Sessions(source.id.clone());
                lines.push(Line {
                    key: folder.clone(),
                    depth: 1,
                    label: format!("Sessions ({})", sessions.len()),
                    expandable: true,
                    object: None,
                    message: false,
                });
                if self.expanded.contains(&folder) {
                    for session in sessions {
                        lines.push(Line {
                            key: NodeKey::Session {
                                source: source.id.clone(),
                                id: session.id.clone(),
                            },
                            depth: 2,
                            label: session.title.clone(),
                            expandable: false,
                            object: None,
                            message: false,
                        });
                    }
                }
            }
            self.append(&mut lines, &key, 1);
        }
        self.lines = lines;
        self.selected = selected
            .and_then(|key| {
                self.lines
                    .iter()
                    .position(|line| line.key == key && !line.message)
            })
            .unwrap_or(self.selected)
            .min(self.lines.len().saturating_sub(1));
        self.ensure_visible();
    }
    fn append(&self, lines: &mut Vec<Line>, key: &NodeKey, depth: usize) {
        if !self.expanded.contains(key) || lines.len() >= 50000 {
            return;
        }
        if depth > 32 {
            lines.push(Line {
                key: key.clone(),
                depth,
                label: "Metadata nesting limit reached".into(),
                expandable: false,
                object: None,
                message: true,
            });
            return;
        }
        let message = if self.pending.contains_key(key) {
            Some("Loading metadata…")
        } else {
            self.errors.get(key).map(String::as_str)
        };
        if let Some(label) = message {
            lines.push(Line {
                key: key.clone(),
                depth,
                label: label.into(),
                expandable: false,
                object: None,
                message: true,
            });
            return;
        }
        if let Some(result) = self.children.get(key) {
            for object in &result.objects {
                if lines.len() >= 50000 {
                    break;
                }
                let child = NodeKey::Object {
                    source: key.source().into(),
                    id: object.id.clone(),
                };
                lines.push(Line {
                    key: child.clone(),
                    depth,
                    label: object.name.clone(),
                    expandable: object.has_children,
                    object: Some(object.clone()),
                    message: false,
                });
                self.append(lines, &child, depth + 1);
            }
            if result.objects.is_empty()
                && let Some(message) = &result.message
            {
                lines.push(Line {
                    key: key.clone(),
                    depth,
                    label: message.clone(),
                    expandable: false,
                    object: None,
                    message: true,
                });
            }
        }
    }
    fn ensure_visible(&mut self) {
        let rows = self.area.height.saturating_sub(3) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + rows.max(1) {
            self.scroll = self.selected + 1 - rows.max(1);
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(rows));
    }
    fn selected_source(&self) -> Option<String> {
        self.lines
            .get(self.selected)
            .filter(|line| !line.message)
            .map(|line| line.key.source().into())
    }
    fn load(&mut self, line: &Line, refresh: bool) {
        if !matches!(line.key, NodeKey::Source(_) | NodeKey::Object { .. }) {
            return;
        }
        let request = match &line.object {
            Some(object) => Self::request_for(object),
            None => Some(MetadataRequest {
                kind: MetadataRequestKind::Root,
                catalog: None,
                schema: None,
                category: None,
                object_name: None,
                object_type: None,
            }),
        };
        let Some(request) = request else {
            self.notice = "Unsupported metadata category".into();
            return;
        };
        let tag = match RequestId::fresh() {
            Ok(request) => JobTag {
                workspace: self.generation,
                request,
                document: None,
            },
            Err(error) => {
                self.notice = error.to_string();
                return;
            }
        };
        if let Some(old) = self.pending.remove(&line.key) {
            old.cancel.cancel();
        }
        self.prune(&line.key);
        self.errors.remove(&line.key);
        let load = Load {
            tag,
            key: line.key.clone(),
            request,
            refresh,
            cancel: CancellationToken::new(),
        };
        self.pending.insert(line.key.clone(), load.clone());
        self.actions.push_back(Action::Load(load));
    }
    pub fn refresh_selected(&mut self) {
        if let Some(line) = self.lines.get(self.selected).cloned() {
            if line.message {
                return;
            }
            self.expanded.insert(line.key.clone());
            self.load(&line, true);
            self.rebuild();
        }
    }
    pub fn activate(&mut self) {
        let Some(line) = self
            .lines
            .get(self.selected)
            .cloned()
            .filter(|line| !line.message)
        else {
            return;
        };
        if let NodeKey::Session { id, .. } = &line.key {
            self.actions.push_back(Action::OpenSession(id.clone()));
            return;
        }
        if matches!(line.key, NodeKey::Source(_))
            && self
                .statuses
                .get(line.key.source())
                .is_none_or(|s| s.phase != Phase::Connected)
        {
            self.actions
                .push_back(Action::Connection(line.key.source().into()));
            return;
        }
        if line.expandable {
            if self.expanded.remove(&line.key) {
                self.prune(&line.key);
            } else {
                if line.depth >= 32 {
                    self.notice = "Metadata nesting limit reached".into();
                    return;
                }
                self.expanded.insert(line.key.clone());
                if !self.children.contains_key(&line.key) && !self.pending.contains_key(&line.key) {
                    self.load(&line, false);
                }
            }
        } else if let Some(object) = line.object {
            self.actions.push_back(Action::Object {
                source: line.key.source().into(),
                object,
            });
        }
        self.rebuild();
    }
    fn prune(&mut self, root: &NodeKey) {
        let mut todo = vec![root.clone()];
        let mut seen = BTreeSet::new();
        while let Some(key) = todo.pop() {
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(result) = self.children.get(&key) {
                todo.extend(result.objects.iter().map(|object| NodeKey::Object {
                    source: key.source().into(),
                    id: object.id.clone(),
                }));
            }
        }
        for key in &seen {
            self.children.remove(key);
            self.retained.remove(key);
            self.errors.remove(key);
            if key != root {
                self.expanded.remove(key);
            }
            if let Some(load) = self.pending.remove(key) {
                load.cancel.cancel();
            }
        }
        self.actions
            .retain(|action| !matches!(action,Action::Load(load)if seen.contains(&load.key)));
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        self.area = area.intersection(buffer.area);
        self.new_button = Rect::default();
        self.refresh_button = Rect::default();
        self.edit_hits.clear();
        self.session_hits.clear();
        self.ensure_visible();
        let area = self.area;
        if area.is_empty() {
            return;
        }
        draw_text(area, buffer, "Databases", theme.style("content"));
        if area.height > 1 {
            let width = area.width.min(17);
            self.new_button = Rect::new(area.x, area.y + 1, width, 1);
            draw_text(
                self.new_button,
                buffer,
                "[+ New database]",
                theme.style("lsp-button"),
            );
            if area.width > width + 1 {
                self.refresh_button = Rect::new(
                    area.x + width + 1,
                    area.y + 1,
                    (area.width - width - 1).min(9),
                    1,
                );
                draw_text(
                    self.refresh_button,
                    buffer,
                    "[refresh]",
                    theme.style("lsp-button"),
                );
            }
        }
        if area.height > 2 {
            draw_text(
                Rect::new(area.x, area.y + 2, area.width, 1),
                buffer,
                &display_text(&self.notice),
                theme.style("content"),
            );
        }
        for (index, line) in self
            .lines
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(area.height.saturating_sub(3) as usize)
        {
            let y = area.y + 3 + (index - self.scroll) as u16;
            let marker = if line.expandable {
                if self.expanded.contains(&line.key) {
                    "[-]"
                } else {
                    "[+]"
                }
            } else {
                "   "
            };
            let text = format!(
                "{}{} {}",
                "  ".repeat(line.depth),
                marker,
                display_text(&line.label)
            );
            let edit = matches!(line.key, NodeKey::Source(_)) && !line.message && area.width >= 18;
            let session = matches!(line.key, NodeKey::Sessions(_) | NodeKey::Session { .. })
                && area.width >= 14;
            let width = area.width
                - if edit {
                    7
                } else if session {
                    6
                } else {
                    0
                };
            draw_text(
                Rect::new(area.x, y, width, 1),
                buffer,
                &text,
                theme.style(if focused && index == self.selected {
                    "file-entry:selected"
                } else {
                    "content"
                }),
            );
            if edit {
                let hit = Rect::new(area.right() - 6, y, 6, 1);
                draw_text(hit, buffer, "[edit]", theme.style("lsp-button"));
                self.edit_hits.push((hit, line.key.source().into()));
            }
            if session {
                let hit = Rect::new(area.right() - 5, y, 5, 1);
                let (action, label) = match &line.key {
                    NodeKey::Sessions(source) => (Action::Console(source.clone()), "[+]"),
                    NodeKey::Session { id, .. } => (Action::RemoveSession(id.clone()), "[-]"),
                    _ => unreachable!(),
                };
                draw_text(hit, buffer, label, theme.style("lsp-button"));
                self.session_hits.push((hit, action));
            }
        }
    }
}
impl EventTarget for DatabaseTree {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        if button(event, self.new_button, false) {
            self.actions.push_back(Action::New);
            return true;
        }
        if button(event, self.refresh_button, false) {
            self.refresh_selected();
            return true;
        }
        if let InputEvent::Pointer(p) = event {
            if !self.area.contains(Position::new(p.x, p.y)) {
                return false;
            }
            match p.action {
                PointerAction::Scroll { vertical, .. } => {
                    self.scroll = self
                        .scroll
                        .saturating_add_signed(vertical as isize * 3)
                        .min(
                            self.lines
                                .len()
                                .saturating_sub(self.area.height.saturating_sub(3) as usize),
                        );
                    return true;
                }
                PointerAction::Down(button) => {
                    if button == MouseButton::Left
                        && let Some((_, action)) = self
                            .session_hits
                            .iter()
                            .find(|(hit, _)| hit.contains(Position::new(p.x, p.y)))
                    {
                        self.actions.push_back(action.clone());
                        return true;
                    }
                    if button == MouseButton::Left
                        && let Some((_, source)) = self
                            .edit_hits
                            .iter()
                            .find(|(hit, _)| hit.contains(Position::new(p.x, p.y)))
                    {
                        self.actions.push_back(Action::Edit(source.clone()));
                        return true;
                    }
                    if p.y < self.area.y + 3 {
                        return false;
                    }
                    let index = self.scroll + (p.y - self.area.y - 3) as usize;
                    if index >= self.lines.len() {
                        return false;
                    }
                    self.selected = index;
                    let line = self.lines[index].clone();
                    if line.message {
                        return true;
                    }
                    if button == MouseButton::Right {
                        if let NodeKey::Session { id, .. } = &line.key {
                            self.actions.push_back(Action::OpenSession(id.clone()));
                        } else if matches!(line.key, NodeKey::Sessions(_)) {
                            self.activate();
                        } else if let Some(object) = line.object {
                            self.actions.push_back(Action::Object {
                                source: line.key.source().into(),
                                object,
                            });
                        } else {
                            self.actions
                                .push_back(Action::Edit(line.key.source().into()));
                        }
                    } else if button == MouseButton::Left {
                        if let Some(object) = line.object
                            && matches!(
                                object.object_type,
                                DatabaseObjectType::Table
                                    | DatabaseObjectType::View
                                    | DatabaseObjectType::Procedure
                                    | DatabaseObjectType::Function
                                    | DatabaseObjectType::Package
                                    | DatabaseObjectType::Type
                            )
                            && p.x >= self.area.x.saturating_add((line.depth * 2 + 3) as u16)
                        {
                            self.actions.push_back(Action::Object {
                                source: line.key.source().into(),
                                object,
                            });
                        } else {
                            self.activate();
                        }
                    }
                    return true;
                }
                _ => return false,
            }
        }
        let InputEvent::Key(input) = event else {
            return false;
        };
        if input.modifiers.ctrl || input.modifiers.alt || input.modifiers.meta {
            return false;
        }
        match input.key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => {
                self.selected = (self.selected + 1).min(self.lines.len().saturating_sub(1))
            }
            Key::PageUp => self.selected = self.selected.saturating_sub(10),
            Key::PageDown => {
                self.selected = (self.selected + 10).min(self.lines.len().saturating_sub(1))
            }
            Key::Home => self.selected = 0,
            Key::End => self.selected = self.lines.len().saturating_sub(1),
            Key::Enter | Key::Right => self.activate(),
            Key::Left => {
                if let Some(line) = self.lines.get(self.selected) {
                    self.expanded.remove(&line.key);
                }
                self.rebuild();
            }
            Key::Character('n') => self.actions.push_back(Action::New),
            Key::Character('r') => self.refresh_selected(),
            Key::Character('c' | 's') | Key::Delete | Key::Function(2) => {
                if input.key == Key::Delete
                    && let Some(Line {
                        key: NodeKey::Session { id, .. },
                        ..
                    }) = self.lines.get(self.selected)
                {
                    self.actions.push_back(Action::RemoveSession(id.clone()));
                    return true;
                }
                if let Some(source) = self.selected_source() {
                    self.actions.push_back(match input.key {
                        Key::Character('c') => Action::Connection(source),
                        Key::Character('s') => Action::Console(source),
                        Key::Delete => Action::Delete(source),
                        _ => Action::Edit(source),
                    });
                }
            }
            _ => return false,
        }
        self.ensure_visible();
        true
    }
}
