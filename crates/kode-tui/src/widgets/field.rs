use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
};
use crossterm::event::MouseButton;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldAction {
    Changed,
    Copy(String),
    PasteRequested,
    LimitReached,
}

#[derive(Debug, Clone)]
pub struct TextField {
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub masked: bool,
    pub read_only: bool,
    pub max_chars: usize,
}
impl TextField {
    pub fn new(value: String) -> Self {
        let cursor = value.len();
        Self {
            value,
            cursor,
            anchor: None,
            masked: false,
            read_only: false,
            max_chars: 16384,
        }
    }
    fn range(&self) -> std::ops::Range<usize> {
        let anchor = self.anchor.unwrap_or(self.cursor);
        anchor.min(self.cursor)..anchor.max(self.cursor)
    }
    fn move_to(&mut self, position: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = position;
    }
    fn replace(&mut self, text: &str) -> Option<FieldAction> {
        if self.read_only {
            return None;
        }
        let text: String = text
            .chars()
            .filter(|ch| *ch != '\n' && *ch != '\r')
            .collect();
        let range = self.range();
        if self.value[..range.start].chars().count()
            + self.value[range.end..].chars().count()
            + text.chars().count()
            > self.max_chars
        {
            return Some(FieldAction::LimitReached);
        }
        if range.is_empty() && text.is_empty() {
            return None;
        }
        let cursor = range.start + text.len();
        self.value.replace_range(range, &text);
        self.cursor = cursor;
        self.anchor = None;
        Some(FieldAction::Changed)
    }
    pub fn handle(&mut self, event: &InputEvent) -> Option<FieldAction> {
        match event {
            InputEvent::Paste(text) => self.replace(text),
            InputEvent::Key(input) => {
                let extend = input.modifiers.shift;
                match input.key {
                    Key::Character('a' | 'A') if input.modifiers.ctrl => {
                        self.anchor = Some(0);
                        self.cursor = self.value.len();
                        None
                    }
                    Key::Character('c' | 'C') if input.modifiers.ctrl => {
                        let range = self.range();
                        Some(FieldAction::Copy(if range.is_empty() {
                            self.value.clone()
                        } else {
                            self.value[range].into()
                        }))
                    }
                    Key::Character('x' | 'X') if input.modifiers.ctrl && !self.read_only => {
                        let range = self.range();
                        let value = if range.is_empty() {
                            self.value.clone()
                        } else {
                            self.value[range].into()
                        };
                        if self.range().is_empty() {
                            self.anchor = Some(0);
                            self.cursor = self.value.len();
                        }
                        self.replace("");
                        Some(FieldAction::Copy(value))
                    }
                    Key::Character('v' | 'V') if input.modifiers.ctrl && !self.read_only => {
                        Some(FieldAction::PasteRequested)
                    }
                    Key::Character(ch)
                        if !input.modifiers.ctrl
                            && !input.modifiers.alt
                            && !input.modifiers.meta
                            && !ch.is_control() =>
                    {
                        self.replace(&ch.to_string())
                    }
                    Key::Left => {
                        self.move_to(
                            self.value[..self.cursor]
                                .char_indices()
                                .next_back()
                                .map_or(0, |(index, _)| index),
                            extend,
                        );
                        None
                    }
                    Key::Right => {
                        let next = self.value[self.cursor..]
                            .chars()
                            .next()
                            .map_or(self.cursor, |ch| self.cursor + ch.len_utf8());
                        self.move_to(next, extend);
                        None
                    }
                    Key::Home => {
                        self.move_to(0, extend);
                        None
                    }
                    Key::End => {
                        self.move_to(self.value.len(), extend);
                        None
                    }
                    Key::Backspace if !self.read_only => {
                        if self.range().is_empty() {
                            self.anchor = Some(
                                self.value[..self.cursor]
                                    .char_indices()
                                    .next_back()
                                    .map_or(0, |(index, _)| index),
                            );
                        }
                        self.replace("")
                    }
                    Key::Delete if !self.read_only => {
                        if self.range().is_empty() {
                            self.anchor = Some(
                                self.cursor
                                    + self.value[self.cursor..]
                                        .chars()
                                        .next()
                                        .map_or(0, char::len_utf8),
                            );
                        }
                        self.replace("")
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    fn display_char(&self, ch: char) -> String {
        if self.masked {
            "*".into()
        } else if ch.is_control() {
            format!("\\x{:02X}", ch as u32)
        } else {
            ch.to_string()
        }
    }
    fn window(&self, width: usize) -> usize {
        let mut start = self.cursor;
        let mut used = 1;
        for (index, ch) in self.value[..self.cursor].char_indices().rev() {
            let count = self.display_char(ch).width();
            if used + count > width {
                break;
            }
            used += count;
            start = index;
        }
        start
    }
    pub fn hit_test(&mut self, area: Rect, x: u16, y: u16) -> bool {
        if !area.contains(Position::new(x, y)) || area.is_empty() {
            return false;
        }
        let inner = if area.width >= 3 {
            Rect::new(area.x + 1, area.y, area.width - 2, 1)
        } else {
            Rect::new(area.x, area.y, area.width, 1)
        };
        if !inner.contains(Position::new(x, y)) {
            return false;
        }
        let target = (x - inner.x) as usize;
        let start = self.window(inner.width as usize);
        let mut cell = 0;
        let mut position = self.value.len();
        for (relative, ch) in self.value[start..].char_indices() {
            let width = self.display_char(ch).width();
            if cell + width > target {
                position = start + relative;
                break;
            }
            cell += width;
        }
        self.move_to(position, false);
        true
    }
    pub fn render(
        &self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        focused: bool,
    ) -> Option<Position> {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return None;
        }
        let style = theme.style(if focused {
            "code-search-field-active"
        } else {
            "code-search-field"
        });
        buffer.set_style(area, style);
        let inner = if area.width >= 3 {
            buffer[(area.x, area.y)].set_symbol("[");
            buffer[(area.right() - 1, area.y)].set_symbol("]");
            Rect::new(area.x + 1, area.y, area.width - 2, 1)
        } else {
            Rect::new(area.x, area.y, area.width, 1)
        };
        let start = self.window(inner.width as usize);
        let display: String = self.value[start..]
            .chars()
            .map(|ch| self.display_char(ch))
            .collect();
        buffer.set_stringn(inner.x, inner.y, display, inner.width as usize, style);
        if focused {
            let caret = self.value[start..self.cursor]
                .chars()
                .map(|ch| {
                    if self.masked {
                        1
                    } else if ch.is_control() {
                        self.display_char(ch).len()
                    } else {
                        ch.width().unwrap_or(0)
                    }
                })
                .sum::<usize>();
            let position = Position::new(inner.x + (caret as u16).min(inner.width - 1), inner.y);
            buffer[position].set_style(
                theme
                    .style("code-search-cursor")
                    .add_modifier(Modifier::REVERSED),
            );
            Some(position)
        } else {
            None
        }
    }
    pub fn pointer(&mut self, event: &InputEvent, area: Rect) -> bool {
        matches!(event, InputEvent::Pointer(pointer) if pointer.action == PointerAction::Down(MouseButton::Left) && self.hit_test(area, pointer.x, pointer.y))
    }
}
