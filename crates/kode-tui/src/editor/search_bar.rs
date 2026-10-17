use super::{
    input::{EditorInputHooks, EditorState, HookContext, InputTarget},
    render::OverlaySpan,
};
use crate::{
    app::events::ResultGate,
    input::terminal::*,
    theme::Theme,
    widgets::field::{FieldAction, TextField},
};
use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DomainError, JobTag, RequestId, Result, WorkspaceGeneration,
    document::{Document, TextSnapshot},
    editor::search::{SearchReport, SearchResults, scan},
};
use kode_platform::jobs::{Delivery, OwnerId, WorkerScheduler};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchField {
    Find,
    Replace,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchAction {
    Next,
    All,
    SelectOccurrences,
    ReplaceOne,
    ReplaceAll,
    Close,
}
#[derive(Debug, Clone)]
pub struct FieldPasteTicket {
    pub request: RequestId,
    pub document: DocumentId,
    pub field: SearchField,
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
}
#[derive(Debug, Clone)]
pub enum SearchEffect {
    Scan {
        tag: JobTag,
        snapshot: TextSnapshot,
        query: String,
    },
    Copy(String),
    ReadClipboard(FieldPasteTicket),
    FieldLimitReached,
}
pub fn schedule_scan(
    scheduler: &mut dyn WorkerScheduler<SearchReport>,
    owner: OwnerId,
    tag: JobTag,
    snapshot: TextSnapshot,
    query: String,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        Delivery::Cancellable,
        Box::new(move |cancel| scan(&snapshot, &query, || cancel.is_cancelled())),
    )
}

#[derive(Debug, Clone, Copy)]
pub struct SearchGeometry {
    pub area: Rect,
    pub find: Rect,
    pub replace: Rect,
    pub close: Rect,
    pub status: Rect,
    pub hints: Rect,
}
fn row_geometry(area: Rect, row: u16, label: u16, reserve: u16, trailing: usize) -> (Rect, Rect) {
    if area.height <= row {
        return (Rect::default(), Rect::default());
    }
    let label = label.min(area.width);
    let available = area.width.saturating_sub(label).saturating_sub(reserve);
    let trailing = trailing.min(available.saturating_sub(1) as usize) as u16;
    let width = available - trailing;
    (
        Rect::new(area.x + label, area.y + row, width, 1),
        Rect::new(area.x + label + width, area.y + row, trailing, 1),
    )
}
const HINTS: &str =
    "Enter:Next  Alt+Enter:Cursors  Ctrl+Enter:All  Ctrl+R:Replace  Ctrl+Shift+R:All";

