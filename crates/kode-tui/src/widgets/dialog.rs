use super::{
    controls::{button, draw_text},
    field::{FieldAction, TextField},
};
use crate::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
};
use crossterm::event::MouseButton;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogAction {
    Submitted(String),
    Dismissed,
    Browse,
    Field(FieldAction),
}
#[derive(Debug, Clone, Copy)]
pub struct DialogLayout {
    pub modal: Rect,
    pub field: Rect,
    pub ok: Rect,
    pub cancel: Rect,
    pub browse: Rect,
}

pub struct TextDialog {
    pub title: String,
    pub field: TextField,
    pub focus: usize,
    pub action: Option<DialogAction>,
    pub finished: bool,
    pub path_mode: Option<super::file_picker::Mode>,
    viewport: Rect,
}
impl TextDialog {
    pub fn new(title: String, initial: String) -> Self {
        Self {
            title,
            field: TextField::new(initial),
            focus: 0,
            action: None,
            finished: false,
            path_mode: None,
            viewport: Rect::default(),
        }
    }
    pub fn with_path(mut self, mode: super::file_picker::Mode) -> Self {
        self.path_mode = Some(mode);
        self
    }
    pub fn layout(&self, area: Rect) -> DialogLayout {
        let modal = centered(area, 48, 9);
        if modal.is_empty() {
            return DialogLayout {
                modal,
                field: Rect::default(),
                ok: Rect::default(),
                cancel: Rect::default(),
                browse: Rect::default(),
            };
        }
        if modal.height == 1 {
            return DialogLayout {
                modal,
                field: if self.focus == 0 {
                    modal
                } else {
                    Rect::default()
                },
                ok: if self.focus == 1 {
                    modal
                } else {
                    Rect::default()
                },
                cancel: if self.focus == 2 {
                    modal
                } else {
                    Rect::default()
                },
                browse: if self.path_mode.is_some() && self.focus == 3 {
                    modal
                } else {
                    Rect::default()
                },
            };
        }
        let padding = u16::from(modal.width >= 4);
        let x = modal.x + padding;
        let width = modal.width - padding * 2;
        let field_y = modal.y + if modal.height >= 5 { 2 } else { 0 };
        let button_y = modal.bottom() - if modal.height >= 5 { 2 } else { 1 };
        let (ok, cancel) = if width == 1 {
            (
                if self.focus != 2 {
                    Rect::new(x, button_y, 1, 1)
                } else {
                    Rect::default()
                },
                if self.focus == 2 {
                    Rect::new(x, button_y, 1, 1)
                } else {
                    Rect::default()
                },
            )
        } else {
            let ok_width = 6.min(width / 2);
            let cancel_width = 10.min(width - ok_width);
            (
                Rect::new(x, button_y, ok_width, 1),
                Rect::new(x + ok_width, button_y, cancel_width, 1),
            )
        };
        let field = Rect::new(x, field_y, width, 1);
        let (field, browse) = if self.path_mode.is_some() {
            if width < 18 {
                if self.focus == 3 {
                    (Rect::default(), field)
                } else {
                    (field, Rect::default())
                }
            } else {
                let browse = Rect::new(field.right() - 14, field.y, 14, 1);
                (Rect::new(field.x, field.y, field.width - 14, 1), browse)
            }
        } else {
            (field, Rect::default())
        };
        DialogLayout {
            modal,
            field,
            ok,
            cancel,
            browse,
        }
    }
    fn finish(&mut self, confirm: bool) {
        self.finished = true;
        self.action = Some(if confirm {
            DialogAction::Submitted(self.field.value.clone())
        } else {
            DialogAction::Dismissed
        });
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Option<Position> {
        self.viewport = area.intersection(buffer.area);
        let layout = self.layout(self.viewport);
        if layout.modal.is_empty() {
            return None;
        }
        Clear.render(layout.modal, buffer);
        Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .style(theme.style("project-search-dialog"))
            .border_style(theme.style("project-search-dialog-border"))
            .render(layout.modal, buffer);
        let caret = self
            .field
            .render(layout.field, buffer, theme, self.focus == 0);
        draw_text(
            layout.browse,
            buffer,
            "[Browse Alt+P]",
            theme.style(if self.focus == 3 {
                "lsp-button"
            } else {
                "project-search-dialog"
            }),
        );
        draw_text(
            layout.ok,
            buffer,
            if layout.ok.width < 4 { "OK" } else { " OK " },
            theme.style(if self.focus == 1 {
                "lsp-button"
            } else {
                "project-search-dialog"
            }),
        );
        draw_text(
            layout.cancel,
            buffer,
            if layout.cancel.width < 8 {
                "Cancel"
            } else {
                " Cancel "
            },
            theme.style(if self.focus == 2 {
                "lsp-button"
            } else {
                "project-search-dialog"
            }),
        );
        caret
    }
    pub fn handle(&mut self, event: &InputEvent) {
        if self.finished {
            return;
        }
        let layout = self.layout(self.viewport);
        if let InputEvent::Key(input) = event {
            if self.path_mode.is_some()
                && input.modifiers.alt
                && matches!(input.key, Key::Character('p' | 'P'))
            {
                self.action = Some(DialogAction::Browse);
                return;
            }
            let count = if self.path_mode.is_some() { 4 } else { 3 };
            match input.key {
                Key::Escape => {
                    self.finish(false);
                    return;
                }
                Key::Tab => {
                    self.focus =
                        (self.focus + if input.modifiers.shift { count - 1 } else { 1 }) % count;
                    return;
                }
                Key::Down => {
                    self.focus = (self.focus + 1) % count;
                    return;
                }
                Key::Up => {
                    self.focus = (self.focus + count - 1) % count;
                    return;
                }
                Key::Enter => {
                    if self.path_mode.is_some() && self.focus == 3 {
                        self.action = Some(DialogAction::Browse);
                        return;
                    }
                    self.finish(self.focus != 2);
                    return;
                }
                _ => {}
            }
        }
        if self.path_mode.is_some() && button(event, layout.browse, self.focus == 3) {
            self.focus = 3;
            self.action = Some(DialogAction::Browse);
            return;
        }
        if button(event, layout.ok, self.focus == 1) {
            self.focus = 1;
            self.finish(true);
            return;
        }
        if button(event, layout.cancel, self.focus == 2) {
            self.focus = 2;
            self.finish(false);
            return;
        }
        if let InputEvent::Pointer(pointer) = event {
            if pointer.action == PointerAction::Down(MouseButton::Left)
                && self.field.hit_test(layout.field, pointer.x, pointer.y)
            {
                self.focus = 0;
            }
        } else if self.focus == 0
            && let Some(action) = self.field.handle(event)
        {
            self.action = Some(DialogAction::Field(action));
        }
    }
}
impl EventTarget for TextDialog {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        self.handle(event);
        true
    }
}
