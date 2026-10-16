//! Selectable last-run outcomes. Execution and response ownership stay in RunState.
use super::run::RunState;
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
pub enum Intent {
    Open(String),
    Stop,
    Rerun,
    Dismiss,
}
pub struct Runner {
    pub id: FocusId,
    pub visible: bool,
    pub focus_after_open: bool,
    pub selected: usize,
    pub scroll: usize,
    pub area: Rect,
    pub list: Rect,
    pub button: Rect,
    pub close: Rect,
    pub message: String,
    follow: bool,
    intents: Vec<Intent>,
}
impl Runner {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            visible: false,
            focus_after_open: false,
            selected: 0,
            scroll: 0,
            area: Rect::default(),
            list: Rect::default(),
            button: Rect::default(),
            close: Rect::default(),
            message: String::new(),
            follow: true,
            intents: Vec::new(),
        })
    }
    pub fn take_intents(&mut self) -> Vec<Intent> {
        std::mem::take(&mut self.intents)
    }
    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.follow = true;
        self.message.clear();
    }
    fn open(&mut self, run: &RunState) {
        if let Some(outcome) = run.outcomes.get(self.selected) {
            self.intents.push(Intent::Open(outcome.item.id.clone()));
        }
    }
    pub fn dispatch(&mut self, event: &InputEvent, run: &RunState, focus: &mut FocusState) -> bool {
        if !self.visible {
            return false;
        }
        if let InputEvent::Pointer(pointer) = event {
            let position = Position::new(pointer.x, pointer.y);
            if !self.area.contains(position) {
                return false;
            }
            match pointer.action {
                PointerAction::Down(MouseButton::Left) => {
                    focus.focused = Some(self.id);
                    if self.close.contains(position) {
                        self.intents.push(Intent::Dismiss);
                    } else if self.button.contains(position) {
                        self.intents
                            .push(if run.pending.is_some() || run.running.is_some() {
                                Intent::Stop
                            } else {
                                Intent::Rerun
                            });
                    } else if self.list.contains(position) {
                        let index = self.scroll + (pointer.y - self.list.y) as usize;
                        if index < run.outcomes.len() {
                            self.selected = index;
                            self.follow = true;
                            self.open(run);
                        }
                    }
                    return true;
                }
                PointerAction::Scroll { vertical, .. } => {
                    self.scroll = self.scroll.saturating_add_signed(vertical as isize * 3);
                    self.follow = false;
                    return true;
                }
                _ => return false,
            }
        }
        if focus.focused != Some(self.id) {
            return false;
        }
        if let InputEvent::Key(input) = event
            && !input.modifiers.ctrl
            && !input.modifiers.alt
            && !input.modifiers.meta
        {
            let count = run.outcomes.len();
            let page = self.list.height.max(1) as usize;
            match input.key {
                Key::Up => self.selected = self.selected.saturating_sub(1),
                Key::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
                Key::PageUp => self.selected = self.selected.saturating_sub(page),
                Key::PageDown => {
                    self.selected = self
                        .selected
                        .saturating_add(page)
                        .min(count.saturating_sub(1))
                }
                Key::Home => self.selected = 0,
                Key::End => self.selected = count.saturating_sub(1),
                Key::Enter => self.open(run),
                Key::Escape => self.intents.push(Intent::Dismiss),
                Key::Character('r' | 'R') => {
                    self.intents
                        .push(if run.pending.is_some() || run.running.is_some() {
                            Intent::Stop
                        } else {
                            Intent::Rerun
                        })
                }
                Key::Character('s' | 'S') => self.intents.push(Intent::Stop),
                _ => return false,
            }
            self.follow = true;
            return true;
        }
        false
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        run: &RunState,
        focused: bool,
    ) {
        self.area = area.intersection(buffer.area);
        self.list = Rect::default();
        self.button = Rect::default();
        self.close = Rect::default();
        if self.area.is_empty() {
            return;
        }
        let area = self.area;
        Clear.render(area, buffer);
        let busy = run.pending.is_some() || run.running.is_some();
        let heading = if run.pending.is_some() {
            "Collection run: preparing".into()
        } else if let Some(running) = &run.running {
            format!(
                "Collection run: {} {}/{} | {} bytes",
                if running.cancel.is_cancelled() {
                    "stopping"
                } else {
                    "running"
                },
                running.completed,
                running.total,
                running.received_bytes
            )
        } else if let Some(summary) = &run.summary {
            format!(
                "Collection run: {} | {} completed",
                if summary.cancelled {
                    "stopped"
                } else {
                    "finished"
                },
                summary.outcomes.len()
            )
        } else {
            "Collection run: no history".into()
        };
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            &heading,
            theme.style("content"),
        );
        if area.height < 2 {
            return;
        }
        let label = if busy { "Stop" } else { "Run again" };
        self.button = Rect::new(area.x, area.y + 1, (label.len() as u16).min(area.width), 1);
        self.close = Rect::new(
            area.right().saturating_sub(7.min(area.width)),
            area.y + 1,
            7.min(area.width),
            1,
        );
        draw_text(self.button, buffer, label, theme.style("lsp-button"));
        draw_text(self.close, buffer, "Close", theme.style("lsp-button"));
        let mut top = area.y + 2;
        if top < area.bottom() {
            let detail = if !self.message.is_empty() {
                self.message.clone()
            } else if busy {
                run.active_item
                    .as_ref()
                    .map(|item| format!("Current: {}", item.name))
                    .unwrap_or_else(|| "Preparing selected requests…".into())
            } else {
                "Enter/click opens result | F7 focus | r rerun | Esc close".into()
            };
            draw_text(
                Rect::new(area.x, top, area.width, 1),
                buffer,
                &display_text(&detail),
                theme.style("content"),
            );
            top += 1;
        }
        self.list = Rect::new(area.x, top, area.width, area.bottom().saturating_sub(top));
        self.selected = self.selected.min(run.outcomes.len().saturating_sub(1));
        let height = self.list.height as usize;
        if self.follow {
            if self.selected < self.scroll {
                self.scroll = self.selected;
            } else if self.selected >= self.scroll + height {
                self.scroll = (self.selected + 1).saturating_sub(height);
            }
            self.follow = false;
        }
        self.scroll = self.scroll.min(run.outcomes.len().saturating_sub(height));
        for (row, outcome) in run
            .outcomes
            .iter()
            .skip(self.scroll)
            .take(height)
            .enumerate()
        {
            let status = if let Some(error) = &outcome.validation_error {
                format!("INVALID {error}")
            } else if let Some(error) = outcome.error {
                format!("ERR {}", error.message())
            } else {
                format!(
                    "HTTP {}",
                    outcome
                        .status
                        .map_or_else(|| "?".into(), |status| status.to_string())
                )
            };
            let line = format!(
                "{} | {} | {} ms | {} bytes",
                outcome.item.name,
                status,
                outcome.duration.as_millis(),
                outcome.received_bytes
            );
            draw_text(
                Rect::new(self.list.x, self.list.y + row as u16, self.list.width, 1),
                buffer,
                &display_text(&line),
                theme.style(if focused && self.selected == self.scroll + row {
                    "file-entry:selected"
                } else {
                    "content"
                }),
            );
        }
    }
}
