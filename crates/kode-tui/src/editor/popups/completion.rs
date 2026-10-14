use crate::{
    app::events::ResultGate,
    editor::{
        display::display_text,
        input::{EditorInputHooks, EditorState, HookContext, InputTarget},
    },
    input::terminal::*,
    theme::Theme,
};
use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DomainError, JobTag, Result, Selection, TextRange, WorkspaceGeneration,
    commands::EditKind,
    document::{Document, TextSnapshot},
    editor::edit,
    intelligence::{CompletionItem, IntelligenceProvider, IntelligenceRequest},
};
use kode_platform::jobs::{Delivery, OwnerId, WorkerScheduler};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};
use std::{collections::HashSet, sync::Arc};

pub const MAX_ENTRIES: usize = 50;
#[derive(Debug, Clone)]
pub struct CompletionJob {
    pub tag: JobTag,
    pub snapshot: TextSnapshot,
    pub selection: Selection,
    pub prefix: TextRange,
}
#[derive(Debug, Clone)]
pub struct HostSuggestion {
    pub label: String,
    pub insert_text: Option<String>,
    pub detail: Option<String>,
}
pub trait HostCompletionProvider: Send + Sync {
    fn suggestions(
        &self,
        request: &IntelligenceRequest,
        prefix: &str,
    ) -> Result<Vec<HostSuggestion>>;
}
pub fn prefix_range(snapshot: &TextSnapshot, cursor: ByteOffset) -> Result<TextRange> {
    let line = snapshot.byte_to_position(cursor)?.line;
    let before = snapshot.slice(TextRange::new(snapshot.line_start(line)?, cursor)?)?;
    let count = before
        .chars_at(before.len_chars())
        .reversed()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$')
        .map(char::len_utf8)
        .sum::<usize>();
    TextRange::new(ByteOffset(cursor.0 - count), cursor)
}
pub fn prepare(
    job: &CompletionJob,
    external: Option<&dyn HostCompletionProvider>,
    provider: Option<&dyn IntelligenceProvider>,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<CompletionItem>> {
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    job.snapshot.slice(job.prefix)?;
    job.snapshot.byte_to_position(job.selection.head)?;
    let snapshot = job.snapshot.to_service_snapshot();
    let prefix = &snapshot.text[job.prefix.start.0..job.prefix.end.0];
    let request = IntelligenceRequest {
        tag: job.tag,
        snapshot: snapshot.clone(),
        position: job.selection.head,
    };
    let mut entries = vec![];
    let mut labels = HashSet::new();
    let mut add = |item: CompletionItem| -> Result<()> {
        if item.label.trim().is_empty() || entries.len() >= MAX_ENTRIES {
            return Ok(());
        }
        item.replacement_range.validate(&snapshot.text)?;
        if labels.insert(item.label.clone()) {
            entries.push(item);
        }
        Ok(())
    };
    if let Some(external) = external {
        for host in external.suggestions(&request, prefix)? {
            add(CompletionItem {
                insert_text: host.insert_text.unwrap_or_else(|| host.label.clone()),
                label: host.label,
                detail: host.detail,
                kind: None,
                replacement_range: job.prefix,
                source: "host".into(),
            })?;
        }
    }
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let provided = provider
        .map(|provider| provider.completions(&request))
        .transpose()?
        .unwrap_or_default();
    let authoritative = !provided.is_empty();
    for item in provided {
        add(item)?;
    }
    let line = job.snapshot.byte_to_position(job.prefix.start)?.line;
    let line_start = job.snapshot.line_start(line)?.0;
    let receiver = snapshot.text[line_start..job.prefix.start.0].trim_end();
    let member = receiver.ends_with('.') || receiver.ends_with("->") || receiver.ends_with("::");
    if !authoritative && !member {
        let mut start = None;
        for (offset, ch) in snapshot
            .text
            .char_indices()
            .chain(std::iter::once((snapshot.text.len(), ' ')))
        {
            if cancelled() {
                return Err(DomainError::Cancelled);
            }
            if ch.is_ascii_alphanumeric() || ch == '_' {
                start.get_or_insert(offset);
            } else if let Some(start) = start.take() {
                let name = &snapshot.text[start..offset];
                if !name.as_bytes()[0].is_ascii_digit() && name.starts_with(prefix) {
                    add(CompletionItem {
                        label: name.into(),
                        insert_text: name.into(),
                        detail: Some("local".into()),
                        kind: None,
                        replacement_range: job.prefix,
                        source: "local".into(),
                    })?;
                }
            }
        }
    }
    Ok(entries)
}
pub fn schedule(
    scheduler: &mut dyn WorkerScheduler<Vec<CompletionItem>>,
    owner: OwnerId,
    job: CompletionJob,
    external: Option<Arc<dyn HostCompletionProvider>>,
    provider: Option<Arc<dyn IntelligenceProvider>>,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Cancellable,
        Box::new(move |cancel| {
            prepare(&job, external.as_deref(), provider.as_deref(), || {
                cancel.is_cancelled()
            })
        }),
    )
}

