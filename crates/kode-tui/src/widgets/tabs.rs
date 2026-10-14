use super::controls::draw_text;
use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    layout::Axis,
    theme::Theme,
};
use crossterm::event::MouseButton;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
pub struct Tabs {
    pub titles: Vec<String>,
    pub selected: usize,
    pub axis: Axis,
    first: usize,
    hits: Vec<(usize, Rect)>,
}
impl Tabs {
    pub fn new(titles: Vec<String>, axis: Axis) -> Self {
        Self {
            titles,
            selected: 0,
            axis,
            first: 0,
            hits: vec![],
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        self.hits.clear();
        let area = area.intersection(buffer.area);
        if self.titles.is_empty() || area.is_empty() {
            return;
        }
        self.selected = self.selected.min(self.titles.len() - 1);
        if self.selected < self.first {
            self.first = self.selected;
        }
        if self.axis == Axis::Vertical {
            if self.selected >= self.first + area.height as usize {
                self.first = self.selected - area.height as usize + 1;
            }
        } else {
            let selected_width: usize = self.titles[self.first..=self.selected]
                .iter()
                .map(|title| title.width() + 2)
                .sum();
            if selected_width > area.width as usize {
                self.first = self.selected;
            }
        }
        let mut used = 0u16;
        for index in self.first..self.titles.len() {
            let hit = match self.axis {
                Axis::Vertical if used < area.height => {
                    Rect::new(area.x, area.y + used, area.width, 1)
                }
                Axis::Horizontal if used < area.width => Rect::new(
                    area.x + used,
                    area.y,
                    ((self.titles[index].width() + 2).min(u16::MAX as usize) as u16)
                        .min(area.width - used),
                    1,
                ),
                _ => break,
            };
            let id = if index == self.selected {
                if focused {
                    "tab-button::selection:focus"
                } else {
                    "tab-button::selection"
                }
            } else {
                "tab-button"
            };
            let label = if hit.width == 1 {
                self.titles[index].clone()
            } else {
                format!(" {} ", self.titles[index])
            };
            draw_text(hit, buffer, &label, theme.style(id));
            self.hits.push((index, hit));
            used += if self.axis == Axis::Vertical {
                1
            } else {
                hit.width
            };
        }
    }
    pub fn handle(&mut self, event: &InputEvent, focused: bool) -> bool {
        if self.titles.is_empty() {
            return false;
        }
        match event {
            InputEvent::Pointer(pointer)
                if pointer.action == PointerAction::Down(MouseButton::Left) =>
            {
                if let Some((index, _)) = self
                    .hits
                    .iter()
                    .find(|(_, area)| area.contains(Position::new(pointer.x, pointer.y)))
                {
                    self.selected = *index;
                    true
                } else {
                    false
                }
            }
            InputEvent::Key(input) if focused => {
                let direction = match (self.axis, input.key) {
                    (Axis::Horizontal, Key::Left) | (Axis::Vertical, Key::Up) => -1,
                    (Axis::Horizontal, Key::Right) | (Axis::Vertical, Key::Down) => 1,
                    _ => return false,
                };
                self.selected = self
                    .selected
                    .saturating_add_signed(direction)
                    .min(self.titles.len() - 1);
                true
            }
            InputEvent::Pointer(pointer)
                if matches!(pointer.action, PointerAction::Scroll { .. })
                    && self
                        .hits
                        .iter()
                        .any(|(_, area)| area.contains(Position::new(pointer.x, pointer.y))) =>
            {
                let PointerAction::Scroll {
                    horizontal,
                    vertical,
                } = pointer.action
                else {
                    unreachable!()
                };
                self.selected = self
                    .selected
                    .saturating_add_signed((vertical + horizontal) as isize)
                    .min(self.titles.len() - 1);
                true
            }
            _ => false,
        }
    }
}
