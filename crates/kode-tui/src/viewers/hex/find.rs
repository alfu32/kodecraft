//! The hex find bar owns its field/focus; its parent queues actual searches.
use super::search::{Direction, SearchMode};
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::{
        controls::{button, draw_text},
        field::{FieldAction, TextField},
    },
};
use crossterm::event::MouseButton;
use kode_core::{RequestId, Result};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};

pub enum Action {
    None,
    Changed,
    Search(Direction),
    Stop,
    Close,
    Field(FieldAction),
}
pub struct FindBar {
    pub id: FocusId,
    pub field: TextField,
    pub mode: SearchMode,
    pub visible: bool,
    pub status: String,
    mode_hit: Rect,
    field_hit: Rect,
    buttons: [Rect; 4],
}
impl FindBar {
    pub fn new() -> Result<Self> {
        let mut field = TextField::new(String::new());
        field.max_chars = 8192;
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            field,
            mode: SearchMode::Text,
            visible: false,
            status: "UTF-8 text; Tab: hex".into(),
            mode_hit: Rect::default(),
            field_hit: Rect::default(),
            buttons: [Rect::default(); 4],
        })
    }
    fn toggle(&mut self) -> Action {
        self.mode = if self.mode == SearchMode::Text {
            SearchMode::Hex
        } else {
            SearchMode::Text
        };
        self.status = if self.mode == SearchMode::Text {
            "UTF-8 text"
        } else {
            "Hex bytes: 00 FF"
        }
        .into();
        Action::Changed
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Option<Action> {
        if !self.visible {
            return None;
        }
        if let InputEvent::Key(input) = event
            && focus.focused == Some(self.id)
        {
            return Some(match input.key {
                Key::Escape => Action::Close,
                Key::Enter => Action::Search(if input.modifiers.shift {
                    Direction::Previous
                } else {
                    Direction::Next
                }),
                Key::Tab => self.toggle(),
                _ => self.field.handle(event).map_or(Action::None, |action| {
                    if action == FieldAction::Changed {
                        Action::Changed
                    } else {
                        Action::Field(action)
                    }
                }),
            });
        }
        if let InputEvent::Paste(_) = event
            && focus.focused == Some(self.id)
        {
            return Some(self.field.handle(event).map_or(Action::None, |action| {
                if action == FieldAction::Changed {
                    Action::Changed
                } else {
                    Action::Field(action)
                }
            }));
        }
        if button(event, self.mode_hit, false) {
            focus.focused = Some(self.id);
            return Some(self.toggle());
        }
        for (index, hit) in self.buttons.iter().enumerate() {
            if button(event, *hit, false) {
                focus.focused = Some(self.id);
                return Some(match index {
                    0 => Action::Search(Direction::Previous),
                    1 => Action::Search(Direction::Next),
                    2 => Action::Stop,
                    _ => Action::Close,
                });
            }
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(MouseButton::Left)
            && self.field_hit.contains(Position::new(pointer.x, pointer.y))
        {
            focus.focused = Some(self.id);
            self.field.hit_test(self.field_hit, pointer.x, pointer.y);
            return Some(Action::None);
        }
        None
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        focus: &FocusState,
    ) -> Option<Position> {
        self.mode_hit = Rect::default();
        self.field_hit = Rect::default();
        self.buttons = [Rect::default(); 4];
        if area.is_empty() {
            return None;
        }
        let width = if area.width >= 8 { 6 } else { 0 };
        self.mode_hit = Rect::new(area.x, area.y, width, 1);
        draw_text(
            self.mode_hit,
            buffer,
            if self.mode == SearchMode::Text {
                "[Text]"
            } else {
                "[Hex]"
            },
            theme.style("lsp-button"),
        );
        self.field_hit = Rect::new(area.x + width, area.y, area.width - width, 1);
        let cursor = self.field.render(
            self.field_hit,
            buffer,
            theme,
            focus.focused == Some(self.id),
        );
        if area.height > 1 {
            let mut x = area.x;
            for (index, label) in ["[<]", "[>]", "[Stop]", "[X]"].iter().enumerate() {
                let width = (label.len() as u16).min(area.right().saturating_sub(x));
                self.buttons[index] = Rect::new(x, area.y + 1, width, 1);
                draw_text(
                    self.buttons[index],
                    buffer,
                    label,
                    theme.style("lsp-button"),
                );
                x += width;
            }
        }
        if area.height > 2 {
            draw_text(
                Rect::new(area.x, area.y + 2, area.width, 1),
                buffer,
                &crate::editor::display::display_text(&self.status),
                theme.style("content"),
            );
        }
        cursor
    }
}
