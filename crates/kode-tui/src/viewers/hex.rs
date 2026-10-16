//! Portable binary view. Source-compatible edits affect memory only; no direct
//! hex-digit entry or physical binary save command is invented here.
pub mod find;
pub mod search;
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::{controls::draw_text, field::FieldAction},
};
use crossterm::event::MouseButton;
use kode_core::{
    RequestId, Result,
    document::bytes::{ByteBuffer, ByteIndex, ByteViewport},
};
use kode_platform::jobs::CancellationToken;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Clear, Widget},
};
use search::{Direction, SearchJob, SearchReport};
use std::{ops::Range, path::PathBuf};

#[derive(Clone)]
pub struct PasteProof {
    request: Option<RequestId>,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
    mode: search::SearchMode,
}
pub enum HexWork {
    Search(SearchJob),
    Copy(String),
    Paste(PasteProof),
}
struct PendingSearch {
    request: RequestId,
    source: ByteBuffer,
    proof: PasteProof,
    cancel: CancellationToken,
}

pub struct HexView {
    pub id: FocusId,
    pub path: PathBuf,
    pub mime: String,
    pub buffer: ByteBuffer,
    pub scroll_row: usize,
    pub modified: bool,
    pub find: find::FindBar,
    width: usize,
    area: Rect,
    body: Rect,
    horizontal: usize,
    scroll_anchor: usize,
    controls: Vec<(Rect, usize)>,
    pending: Option<PendingSearch>,
    pending_paste: Option<RequestId>,
    last_match: Option<Range<usize>>,
    work: Vec<HexWork>,
    dragging: bool,
    window_focused: bool,
}
impl HexView {
    pub fn new(path: PathBuf, mime: String, bytes: Vec<u8>) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            path,
            mime,
            buffer: ByteBuffer::new(bytes),
            scroll_row: 0,
            modified: false,
            find: find::FindBar::new()?,
            width: 16,
            area: Rect::default(),
            body: Rect::default(),
            horizontal: 0,
            scroll_anchor: 0,
            controls: vec![],
            pending: None,
            pending_paste: None,
            last_match: None,
            work: vec![],
            dragging: false,
            window_focused: true,
        })
    }
    pub fn bytes_per_row(&self) -> usize {
        self.width
    }
    pub fn take_work(&mut self) -> Vec<HexWork> {
        std::mem::take(&mut self.work)
    }
    pub fn searching(&self) -> bool {
        self.pending.is_some()
    }
    pub fn contains_focus(&self, focus: &FocusState) -> bool {
        focus.focused == Some(self.id) || (self.find.visible && focus.focused == Some(self.find.id))
    }
    pub fn set_bytes_per_row(&mut self, width: usize) -> bool {
        if ![8, 16, 32].contains(&width) {
            return false;
        }
        if self.width == width {
            return true;
        }
        let first_byte = if self.scroll_anchor / self.width == self.scroll_row {
            self.scroll_anchor
        } else {
            self.scroll_row.saturating_mul(self.width)
        };
        self.width = width;
        self.scroll_row = (first_byte / width).min(self.max_row());
        self.scroll_anchor = first_byte;
        self.horizontal = 0;
        self.reveal_column(
            self.buffer
                .cursor()
                .0
                .min(self.buffer.len().saturating_sub(1)),
        );
        true
    }
    fn proof(&self) -> PasteProof {
        PasteProof {
            request: None,
            value: self.find.field.value.clone(),
            cursor: self.find.field.cursor,
            anchor: self.find.field.anchor,
            mode: self.find.mode,
        }
    }
    fn matches_proof(&self, proof: &PasteProof) -> bool {
        self.find.visible
            && proof.value == self.find.field.value
            && proof.cursor == self.find.field.cursor
            && proof.anchor == self.find.field.anchor
            && proof.mode == self.find.mode
    }
    fn cancel_search(&mut self) {
        if let Some(pending) = self.pending.take() {
            pending.cancel.cancel();
        }
        self.work.retain(|work| !matches!(work, HexWork::Search(_)));
    }
    fn invalidate_paste(&mut self) {
        self.pending_paste = None;
        self.work.retain(|work| !matches!(work, HexWork::Paste(_)));
    }
    fn changed_pattern(&mut self) {
        self.cancel_search();
        self.last_match = None;
        self.find.status = if self.find.mode == search::SearchMode::Text {
            "UTF-8 text; Enter: next"
        } else {
            "Hex bytes: 00 FF"
        }
        .into();
    }
    fn open_find(&mut self, focus: &mut FocusState) {
        self.dragging = false;
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
        self.find.visible = true;
        focus.focused = Some(self.find.id);
    }
    fn search(&mut self, direction: Direction) {
        self.cancel_search();
        let pattern = match self.find.mode.pattern(&self.find.field.value) {
            Ok(pattern) => pattern,
            Err(error) => {
                self.find.status = error.into();
                return;
            }
        };
        let request = match RequestId::fresh() {
            Ok(id) => id,
            Err(error) => {
                self.find.status = error.to_string();
                return;
            }
        };
        let found = self
            .last_match
            .as_ref()
            .filter(|range| self.buffer.selection().as_ref() == Some(range));
        let from = found.map_or(self.buffer.cursor().0, |range| {
            if direction == Direction::Next {
                range.start + 1
            } else {
                range.start
            }
        });
        let source = self.buffer.clone();
        let cancel = CancellationToken::new();
        self.pending = Some(PendingSearch {
            request,
            source: source.clone(),
            proof: self.proof(),
            cancel: cancel.clone(),
        });
        self.work.push(HexWork::Search(SearchJob {
            request,
            source,
            pattern,
            from,
            direction,
            cancel,
        }));
        self.find.status = "Searching; Esc stops".into();
    }
    pub fn complete_search(&mut self, request: RequestId, result: Result<SearchReport>) -> bool {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.request != request)
        {
            return false;
        }
        let pending = self.pending.take().expect("matched search");
        if !self.matches_proof(&pending.proof)
            || !self.buffer.same_snapshot(&pending.source)
            || self.buffer.cursor() != pending.source.cursor()
            || self.buffer.anchor() != pending.source.anchor()
        {
            pending.cancel.cancel();
            self.find.status = "Search stopped".into();
            return false;
        }
        match result {
            Ok(report) => {
                self.last_match = report.range.clone();
                if let Some(range) = report.range {
                    if range.start >= range.end || range.end > self.buffer.len() {
                        self.find.status = "Invalid search result".into();
                        return false;
                    }
                    self.buffer.start_selection(ByteIndex(range.start));
                    self.buffer.select_to(ByteIndex(range.end));
                    self.reveal_offset(range.start);
                    self.find.status = format!(
                        "Match @ {:08X}{}",
                        range.start,
                        if report.wrapped { " [wrap]" } else { "" }
                    );
                } else {
                    self.find.status = "No match".into();
                }
            }
            Err(kode_core::DomainError::Cancelled) => self.find.status = "Search stopped".into(),
            Err(error) => self.find.status = error.to_string(),
        }
        true
    }
    pub fn complete_clipboard(
        &mut self,
        proof: &PasteProof,
        result: Result<String>,
        focus: &FocusState,
    ) -> bool {
        if proof.request.is_none() || proof.request != self.pending_paste {
            return false;
        }
        self.pending_paste = None;
        if focus.focused != Some(self.find.id) || !self.matches_proof(proof) {
            return false;
        }
        match result {
            Ok(text) => {
                if let Some(action) = self.find.field.handle(&InputEvent::Paste(text)) {
                    if action == FieldAction::Changed {
                        self.changed_pattern();
                    } else if action == FieldAction::LimitReached {
                        self.find.status = "Pattern too long".into();
                    }
                }
            }
            Err(error) => self.find.status = error.to_string(),
        }
        true
    }
    pub fn is_dragging(&self, focus: &FocusState) -> bool {
        self.dragging && focus.captured == Some(self.id)
    }
    pub fn detach(&mut self, focus: &mut FocusState) {
        self.cancel_search();
        self.invalidate_paste();
        self.dragging = false;
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
    }
    fn max_row(&self) -> usize {
        self.buffer.len().div_ceil(self.width).saturating_sub(1)
    }
    fn reveal(&mut self) {
        self.reveal_offset(self.buffer.cursor().0);
    }
    fn reveal_offset(&mut self, offset: usize) {
        let previous = self.scroll_row;
        let height = self.body.height.max(1) as usize;
        let row = offset / self.width;
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + height {
            self.scroll_row = row + 1 - height;
        }
        self.scroll_row = self.scroll_row.min(self.max_row());
        if previous != self.scroll_row {
            self.scroll_anchor = self.scroll_row * self.width;
        }
        self.reveal_column(offset.min(self.buffer.len().saturating_sub(1)));
    }
    fn reveal_column(&mut self, offset: usize) {
        let available = self.area.width.saturating_sub(10) as usize;
        if available == 0 {
            self.horizontal = 0;
            return;
        }
        let column = (offset % self.width) * 3;
        if column < self.horizontal {
            self.horizontal = column;
        } else if column + 2 > self.horizontal + available {
            self.horizontal = column + 2 - available;
        }
    }
    pub fn hit_test(&self, x: u16, y: u16) -> Option<ByteIndex> {
        if !self.body.contains(Position::new(x, y)) {
            return None;
        }
        let x = (x - self.area.x) as usize;
        let column = x.saturating_sub(10) + self.horizontal;
        let ascii = self.width * 3 + 2;
        let local = if x < 10 {
            return None;
        } else if column >= ascii && column < ascii + self.width {
            column - ascii
        } else if column < self.width * 3 {
            column / 3
        } else {
            return None;
        };
        let index = (self.scroll_row + (y - self.body.y) as usize) * self.width + local;
        (index < self.buffer.len()).then_some(ByteIndex(index))
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        if matches!(
            event,
            InputEvent::Key(_) | InputEvent::Paste(_) | InputEvent::Focus(_)
        ) || matches!(event, InputEvent::Pointer(pointer) if matches!(pointer.action, PointerAction::Down(_)))
        {
            self.invalidate_paste();
        }
        if let InputEvent::Focus(active) = event {
            self.window_focused = *active;
            if !active {
                self.detach(focus);
            }
            return true;
        }
        if !self.window_focused {
            return false;
        }
        if let InputEvent::Key(input) = event
            && self.contains_focus(focus)
            && let Some(input) =
                crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Hex, *input)
        {
            if input.key == Key::Escape && self.find.visible {
                self.cancel_search();
                self.find.visible = false;
                focus.focused = Some(self.id);
                return true;
            }
            if input.modifiers.ctrl
                && !input.modifiers.alt
                && matches!(input.key, Key::Character('f' | 'F'))
            {
                self.open_find(focus);
                return true;
            }
            if !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta {
                if input.key == Key::Function(8) {
                    self.set_bytes_per_row(match self.width {
                        8 => 16,
                        16 => 32,
                        _ => 8,
                    });
                    return true;
                }
                if input.key == Key::Function(3) {
                    self.find.visible = true;
                    if self.find.field.value.is_empty() {
                        focus.focused = Some(self.find.id);
                    }
                    self.search(if input.modifiers.shift {
                        Direction::Previous
                    } else {
                        Direction::Next
                    });
                    return true;
                }
            }
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(MouseButton::Left)
            && let Some((_, width)) = self
                .controls
                .iter()
                .find(|(hit, _)| hit.contains(Position::new(pointer.x, pointer.y)))
        {
            let width = *width;
            self.dragging = false;
            if focus.captured == Some(self.id) {
                focus.captured = None;
            }
            if width == 0 {
                self.open_find(focus);
            } else {
                self.set_bytes_per_row(width);
                focus.focused = Some(self.id);
            }
            return true;
        }
        let before = self.find.field.value.clone();
        if let Some(action) = self.find.dispatch(event, focus) {
            if before != self.find.field.value && !matches!(action, find::Action::Changed) {
                self.changed_pattern();
            }
            match action {
                find::Action::None => {}
                find::Action::Changed => self.changed_pattern(),
                find::Action::Search(direction) => self.search(direction),
                find::Action::Stop => {
                    self.cancel_search();
                    self.find.status = "Search stopped".into();
                }
                find::Action::Close => {
                    self.cancel_search();
                    self.find.visible = false;
                    focus.focused = Some(self.id);
                }
                find::Action::Field(FieldAction::Copy(value)) => {
                    self.work.push(HexWork::Copy(value))
                }
                find::Action::Field(FieldAction::PasteRequested) => match RequestId::fresh() {
                    Ok(request) => {
                        self.pending_paste = Some(request);
                        let mut proof = self.proof();
                        proof.request = Some(request);
                        self.work.push(HexWork::Paste(proof));
                    }
                    Err(error) => self.find.status = error.to_string(),
                },
                find::Action::Field(FieldAction::LimitReached) => {
                    self.find.status = "Pattern too long".into()
                }
                find::Action::Field(FieldAction::Changed) => self.changed_pattern(),
            }
            return true;
        }
        match event {
            InputEvent::Pointer(pointer) => match pointer.action {
                PointerAction::Down(MouseButton::Left) => {
                    let Some(index) = self.hit_test(pointer.x, pointer.y) else {
                        return false;
                    };
                    self.cancel_search();
                    self.last_match = None;
                    focus.focused = Some(self.id);
                    focus.captured = Some(self.id);
                    self.dragging = true;
                    self.buffer.start_selection(index);
                    self.reveal();
                }
                PointerAction::Drag(MouseButton::Left) | PointerAction::Move
                    if self.is_dragging(focus) =>
                {
                    if let Some(index) = self.hit_test(pointer.x, pointer.y) {
                        self.buffer.select_to(index);
                        self.reveal();
                    }
                }
                PointerAction::Up(MouseButton::Left) if self.is_dragging(focus) => {
                    self.detach(focus);
                }
                PointerAction::Scroll {
                    vertical,
                    horizontal,
                } if self.area.contains(Position::new(pointer.x, pointer.y)) => {
                    self.cancel_search();
                    self.scroll_row = self
                        .scroll_row
                        .saturating_add_signed(vertical as isize)
                        .min(self.max_row());
                    if vertical != 0 {
                        self.scroll_anchor = self.scroll_row * self.width;
                    }
                    self.horizontal = self
                        .horizontal
                        .saturating_add_signed(horizontal as isize * 3)
                        .min(
                            (4 * self.width + 2)
                                .saturating_sub(self.area.width.saturating_sub(10) as usize),
                        );
                }
                _ => return false,
            },
            InputEvent::Key(input) if focus.focused == Some(self.id) && !input.modifiers.meta => {
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Hex,
                    *input,
                ) else {
                    return false;
                };
                let expand = input.modifiers.shift;
                let command = match input.key {
                    Key::Character(ch) => Key::Character(ch.to_ascii_lowercase()),
                    key => key,
                };
                self.cancel_search();
                match command {
                    Key::Character('c') if input.modifiers.alt && !input.modifiers.ctrl => {
                        self.buffer.copy_selection();
                    }
                    Key::Character('x') if input.modifiers.alt && !input.modifiers.ctrl => {
                        self.modified |= self.buffer.cut_selection()
                    }
                    Key::Character('v') if input.modifiers.alt && !input.modifiers.ctrl => {
                        self.modified |= self.buffer.paste_clipboard()
                    }
                    Key::Character('a') if input.modifiers.ctrl && !input.modifiers.alt => {
                        self.buffer.select_all()
                    }
                    _ if input.modifiers.alt || input.modifiers.ctrl => return false,
                    Key::Left => self.buffer.move_left(expand),
                    Key::Right => self.buffer.move_right(expand),
                    Key::Up => self.buffer.move_up(self.width, expand),
                    Key::Down => self.buffer.move_down(self.width, expand),
                    Key::Home => self.buffer.move_to(
                        ByteIndex(self.buffer.cursor().0 / self.width * self.width),
                        expand,
                    ),
                    Key::End => self.buffer.move_to(
                        ByteIndex((self.buffer.cursor().0 / self.width + 1) * self.width),
                        expand,
                    ),
                    Key::PageUp | Key::PageDown => {
                        let page = self.body.height.max(1) as usize;
                        let row = self.buffer.cursor().0 / self.width;
                        let down = command == Key::PageDown;
                        let target = if down {
                            row.saturating_add(page)
                        } else {
                            row.saturating_sub(page)
                        };
                        self.buffer
                            .move_to(ByteIndex(target.saturating_mul(self.width)), false);
                        self.scroll_row = if down {
                            self.scroll_row.saturating_add(page)
                        } else {
                            self.scroll_row.saturating_sub(page)
                        }
                        .min(self.max_row());
                    }
                    Key::Backspace => self.modified |= self.buffer.delete_backspace(),
                    Key::Delete => self.modified |= self.buffer.delete_forward(),
                    _ => return false,
                }
                self.reveal();
            }
            _ => return false,
        }
        true
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        focus: &FocusState,
    ) -> Option<Position> {
        let resized = area.width != self.area.width;
        self.area = area.intersection(buffer.area);
        let area = self.area;
        self.controls.clear();
        let find_height = if self.find.visible {
            area.height.saturating_sub(1).min(3)
        } else {
            0
        };
        self.body = Rect::new(
            area.x,
            area.y.saturating_add(1 + find_height),
            area.width,
            area.height.saturating_sub(1 + find_height),
        );
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-body"));
        if area.is_empty() {
            return None;
        }
        if resized {
            self.reveal_column(
                self.buffer
                    .cursor()
                    .0
                    .min(self.buffer.len().saturating_sub(1)),
            );
        }
        let labels = [("[8]", 8), ("[16]", 16), ("[32]", 32), ("[Find]", 0)];
        let controls_width: u16 = labels.iter().map(|(label, _)| label.len() as u16).sum();
        let title_width = area.width.saturating_sub(controls_width);
        draw_text(
            Rect::new(area.x, area.y, title_width, 1),
            buffer,
            &format!(
                "{} [{}] · {} bytes{}",
                display_text(&self.path.to_string_lossy()),
                display_text(&self.mime),
                self.buffer.len(),
                if self.modified {
                    " · edits in memory"
                } else {
                    ""
                }
            ),
            theme.style("code-header"),
        );
        let mut x = area.x + title_width;
        for (label, width) in labels {
            let hit = Rect::new(
                x,
                area.y,
                (label.len() as u16).min(area.right().saturating_sub(x)),
                1,
            );
            draw_text(
                hit,
                buffer,
                label,
                theme.style(if width == self.width {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
            self.controls.push((hit, width));
            x += hit.width;
        }
        let field_cursor = if self.find.visible {
            self.find.render(
                Rect::new(area.x, area.y + 1, area.width, find_height),
                buffer,
                theme,
                focus,
            )
        } else {
            None
        };
        self.scroll_row = self.scroll_row.min(self.max_row());
        let rows = self.buffer.viewport(ByteViewport {
            start: ByteIndex(self.scroll_row * self.width),
            bytes_per_row: self.width,
            rows: self.body.height as usize,
        });
        let body = theme.style("code-body");
        let selection = inverted(theme.style("code-selection"), body);
        let cursor = inverted(theme.style("code-cursor"), body);
        for (y, row) in rows.iter().enumerate() {
            let y = self.body.y + y as u16;
            draw_text(
                Rect::new(area.x, y, area.width.min(10), 1),
                buffer,
                &format!("{:08X}", row.offset.0),
                theme.style("code-gutter"),
            );
            for (local, byte) in row.bytes.iter().enumerate() {
                let style = if focus.focused == Some(self.id)
                    && self.window_focused
                    && row.offset.0 + local == self.buffer.cursor().0
                {
                    cursor
                } else if row.selected[local] {
                    selection
                } else {
                    body
                };
                for (x, text) in [
                    (local * 3, format!("{byte:02X}")),
                    (
                        self.width * 3 + 2 + local,
                        if (32..=126).contains(byte) {
                            (*byte as char).to_string()
                        } else {
                            ".".into()
                        },
                    ),
                ] {
                    // Retain the offset gutter while scrolling the byte columns.
                    for (column, ch) in text.chars().enumerate() {
                        let Some(x) = (x + column).checked_sub(self.horizontal).map(|x| x + 10)
                        else {
                            continue;
                        };
                        if x >= area.width as usize {
                            continue;
                        }
                        draw_text(
                            Rect::new(area.x + x as u16, y, 1, 1),
                            buffer,
                            &ch.to_string(),
                            style,
                        );
                    }
                }
            }
        }
        field_cursor
    }
}
impl Drop for HexView {
    fn drop(&mut self) {
        self.cancel_search();
    }
}
fn inverted(style: Style, body: Style) -> Style {
    style
        .fg(style.fg.or(body.bg).unwrap_or(ratatui::style::Color::Black))
        .bg(style.bg.or(body.fg).unwrap_or(ratatui::style::Color::White))
}
