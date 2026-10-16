//! Shared key/value and variable dialog, with complete field/button ownership.
mod paths;
use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
    widgets::{
        controls::{button, draw_text},
        field::{FieldAction, TextField},
    },
};
use crossterm::event::MouseButton;
use kode_core::{RequestId, Result};
pub use kode_rest::model::editing::EntryValue;
pub use paths::PathProof;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
#[derive(Clone)]
pub struct PasteProof {
    nonce: RequestId,
    index: usize,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct Geometry {
    pub modal: Rect,
    pub fields: Vec<Rect>,
    pub ok: Rect,
    pub cancel: Rect,
    pub browse: Rect,
}
pub struct EntryDialog {
    pub title: String,
    pub fields: Vec<TextField>,
    pub focus: usize,
    pub finished: bool,
    pub result: Option<EntryValue>,
    pub file_source: bool,
    browse_requested: bool,
    labels: Vec<String>,
    variable: bool,
    nonce: RequestId,
    viewport: Rect,
    actions: Vec<FieldAction>,
}
impl EntryDialog {
    pub fn new(
        title: String,
        value: EntryValue,
        variable: bool,
        third_label: &str,
    ) -> Result<Self> {
        let (labels, values) = if variable {
            (
                vec!["Key", "Value", "Type", "Description", "Enabled true/false"],
                vec![
                    value.key,
                    value.value,
                    value.kind,
                    value.description,
                    value.enabled.to_string(),
                ],
            )
        } else {
            (
                vec!["Key", "Value", third_label],
                vec![value.key, value.value, value.description],
            )
        };
        Ok(Self {
            title,
            fields: values.into_iter().map(TextField::new).collect(),
            focus: 0,
            finished: false,
            result: None,
            file_source: false,
            browse_requested: false,
            labels: labels.into_iter().map(str::to_string).collect(),
            variable,
            nonce: RequestId::fresh()?,
            viewport: Rect::default(),
            actions: vec![],
        })
    }
    pub fn proof(&self) -> Option<PasteProof> {
        let field = self.fields.get(self.focus)?;
        Some(PasteProof {
            nonce: self.nonce,
            index: self.focus,
            value: field.value.clone(),
            cursor: field.cursor,
            anchor: field.anchor,
        })
    }
    pub fn paste(&mut self, proof: PasteProof, text: String) {
        if self.finished || proof.nonce != self.nonce || proof.index != self.focus {
            return;
        }
        if let Some(field) = self.fields.get_mut(proof.index)
            && field.value == proof.value
            && field.cursor == proof.cursor
            && field.anchor == proof.anchor
            && let Some(action) = field.handle(&InputEvent::Paste(text))
        {
            self.actions.push(action);
        }
    }
    pub fn take_actions(&mut self) -> Vec<FieldAction> {
        std::mem::take(&mut self.actions)
    }
    pub fn layout(&self, area: Rect) -> Geometry {
        let modal = centered(area, 76, if self.variable { 12 } else { 10 });
        let mut geometry = Geometry {
            modal,
            fields: vec![Rect::default(); self.fields.len()],
            ok: Rect::default(),
            cancel: Rect::default(),
            browse: Rect::default(),
        };
        if modal.is_empty() {
            return geometry;
        }
        let inner = if modal.width >= 4 && modal.height >= 4 {
            Rect::new(modal.x + 1, modal.y + 1, modal.width - 2, modal.height - 2)
        } else {
            modal
        };
        let count = self.fields.len();
        if self.file_enabled() && self.focus == count + 2 {
            geometry.browse = Rect::new(inner.x, inner.y, inner.width, 1);
            return geometry;
        }
        if inner.height == 1 {
            if self.focus < count {
                geometry.fields[self.focus] = inner;
            } else if self.focus == count {
                geometry.ok = inner;
            } else {
                geometry.cancel = inner;
            }
            return geometry;
        }
        let rows = (inner.height as usize - 1).min(count);
        let first = self
            .focus
            .min(count.saturating_sub(1))
            .saturating_add(1)
            .saturating_sub(rows);
        let label_width = if inner.width >= 24 {
            self.labels
                .iter()
                .map(String::len)
                .max()
                .unwrap_or(0)
                .min(inner.width as usize / 2) as u16
                + 1
        } else {
            0
        };
        for index in first..(first + rows).min(count) {
            geometry.fields[index] = Rect::new(
                inner.x + label_width,
                inner.y + (index - first) as u16,
                inner.width.saturating_sub(label_width),
                1,
            );
        }
        if self.file_enabled() {
            let field = &mut geometry.fields[1];
            if field.width >= 20 {
                geometry.browse = Rect::new(field.right() - 16, field.y, 16, 1);
                field.width -= 16;
            }
        }
        let y = inner.bottom() - 1;
        if inner.width == 1 {
            if self.focus == count + 1 {
                geometry.cancel = Rect::new(inner.x, y, 1, 1);
            } else {
                geometry.ok = Rect::new(inner.x, y, 1, 1);
            }
        } else {
            let half = inner.width / 2;
            geometry.ok = Rect::new(inner.x, y, half, 1);
            geometry.cancel = Rect::new(inner.x + half, y, inner.width - half, 1);
        }
        geometry
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Option<Position> {
        self.viewport = area.intersection(buffer.area);
        let geometry = self.layout(self.viewport);
        Clear.render(geometry.modal, buffer);
        if geometry.modal.width >= 4 && geometry.modal.height >= 4 {
            Block::default()
                .title(crate::editor::display::display_text(&self.title))
                .borders(Borders::ALL)
                .style(theme.style("project-search-dialog"))
                .render(geometry.modal, buffer);
        }
        draw_text(
            geometry.browse,
            buffer,
            "[File Alt+P]",
            theme.style("lsp-button"),
        );
        let mut cursor = None;
        for (index, rect) in geometry.fields.iter().enumerate() {
            if rect.is_empty() {
                continue;
            }
            let label_x = geometry.modal.x
                + u16::from(geometry.modal.width >= 4 && geometry.modal.height >= 4);
            if rect.x > label_x {
                draw_text(
                    Rect::new(label_x, rect.y, rect.x - label_x, 1),
                    buffer,
                    &format!("{}:", self.labels[index]),
                    theme.style("project-search-dialog"),
                );
            }
            let caret = self.fields[index].render(*rect, buffer, theme, self.focus == index);
            if caret.is_some() {
                cursor = caret;
            }
        }
        for (rect, label, focused) in [
            (
                geometry.ok,
                if geometry.ok.width < 4 { "OK" } else { " OK " },
                self.focus == self.fields.len(),
            ),
            (
                geometry.cancel,
                if geometry.cancel.width < 8 {
                    "Cancel"
                } else {
                    " Cancel "
                },
                self.focus == self.fields.len() + 1,
            ),
        ] {
            draw_text(
                rect,
                buffer,
                label,
                theme.style(if focused {
                    "lsp-button"
                } else {
                    "project-search-dialog"
                }),
            );
        }
        cursor
    }
    fn finish(&mut self, apply: bool) {
        if apply {
            let fields = &self.fields;
            self.result = Some(EntryValue {
                key: fields[0].value.trim().to_string(),
                value: fields[1].value.clone(),
                description: fields[if self.variable { 3 } else { 2 }].value.clone(),
                kind: if self.variable && !fields[2].value.trim().is_empty() {
                    fields[2].value.clone()
                } else {
                    "string".into()
                },
                enabled: !self.variable || fields[4].value.parse().unwrap_or(true),
            });
        }
        self.finished = true;
    }
    pub fn dispatch(&mut self, event: &InputEvent) -> bool {
        if self.finished {
            return false;
        }
        let count = self.fields.len();
        let geometry = self.layout(self.viewport);
        let targets = count + 2 + usize::from(self.file_enabled());
        if let InputEvent::Key(input) = event {
            if self.file_enabled()
                && ((input.modifiers.alt && matches!(input.key, Key::Character('p' | 'P')))
                    || (input.key == Key::Enter && self.focus == count + 2))
            {
                self.browse_requested = true;
                return true;
            }
            match input.key {
                Key::Escape => {
                    self.finish(false);
                    return true;
                }
                Key::Tab => {
                    self.focus = (self.focus
                        + if input.modifiers.shift {
                            targets - 1
                        } else {
                            1
                        })
                        % targets;
                    return true;
                }
                Key::Down => {
                    self.focus = (self.focus + 1) % targets;
                    return true;
                }
                Key::Up => {
                    self.focus = (self.focus + targets - 1) % targets;
                    return true;
                }
                Key::Left if self.focus == count + 1 => {
                    self.focus = count;
                    return true;
                }
                Key::Right if self.focus == count => {
                    self.focus = count + 1;
                    return true;
                }
                Key::Enter => {
                    self.finish(self.focus != count + 1);
                    return true;
                }
                _ => {}
            }
        }
        if self.file_enabled() && button(event, geometry.browse, false) {
            self.focus = count + 2;
            self.browse_requested = true;
            return true;
        }
        if button(event, geometry.ok, self.focus == count) {
            self.focus = count;
            self.finish(true);
            return true;
        }
        if button(event, geometry.cancel, self.focus == count + 1) {
            self.focus = count + 1;
            self.finish(false);
            return true;
        }
        if let InputEvent::Pointer(pointer) = event {
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                for (index, rect) in geometry.fields.iter().enumerate() {
                    if self.fields[index].hit_test(*rect, pointer.x, pointer.y) {
                        self.focus = index;
                        break;
                    }
                }
            }
        } else if let Some(field) = self.fields.get_mut(self.focus)
            && let Some(action) = field.handle(event)
        {
            self.actions.push(action);
        }
        true
    }
}
