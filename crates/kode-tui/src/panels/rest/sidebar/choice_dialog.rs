//! Context actions and destination folders share bounded, scrolling navigation.
use crate::{
    editor::display::display_text,
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use crossterm::event::MouseButton;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
pub struct ChoiceDialog<T: Clone> {
    pub title: String,
    pub options: Vec<(String, T)>,
    pub selected: usize,
    pub focus: usize,
    pub finished: bool,
    pub result: Option<T>,
    pub rows: Vec<(Rect, usize)>,
    pub ok: Rect,
    pub cancel: Rect,
    pub bounds: Rect,
    label: String,
    immediate: bool,
    scroll: usize,
    viewport: Rect,
}
impl<T: Clone> ChoiceDialog<T> {
    pub fn new(title: String, options: Vec<(String, T)>, label: &str, immediate: bool) -> Self {
        Self {
            title,
            options,
            selected: 0,
            focus: 0,
            finished: false,
            result: None,
            rows: vec![],
            ok: Rect::default(),
            cancel: Rect::default(),
            bounds: Rect::default(),
            label: label.into(),
            immediate,
            scroll: 0,
            viewport: Rect::default(),
        }
    }
    fn finish(&mut self, apply: bool) {
        if apply {
            self.result = self
                .options
                .get(self.selected)
                .map(|(_, value)| value.clone());
        }
        self.finished = true;
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.viewport = area.intersection(buffer.area);
        self.bounds = centered(
            self.viewport,
            70,
            (self.options.len() + 4).min(u16::MAX as usize) as u16,
        );
        self.rows.clear();
        self.ok = Rect::default();
        self.cancel = Rect::default();
        if self.bounds.is_empty() {
            return;
        }
        Clear.render(self.bounds, buffer);
        let framed = self.bounds.width >= 4 && self.bounds.height >= 4;
        if framed {
            Block::default()
                .title(display_text(&self.title))
                .borders(Borders::ALL)
                .style(theme.style("project-search-dialog"))
                .render(self.bounds, buffer);
        }
        let inner = if framed {
            Rect::new(
                self.bounds.x + 1,
                self.bounds.y + 1,
                self.bounds.width - 2,
                self.bounds.height - 2,
            )
        } else {
            self.bounds
        };
        let visible = if inner.height == 1 {
            usize::from(self.focus == 0)
        } else {
            inner.height as usize - 1
        };
        self.selected = self.selected.min(self.options.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.options.len().saturating_sub(visible));
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if visible > 0 && self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        for (index, (label, _)) in self
            .options
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(visible)
        {
            let rect = Rect::new(
                inner.x,
                inner.y + (index - self.scroll) as u16,
                inner.width,
                1,
            );
            draw_text(
                rect,
                buffer,
                &display_text(label),
                theme.style(if self.focus == 0 && self.selected == index {
                    "file-entry:selected"
                } else {
                    "project-search-dialog"
                }),
            );
            self.rows.push((rect, index));
        }
        let y = inner.bottom() - 1;
        if inner.height > 1 || self.focus > 0 {
            if inner.width == 1 {
                if self.focus == 2 {
                    self.cancel = Rect::new(inner.x, y, 1, 1);
                } else {
                    self.ok = Rect::new(inner.x, y, 1, 1);
                }
            } else {
                let half = inner.width / 2;
                self.ok = Rect::new(inner.x, y, half, 1);
                self.cancel = Rect::new(inner.x + half, y, inner.width - half, 1);
            }
        }
        draw_text(
            self.ok,
            buffer,
            &self.label,
            theme.style(if self.focus == 1 {
                "lsp-button"
            } else {
                "project-search-dialog"
            }),
        );
        draw_text(
            self.cancel,
            buffer,
            "Cancel",
            theme.style(if self.focus == 2 {
                "lsp-button"
            } else {
                "project-search-dialog"
            }),
        );
    }
    pub fn dispatch(&mut self, event: &InputEvent) -> bool {
        if self.finished {
            return false;
        }
        if let InputEvent::Key(input) = event {
            match input.key {
                Key::Escape => self.finish(false),
                Key::Tab => {
                    self.focus = (self.focus + if input.modifiers.shift { 2 } else { 1 }) % 3
                }
                Key::Enter => self.finish(self.focus != 2),
                Key::Left if self.focus == 2 => self.focus = 1,
                Key::Right if self.focus == 1 => self.focus = 2,
                Key::Up if self.focus == 0 => self.selected = self.selected.saturating_sub(1),
                Key::Down if self.focus == 0 => {
                    self.selected = (self.selected + 1).min(self.options.len().saturating_sub(1))
                }
                Key::PageUp if self.focus == 0 => {
                    self.selected = self.selected.saturating_sub(self.rows.len().max(1))
                }
                Key::PageDown if self.focus == 0 => {
                    self.selected = self
                        .selected
                        .saturating_add(self.rows.len().max(1))
                        .min(self.options.len().saturating_sub(1))
                }
                Key::Home if self.focus == 0 => self.selected = 0,
                Key::End if self.focus == 0 => self.selected = self.options.len().saturating_sub(1),
                Key::Up => self.focus = (self.focus + 2) % 3,
                Key::Down => self.focus = (self.focus + 1) % 3,
                _ => {}
            }
            return true;
        }
        if button(event, self.ok, self.focus == 1) {
            self.finish(true);
            return true;
        }
        if button(event, self.cancel, self.focus == 2) {
            self.finish(false);
            return true;
        }
        if let InputEvent::Pointer(pointer) = event {
            let position = Position::new(pointer.x, pointer.y);
            match pointer.action {
                PointerAction::Down(MouseButton::Left) => {
                    if let Some((_, index)) =
                        self.rows.iter().find(|(rect, _)| rect.contains(position))
                    {
                        self.selected = *index;
                        self.focus = 0;
                        if self.immediate {
                            self.finish(true);
                        }
                    } else if !self.bounds.contains(position) {
                        self.finish(false);
                    }
                }
                PointerAction::Scroll { vertical, .. } if self.bounds.contains(position) => {
                    self.selected = self
                        .selected
                        .saturating_add_signed(vertical as isize * 3)
                        .min(self.options.len().saturating_sub(1));
                }
                _ => {}
            }
        }
        true
    }
}
