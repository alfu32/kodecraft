//! Portable binary view. Source-compatible edits affect memory only; no direct
//! hex-digit entry or physical binary save command is invented here.
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::{
    RequestId, Result,
    document::bytes::{ByteBuffer, ByteIndex, ByteViewport},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Clear, Widget},
};
use std::path::PathBuf;

pub struct HexView {
    pub id: FocusId,
    pub path: PathBuf,
    pub mime: String,
    pub buffer: ByteBuffer,
    pub scroll_row: usize,
    pub modified: bool,
    width: usize,
    area: Rect,
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
            width: 16,
            area: Rect::default(),
            dragging: false,
            window_focused: true,
        })
    }
    pub fn bytes_per_row(&self) -> usize {
        self.width
    }
    pub fn is_dragging(&self, focus: &FocusState) -> bool {
        self.dragging && focus.captured == Some(self.id)
    }
    pub fn detach(&mut self, focus: &mut FocusState) {
        self.dragging = false;
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
    }
    fn max_row(&self) -> usize {
        self.buffer.len().div_ceil(self.width).saturating_sub(1)
    }
    fn reveal(&mut self) {
        let height = self.area.height.saturating_sub(1).max(1) as usize;
        let row = self.buffer.cursor().0 / self.width;
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + height {
            self.scroll_row = row + 1 - height;
        }
        self.scroll_row = self.scroll_row.min(self.max_row());
    }
    pub fn hit_test(&self, x: u16, y: u16) -> Option<ByteIndex> {
        if !self.area.contains(Position::new(x, y)) || y == self.area.y {
            return None;
        }
        let x = (x - self.area.x) as usize;
        let ascii = 10 + self.width * 3 + 2;
        let local = if x >= ascii {
            (x - ascii).min(self.width - 1)
        } else if x >= 10 {
            ((x - 10) / 3).min(self.width - 1)
        } else {
            return None;
        };
        let index = (self.scroll_row + (y - self.area.y - 1) as usize) * self.width + local;
        (index < self.buffer.len()).then_some(ByteIndex(index))
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
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
        match event {
            InputEvent::Pointer(pointer) => match pointer.action {
                PointerAction::Down(MouseButton::Left) => {
                    let Some(index) = self.hit_test(pointer.x, pointer.y) else {
                        return false;
                    };
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
                PointerAction::Scroll { vertical, .. }
                    if self.area.contains(Position::new(pointer.x, pointer.y)) =>
                {
                    self.scroll_row = self
                        .scroll_row
                        .saturating_add_signed(vertical as isize)
                        .min(self.max_row());
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
                        let page = self.area.height.saturating_sub(1).max(1) as usize;
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
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        self.area = area.intersection(buffer.area);
        let area = self.area;
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-body"));
        if area.is_empty() {
            return;
        }
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
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
        self.scroll_row = self.scroll_row.min(self.max_row());
        let rows = self.buffer.viewport(ByteViewport {
            start: ByteIndex(self.scroll_row * self.width),
            bytes_per_row: self.width,
            rows: area.height.saturating_sub(1) as usize,
        });
        let body = theme.style("code-body");
        let selection = inverted(theme.style("code-selection"), body);
        let cursor = inverted(theme.style("code-cursor"), body);
        for (y, row) in rows.iter().enumerate() {
            let y = area.y + y as u16 + 1;
            draw_text(
                Rect::new(area.x, y, area.width.min(10), 1),
                buffer,
                &format!("{:08X}", row.offset.0),
                theme.style("code-gutter"),
            );
            for (local, byte) in row.bytes.iter().enumerate() {
                let style = if focused
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
                    (10 + local * 3, format!("{byte:02X}")),
                    (
                        10 + self.width * 3 + 2 + local,
                        if (32..=126).contains(byte) {
                            (*byte as char).to_string()
                        } else {
                            ".".into()
                        },
                    ),
                ] {
                    if x < area.width as usize {
                        draw_text(
                            Rect::new(
                                area.x + x as u16,
                                y,
                                (area.width - x as u16).min(text.len() as u16),
                                1,
                            ),
                            buffer,
                            &text,
                            style,
                        );
                    }
                }
            }
        }
    }
}
fn inverted(style: Style, body: Style) -> Style {
    style
        .fg(style.fg.or(body.bg).unwrap_or(ratatui::style::Color::Black))
        .bg(style.bg.or(body.fg).unwrap_or(ratatui::style::Color::White))
}
