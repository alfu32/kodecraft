//! Wrapped, scrollable informational text with stable source-line hit testing.
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::{RequestId, Result};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};

pub struct TextPanel {
    pub id: FocusId,
    pub scroll: usize,
    lines: Vec<String>,
    wrapped: Vec<(usize, String)>,
    width: Option<u16>,
    area: Rect,
}
impl TextPanel {
    pub fn new(lines: Vec<String>) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            scroll: 0,
            lines,
            wrapped: Vec::new(),
            width: None,
            area: Rect::default(),
        })
    }
    pub fn set_lines(&mut self, lines: Vec<String>) {
        if lines != self.lines {
            self.lines = lines;
            self.width = None;
        }
    }
    pub fn hide(&mut self) {
        self.area = Rect::default();
    }
    fn limit(&self) -> usize {
        self.wrapped.len().saturating_sub(self.area.height as usize)
    }
    pub fn line_at(&self, position: Position) -> Option<usize> {
        self.area
            .contains(position)
            .then(|| {
                self.wrapped
                    .get(self.scroll + (position.y - self.area.y) as usize)
                    .map(|(line, _)| *line)
            })
            .flatten()
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        let delta = match event {
            InputEvent::Pointer(pointer)
                if self.area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                match pointer.action {
                    PointerAction::Down(MouseButton::Left) => {
                        focus.focused = Some(self.id);
                        return true;
                    }
                    PointerAction::Scroll { vertical, .. } => vertical as isize,
                    _ => return false,
                }
            }
            InputEvent::Key(input)
                if focus.focused == Some(self.id)
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                match input.key {
                    Key::Up => -1,
                    Key::Down => 1,
                    Key::PageUp => -(self.area.height.max(1) as isize),
                    Key::PageDown => self.area.height.max(1) as isize,
                    Key::Home => {
                        self.scroll = 0;
                        return true;
                    }
                    Key::End => {
                        self.scroll = self.limit();
                        return true;
                    }
                    _ => return false,
                }
            }
            _ => return false,
        };
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.limit());
        true
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.area = area.intersection(buffer.area);
        let area = self.area;
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("content"));
        if self.width != Some(area.width) {
            self.wrapped.clear();
            for (index, line) in self.lines.iter().enumerate() {
                for chunk in wrap_line(line, area.width) {
                    self.wrapped.push((index, chunk));
                }
            }
            self.width = Some(area.width);
        }
        self.scroll = self.scroll.min(self.limit());
        for (row, (_, text)) in self
            .wrapped
            .iter()
            .skip(self.scroll)
            .take(area.height as usize)
            .enumerate()
        {
            draw_text(
                Rect::new(area.x, area.y + row as u16, area.width, 1),
                buffer,
                text,
                theme.style("content"),
            );
        }
    }
}

/// Wrap using terminal cell widths, escaping a wide scalar in a one-cell view.
pub fn wrap_line(line: &str, width: u16) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let width = usize::from(width.max(1));
    let mut safe = String::new();
    for ch in display_text(line).chars() {
        if ch.width().unwrap_or(0) > width {
            safe.push_str(&format!("\\u{{{:X}}}", ch as u32));
        } else {
            safe.push(ch);
        }
    }
    let mut lines = vec![];
    let mut chunk = String::new();
    let mut used = 0;
    for ch in safe.chars() {
        let size = ch.width().unwrap_or(0);
        if used + size > width && !chunk.is_empty() {
            lines.push(std::mem::take(&mut chunk));
            used = 0;
        }
        chunk.push(ch);
        used += size;
    }
    lines.push(chunk);
    lines
}