#[derive(Debug, Clone, Copy)]
pub struct PopupGeometry {
    pub area: Rect,
    pub rows: Rect,
    pub text: Rect,
    pub track: Rect,
}
pub struct CompletionPopup {
    pub entries: Vec<CompletionItem>,
    pub selected: usize,
    pub offset: usize,
    pub auto_show_external: bool,
    pub error: Option<DomainError>,
    gate: ResultGate,
    context: Option<CompletionJob>,
    accepted_revision: Option<kode_core::Revision>,
}
impl CompletionPopup {
    pub fn new(workspace: WorkspaceGeneration) -> Self {
        Self {
            entries: vec![],
            selected: 0,
            offset: 0,
            auto_show_external: false,
            error: None,
            gate: ResultGate::new(workspace),
            context: None,
            accepted_revision: None,
        }
    }
    pub fn dismiss(&mut self) {
        self.entries.clear();
        self.context = None;
        self.selected = 0;
        self.offset = 0;
        self.gate.dismiss("completion");
        self.error = None;
        self.accepted_revision = None;
    }
    pub fn switch_workspace(&mut self) {
        self.dismiss();
        self.gate.switch_workspace();
    }
    pub fn begin(&mut self, snapshot: &TextSnapshot, state: &EditorState) -> Result<CompletionJob> {
        self.dismiss();
        let prefix = prefix_range(snapshot, state.caret.cursor())?;
        let tag = self
            .gate
            .request("completion".into(), Some((snapshot.id, snapshot.revision)));
        let job = CompletionJob {
            tag,
            snapshot: snapshot.clone(),
            selection: state.caret.selection,
            prefix,
        };
        self.context = Some(job.clone());
        Ok(job)
    }
    pub fn on_edit(
        &mut self,
        snapshot: &TextSnapshot,
        state: &EditorState,
        external_available: bool,
    ) -> Result<Option<CompletionJob>> {
        let accepted = self.accepted_revision == Some(snapshot.revision);
        self.dismiss();
        if accepted
            || !self.auto_show_external
            || !external_available
            || prefix_range(snapshot, state.caret.cursor())?.is_empty()
        {
            return Ok(None);
        }
        self.begin(snapshot, state).map(Some)
    }
    fn current(&self, snapshot: &TextSnapshot, state: &EditorState) -> bool {
        self.context.as_ref().is_some_and(|job| {
            job.snapshot.id == snapshot.id
                && job.snapshot.revision == snapshot.revision
                && job.selection == state.caret.selection
        })
    }
    pub fn complete(
        &mut self,
        tag: JobTag,
        result: Result<Vec<CompletionItem>>,
        snapshot: &TextSnapshot,
        state: &EditorState,
        focused: bool,
    ) -> Result<bool> {
        if !focused
            || state.search_has_focus
            || !self.current(snapshot, state)
            || !self.gate.accepts("completion", tag)
        {
            return Ok(false);
        }
        match result {
            Ok(entries) => {
                for entry in entries.iter().take(MAX_ENTRIES) {
                    snapshot.slice(entry.replacement_range)?;
                }
                let mut labels = HashSet::new();
                self.entries = entries
                    .into_iter()
                    .filter(|entry| {
                        !entry.label.trim().is_empty() && labels.insert(entry.label.clone())
                    })
                    .take(MAX_ENTRIES)
                    .collect();
                if self.entries.is_empty() {
                    self.dismiss();
                }
            }
            Err(error) => {
                self.dismiss();
                self.error = Some(error);
            }
        }
        self.gate.dismiss("completion");
        Ok(true)
    }
    pub fn geometry(&self, context: HookContext<'_>, state: &EditorState) -> Option<PopupGeometry> {
        let body = context.geometry.text;
        if self.entries.is_empty() || body.width == 0 || body.height < 2 {
            return None;
        }
        let job = self.context.as_ref()?;
        let (row, column) = context.layout.position_to_cell(job.selection.head).ok()?;
        let screen_y = body.y
            + (row
                .saturating_sub(state.scroll_rows)
                .min(body.height as usize - 1)) as u16;
        let above = screen_y - body.y;
        let below = body.bottom() - screen_y - 1;
        let height = ((self.entries.len() + 1) as u16)
            .min(above.max(below).max(2))
            .min(body.height);
        let visible = height - 1;
        let scrollbar = self.entries.len() > visible as usize;
        let width = (self
            .entries
            .iter()
            .map(|entry| {
                display_text(&entry.label).chars().count()
                    + entry
                        .detail
                        .as_ref()
                        .map_or(0, |detail| display_text(detail).chars().count() + 2)
            })
            .max()
            .unwrap_or(0)
            + 2
            + usize::from(scrollbar))
        .min(body.width as usize) as u16;
        let x = (body.x + column.min(body.width as usize - 1) as u16).min(body.right() - width);
        let y = if screen_y + height <= body.bottom() {
            screen_y
        } else if screen_y.saturating_sub(height) >= body.y {
            screen_y - height
        } else {
            body.y
        };
        let area = Rect::new(x, y, width, height);
        let track_width = u16::from(scrollbar).min(width);
        Some(PopupGeometry {
            area,
            rows: Rect::new(x, y + 1, width, visible),
            text: Rect::new(
                x + width.min(1),
                y + 1,
                width.saturating_sub(2 + track_width),
                visible,
            ),
            track: Rect::new(area.right() - track_width, y + 1, track_width, visible),
        })
    }
    fn clamp(&mut self, visible: usize, follow: bool) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = self.selected.min(self.entries.len() - 1);
        let visible = visible.max(1);
        if follow {
            if self.selected < self.offset {
                self.offset = self.selected;
            } else if self.selected >= self.offset + visible {
                self.offset = self.selected - visible + 1;
            }
        } else {
            self.selected = self.selected.clamp(
                self.offset,
                (self.offset + visible - 1).min(self.entries.len() - 1),
            );
        }
        self.offset = self.offset.min(self.entries.len().saturating_sub(visible));
    }
    pub fn render(
        &mut self,
        context: HookContext<'_>,
        state: &EditorState,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        let Some(geometry) = self.geometry(context, state) else {
            return;
        };
        self.clamp(geometry.rows.height as usize, true);
        let area = geometry.area.intersection(buffer.area);
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-search-bar"));
        for (row, item) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(geometry.rows.height as usize)
        {
            let y = geometry.rows.y + (row - self.offset) as u16;
            let label = display_text(&format!(
                "{}{}",
                item.label,
                item.detail
                    .as_ref()
                    .map_or(String::new(), |detail| format!("  {detail}"))
            ));
            let style = theme.style(if self.selected == row {
                "code-search-active"
            } else {
                "code-search-bar"
            });
            let rect =
                Rect::new(geometry.text.x, y, geometry.text.width, 1).intersection(buffer.area);
            if !rect.is_empty() {
                buffer.set_style(rect, style);
                buffer.set_stringn(rect.x, y, label, rect.width as usize, style);
            }
        }
        if !geometry.track.is_empty() {
            let indicator = self.offset * (geometry.track.height as usize - 1)
                / self
                    .entries
                    .len()
                    .saturating_sub(geometry.rows.height as usize)
                    .max(1);
            for row in 0..geometry.track.height {
                let point = Position::new(geometry.track.x, geometry.track.y + row);
                if buffer.area.contains(point) {
                    buffer[point]
                        .set_symbol(if row as usize == indicator {
                            "█"
                        } else {
                            "│"
                        })
                        .set_style(theme.style("code-popup-scrollbar"));
                }
            }
        }
    }
    pub fn accept(&mut self, document: &mut Document, state: &mut EditorState) -> Result<bool> {
        if !self.current(&document.snapshot(), state) {
            self.dismiss();
            return Ok(false);
        }
        let Some(item) = self.entries.get(self.selected).cloned() else {
            return Ok(false);
        };
        document.snapshot().slice(item.replacement_range)?;
        edit::replace(
            document,
            &mut state.caret,
            item.replacement_range,
            &item.insert_text,
            EditKind::Completion,
        )?;
        self.dismiss();
        self.accepted_revision = Some(document.revision());
        Ok(true)
    }
}
impl EditorInputHooks for CompletionPopup {
    fn context_changed(&mut self, snapshot: &TextSnapshot, state: &EditorState) {
        if self.context.is_some() && (!self.current(snapshot, state) || state.search_has_focus) {
            self.dismiss();
        }
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
            self.dismiss();
            return Ok(false);
        }
        if target != InputTarget::Popup {
            return Ok(false);
        }
        self.context_changed(&document.snapshot(), state);
        let Some(geometry) = self.geometry(context, state) else {
            if matches!(event,InputEvent::Key(input) if input.key == Key::Escape)
                && self.context.is_some()
            {
                self.dismiss();
                return Ok(true);
            }
            return Ok(false);
        };
        match event {
            InputEvent::Key(input) => {
                let visible = geometry.rows.height as usize;
                match input.key {
                    Key::Escape => self.dismiss(),
                    Key::Enter => {
                        self.accept(document, state)?;
                    }
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::Down => self.selected = (self.selected + 1).min(self.entries.len() - 1),
                    Key::PageUp => self.selected = self.selected.saturating_sub(visible),
                    Key::PageDown => {
                        self.selected = (self.selected + visible).min(self.entries.len() - 1)
                    }
                    Key::Home => self.selected = 0,
                    Key::End => self.selected = self.entries.len() - 1,
                    _ => return Ok(false),
                }
                self.clamp(visible, true);
                Ok(true)
            }
            InputEvent::Pointer(pointer) => {
                let point = Position::new(pointer.x, pointer.y);
                let inside = geometry.area.contains(point);
                match pointer.action {
                    PointerAction::Scroll { vertical, .. } if inside => {
                        self.offset = self.offset.saturating_add_signed(vertical as isize).min(
                            self.entries
                                .len()
                                .saturating_sub(geometry.rows.height as usize),
                        );
                        self.clamp(geometry.rows.height as usize, false);
                        Ok(true)
                    }
                    PointerAction::Scroll { .. } => {
                        self.dismiss();
                        Ok(false)
                    }
                    PointerAction::Down(_) if !inside => {
                        self.dismiss();
                        Ok(true)
                    }
                    PointerAction::Down(MouseButton::Left) if geometry.track.contains(point) => {
                        self.offset = (pointer.y - geometry.track.y) as usize
                            * self
                                .entries
                                .len()
                                .saturating_sub(geometry.rows.height as usize)
                            / (geometry.track.height as usize - 1).max(1);
                        self.selected = self.offset;
                        Ok(true)
                    }
                    PointerAction::Down(MouseButton::Left) if geometry.rows.contains(point) => {
                        self.selected = self.offset + (pointer.y - geometry.rows.y) as usize;
                        self.accept(document, state)
                    }
                    PointerAction::Move if geometry.rows.contains(point) => {
                        self.selected = self.offset + (pointer.y - geometry.rows.y) as usize;
                        Ok(true)
                    }
                    _ => Ok(inside),
                }
            }
            _ => Ok(false),
        }
    }
}