/// Search owns worker result publication and the two fields. The editor owns
/// document/caret/viewport; host effects perform all expensive/native work.
pub struct SearchBar {
    pub visible: bool,
    pub focused: SearchField,
    pub find: TextField,
    pub replace: TextField,
    pub results: SearchResults,
    pub error: Option<String>,
    gate: ResultGate,
    active_tag: Option<JobTag>,
    document: Option<DocumentId>,
    pending_action: Option<SearchAction>,
    pending_paste: Option<FieldPasteTicket>,
    effects: Vec<SearchEffect>,
}
impl SearchBar {
    pub fn new(workspace: WorkspaceGeneration) -> Self {
        let field = || {
            let mut field = TextField::new(String::new());
            field.framed = false;
            field
        };
        Self {
            visible: false,
            focused: SearchField::Find,
            find: field(),
            replace: field(),
            results: SearchResults::default(),
            error: None,
            gate: ResultGate::new(workspace),
            active_tag: None,
            document: None,
            pending_action: None,
            pending_paste: None,
            effects: vec![],
        }
    }
    pub fn preferred_height(&self) -> u16 {
        if self.visible { 2 } else { 0 }
    }
    pub fn take_effects(&mut self) -> Vec<SearchEffect> {
        std::mem::take(&mut self.effects)
    }
    pub fn open(&mut self, snapshot: &TextSnapshot, selected: &str) {
        let query = if !selected.is_empty() {
            escape_literal(selected)
        } else if self.document == Some(snapshot.id) {
            self.find.value.clone()
        } else {
            String::new()
        };
        self.visible = true;
        self.document = Some(snapshot.id);
        self.focused = SearchField::Find;
        self.find = TextField::new(query);
        self.find.framed = false;
        self.pending_action = None;
        self.pending_paste = None;
        self.results.invalidate();
        self.refresh(snapshot);
    }
    pub fn close(&mut self, state: &mut EditorState) {
        self.visible = false;
        state.search_has_focus = false;
        self.active_tag = None;
        self.pending_action = None;
        self.pending_paste = None;
        self.effects.clear();
        self.gate.dismiss("search");
        self.results.invalidate();
        self.error = None;
    }
    pub fn switch_workspace(&mut self, state: &mut EditorState) {
        self.close(state);
        self.gate.switch_workspace();
        self.document = None;
    }
    /// Call for typing, paste, replace, undo/redo, reload and programmatic edits.
    pub fn document_changed(&mut self, snapshot: &TextSnapshot) {
        if self.visible && self.document == Some(snapshot.id) {
            if self
                .active_tag
                .is_some_and(|tag| tag.document == Some((snapshot.id, snapshot.revision)))
            {
                return;
            }
            self.pending_action = None;
            self.pending_paste = None;
            self.results.invalidate();
            self.refresh(snapshot);
        }
    }
    fn refresh(&mut self, snapshot: &TextSnapshot) {
        self.gate.document_changed(snapshot.id, snapshot.revision);
        self.error = None;
        let tag = self
            .gate
            .request("search".into(), Some((snapshot.id, snapshot.revision)));
        self.active_tag = Some(tag);
        // Coalesce local requests before the host drains them. Already submitted
        // jobs use the scheduler's owner cancellation and exact result gate.
        self.effects
            .retain(|effect| !matches!(effect, SearchEffect::Scan { .. }));
        self.effects.push(SearchEffect::Scan {
            tag,
            snapshot: snapshot.clone(),
            query: self.find.value.clone(),
        });
    }
    pub fn complete_scan(
        &mut self,
        tag: JobTag,
        result: Result<SearchReport>,
        document: &mut Document,
        state: &mut EditorState,
    ) -> Result<bool> {
        if !self.visible
            || self.document != Some(document.id())
            || self.active_tag != Some(tag)
            || tag.document != Some((document.id(), document.revision()))
            || !self.gate.accepts("search", tag)
        {
            return Ok(false);
        }
        let report = match result {
            Ok(report) => report,
            Err(DomainError::Cancelled) => return Ok(false),
            Err(error) => {
                self.error = Some(error.to_string());
                self.active_tag = None;
                self.pending_action = None;
                return Ok(true);
            }
        };
        if !self
            .results
            .publish(report, &document.snapshot(), &self.find.value)
        {
            return Ok(false);
        }
        self.active_tag = None;
        self.error = self
            .results
            .report
            .as_ref()
            .and_then(|report| report.error.clone());
        if let Some(action) = self.pending_action.take()
            && state.search_has_focus
        {
            self.action(action, document, state)?;
        }
        Ok(true)
    }
    pub fn label(&self) -> String {
        if self.error.is_some() {
            return "regex error".into();
        }
        if self.active_tag.is_some() {
            return "searching…".into();
        }
        let count = self
            .results
            .report
            .as_ref()
            .map_or(0, |report| report.matches.len());
        if count == 0 {
            "no matches".into()
        } else {
            format!(
                "{}/{count}",
                self.results.active.map_or(0, |index| index + 1)
            )
        }
    }
    pub fn geometry(&self, area: Rect) -> SearchGeometry {
        let area = Rect::new(area.x, area.y, area.width, area.height.min(2));
        let reserve = (3 + u16::from(area.width >= 4)).min(area.width);
        let (find, status) = row_geometry(area, 0, 6, reserve, self.label().chars().count());
        let (replace, hints) = row_geometry(area, 1, 9, 0, HINTS.len());
        SearchGeometry {
            area,
            find,
            replace,
            status,
            hints,
            close: Rect::new(
                area.right().saturating_sub(3).max(area.x),
                area.y,
                area.width.min(3),
                area.height.min(1),
            ),
        }
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        focused: bool,
    ) -> Option<Position> {
        if !self.visible {
            return None;
        }
        let geometry = self.geometry(area.intersection(buffer.area));
        Clear.render(geometry.area, buffer);
        buffer.set_style(geometry.area, theme.style("code-search-bar"));
        if geometry.area.is_empty() {
            return None;
        }
        buffer.set_stringn(
            geometry.area.x,
            geometry.area.y,
            "Find: ",
            geometry.area.width as usize,
            theme.style("code-search-label"),
        );
        if geometry.area.height > 1 {
            buffer.set_stringn(
                geometry.area.x,
                geometry.area.y + 1,
                "Replace: ",
                geometry.area.width as usize,
                theme.style("code-search-label"),
            );
        }
        self.find.style = self
            .error
            .as_ref()
            .map(|_| theme.style("code-search-field-error"));
        let find = self.find.render(
            geometry.find,
            buffer,
            theme,
            focused && self.focused == SearchField::Find,
        );
        let replace = self.replace.render(
            geometry.replace,
            buffer,
            theme,
            focused && self.focused == SearchField::Replace,
        );
        for (area, text, style) in [
            (
                geometry.status,
                self.label(),
                if self.error.is_some() {
                    "code-search-error"
                } else {
                    "code-search-status"
                },
            ),
            (geometry.hints, HINTS.into(), "code-search-hint"),
            (geometry.close, "[x]".into(), "code-search-close"),
        ] {
            if !area.is_empty() {
                buffer.set_stringn(
                    area.x,
                    area.y,
                    text,
                    area.width as usize,
                    theme.style(style),
                );
            }
        }
        if self.focused == SearchField::Find {
            find
        } else {
            replace
        }
    }
    pub fn overlays(&self, snapshot: &TextSnapshot, theme: &Theme) -> Vec<OverlaySpan> {
        let Some(report) = &self.results.report else {
            return vec![];
        };
        if !self.visible || !report.is_current(snapshot) {
            return vec![];
        }
        report
            .matches
            .iter()
            .enumerate()
            .map(|(index, matched)| OverlaySpan {
                range: matched.range,
                priority: if self.results.active == Some(index) {
                    31
                } else {
                    30
                },
                style: theme.style(if self.results.active == Some(index) {
                    "code-search-active"
                } else {
                    "code-search-match"
                }),
            })
            .collect()
    }
    pub fn action(
        &mut self,
        action: SearchAction,
        document: &mut Document,
        state: &mut EditorState,
    ) -> Result<()> {
        if !self.visible || self.document != Some(document.id()) {
            return Err(DomainError::Unsupported(
                "search bar belongs to another or closed document".into(),
            ));
        }
        if action == SearchAction::Close {
            self.close(state);
            return Ok(());
        }
        if matches!(action, SearchAction::ReplaceOne | SearchAction::ReplaceAll)
            && document.is_read_only()
        {
            return Err(DomainError::ReadOnly);
        }
        if self.results.report.as_ref().is_none_or(|report| {
            !report.is_current(&document.snapshot()) || report.query != self.find.value
        }) {
            self.pending_action = Some(action);
            if self.active_tag.is_none() {
                self.refresh(&document.snapshot());
            }
            return Ok(());
        }
        match action {
            SearchAction::Next => {
                self.results.next(&document.snapshot(), &mut state.caret)?;
            }
            SearchAction::All => {}
            SearchAction::SelectOccurrences => {
                if let Some(report) = &self.results.report
                    && report.error.is_none()
                    && !report.matches.is_empty()
                {
                    let primary = report
                        .matches
                        .iter()
                        .position(|found| {
                            found.range.contains(state.caret.cursor())
                                || found.range.end == state.caret.cursor()
                        })
                        .unwrap_or(0);
                    let selections: Vec<_> = report
                        .matches
                        .iter()
                        .map(|found| kode_core::Selection {
                            anchor: found.range.start,
                            head: found.range.end,
                        })
                        .collect();
                    state.caret = kode_core::editor::selection::CaretState {
                        selection: selections[primary],
                        secondary: selections
                            .into_iter()
                            .enumerate()
                            .filter_map(|(index, selection)| {
                                (index != primary).then_some(selection)
                            })
                            .collect(),
                    }
                    .normalized(&document.snapshot())?;
                    state.search_has_focus = false;
                    self.pending_paste = None;
                }
            }
            SearchAction::ReplaceOne | SearchAction::ReplaceAll => {
                let before = document.revision();
                self.results.replace(
                    document,
                    &mut state.caret,
                    &self.replace.value,
                    action == SearchAction::ReplaceAll,
                )?;
                if before != document.revision() {
                    self.refresh(&document.snapshot());
                }
            }
            SearchAction::Close => unreachable!(),
        }
        Ok(())
    }
    fn field(&mut self) -> &mut TextField {
        match self.focused {
            SearchField::Find => &mut self.find,
            SearchField::Replace => &mut self.replace,
        }
    }
    fn field_event(&mut self, event: &InputEvent, document: &Document) -> Result<()> {
        let before = self.field().value.clone();
        let action = self.field().handle(event);
        self.pending_paste = None;
        match action {
            Some(FieldAction::Copy(text)) => self.effects.push(SearchEffect::Copy(text)),
            Some(FieldAction::PasteRequested) => {
                let field = self.field();
                let (value, cursor, anchor) = (field.value.clone(), field.cursor, field.anchor);
                let ticket = FieldPasteTicket {
                    request: RequestId::fresh()?,
                    document: document.id(),
                    field: self.focused,
                    value,
                    cursor,
                    anchor,
                };
                self.pending_paste = Some(ticket.clone());
                self.effects.push(SearchEffect::ReadClipboard(ticket));
            }
            Some(FieldAction::LimitReached) => self.effects.push(SearchEffect::FieldLimitReached),
            _ => {}
        }
        if self.field().value != before && self.focused == SearchField::Find {
            self.pending_action = None;
            self.results.invalidate();
            self.refresh(&document.snapshot());
        }
        Ok(())
    }
    pub fn complete_paste(
        &mut self,
        ticket: &FieldPasteTicket,
        text: &str,
        document: &Document,
        state: &EditorState,
        focused: bool,
    ) -> Result<bool> {
        if !focused
            || !self.visible
            || !state.search_has_focus
            || self.document != Some(document.id())
            || self
                .pending_paste
                .as_ref()
                .is_none_or(|current| current.request != ticket.request)
            || ticket.document != document.id()
            || ticket.field != self.focused
        {
            return Ok(false);
        }
        let field = self.field();
        if (field.value.as_str(), field.cursor, field.anchor)
            != (ticket.value.as_str(), ticket.cursor, ticket.anchor)
        {
            return Ok(false);
        }
        self.field_event(&InputEvent::Paste(text.into()), document)?;
        Ok(true)
    }
}
impl EditorInputHooks for SearchBar {
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        if target == InputTarget::Popup {
            if let InputEvent::Pointer(pointer) = event
                && !context
                    .geometry
                    .search
                    .contains(Position::new(pointer.x, pointer.y))
            {
                self.pending_paste = None;
                if pointer.action == PointerAction::Down(MouseButton::Left) {
                    self.pending_action = None;
                }
            }
            return Ok(false);
        }
        if target != InputTarget::Search || !self.visible || self.document != Some(document.id()) {
            return Ok(false);
        }
        if matches!(event, InputEvent::Focus(false)) {
            self.pending_paste = None;
            self.pending_action = None;
            return Ok(false);
        }
        let geometry = self.geometry(context.geometry.search);
        if let InputEvent::Pointer(pointer) = event {
            if pointer.action != PointerAction::Down(MouseButton::Left) {
                return Ok(false);
            }
            let position = Position::new(pointer.x, pointer.y);
            if geometry.close.contains(position) {
                self.close(state);
                return Ok(true);
            }
            let field = if geometry.find.contains(position) {
                Some(SearchField::Find)
            } else if geometry.replace.contains(position) {
                Some(SearchField::Replace)
            } else {
                None
            };
            if let Some(field) = field {
                self.focused = field;
                self.pending_paste = None;
                self.field().hit_test(
                    if field == SearchField::Find {
                        geometry.find
                    } else {
                        geometry.replace
                    },
                    pointer.x,
                    pointer.y,
                );
                return Ok(true);
            }
            return Ok(false);
        }
        if let InputEvent::Key(raw) = event
            && let Some(input) =
                crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Search, *raw)
        {
            let action = match input.key {
                Key::Enter
                    if input.modifiers.alt && !input.modifiers.ctrl && !input.modifiers.meta =>
                {
                    Some(SearchAction::SelectOccurrences)
                }
                Key::Enter if input.modifiers.ctrl => Some(SearchAction::All),
                Key::Enter => Some(SearchAction::Next),
                Key::Character('r' | 'R') if input.modifiers.ctrl => {
                    Some(if input.modifiers.shift {
                        SearchAction::ReplaceAll
                    } else {
                        SearchAction::ReplaceOne
                    })
                }
                Key::Escape => Some(SearchAction::Close),
                Key::Tab => {
                    self.focused = if self.focused == SearchField::Find {
                        SearchField::Replace
                    } else {
                        SearchField::Find
                    };
                    self.pending_paste = None;
                    return Ok(true);
                }
                _ => None,
            };
            if let Some(action) = action {
                self.action(action, document, state)?;
                return Ok(true);
            }
        }
        self.field_event(event, document)?;
        Ok(true)
    }
}

pub fn escape_literal(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        if "\\.[]{}()*+?^$|".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}
