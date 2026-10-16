pub mod lsp;
pub mod native;

use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::Result;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
pub struct Settings {
    pub index_selected: bool,
    pub native: native::NativeSettings,
    hits: Vec<(Rect, bool)>,
}
impl Settings {
    pub fn new() -> Result<Self> {
        Ok(Self {
            index_selected: false,
            native: native::NativeSettings::new()?,
            hits: vec![],
        })
    }
    pub fn focus_id(&self, lsp: FocusId) -> FocusId {
        if self.index_selected {
            self.native.id()
        } else {
            lsp
        }
    }
    pub fn render_header(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Rect {
        self.hits.clear();
        let mut x = area.x;
        if area.height > 0 {
            for (label, index) in [("[LSP]", false), ("[Index]", true)] {
                let hit = Rect::new(
                    x,
                    area.y,
                    area.right().saturating_sub(x).min(label.len() as u16),
                    1,
                );
                draw_text(
                    hit,
                    buffer,
                    label,
                    theme.style(if self.index_selected == index {
                        "tab-button::selection"
                    } else {
                        "content"
                    }),
                );
                self.hits.push((hit, index));
                x = hit.right().saturating_add(1).min(area.right());
            }
        }
        Rect::new(
            area.x,
            area.y + area.height.min(1),
            area.width,
            area.height.saturating_sub(1),
        )
    }
    pub fn dispatch_navigation(
        &mut self,
        event: &InputEvent,
        focus: &mut FocusState,
        lsp: FocusId,
    ) -> bool {
        match event {
            InputEvent::Key(input)
                if focus.focused == Some(self.focus_id(lsp))
                    && input.key == Key::Tab
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                self.index_selected = !self.index_selected
            }
            InputEvent::Pointer(pointer)
                if pointer.action == PointerAction::Down(MouseButton::Left) =>
            {
                let Some((_, index)) = self
                    .hits
                    .iter()
                    .find(|(area, _)| area.contains(Position::new(pointer.x, pointer.y)))
                else {
                    return false;
                };
                self.index_selected = *index;
            }
            _ => return false,
        }
        focus.focused = Some(self.focus_id(lsp));
        true
    }
}
