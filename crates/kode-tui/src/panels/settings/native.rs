//! Native cache controls reuse the real project/index worker commands.
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    panels::project::ProjectWork,
    theme::Theme,
    widgets::text_panel::TextPanel,
};
use crossterm::event::MouseButton;
use kode_core::Result;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
pub struct NativeSettings {
    pub text: TextPanel,
    work: Vec<ProjectWork>,
}
impl NativeSettings {
    pub fn new() -> Result<Self> {
        Ok(Self {
            text: TextPanel::new(vec![])?,
            work: vec![],
        })
    }
    pub fn id(&self) -> FocusId {
        self.text.id
    }
    pub fn take_work(&mut self) -> Vec<ProjectWork> {
        std::mem::take(&mut self.work)
    }
    pub fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        status: &str,
        cache: &std::path::Path,
    ) {
        self.text.set_lines(vec![
            "[Rescan (r)]".into(),"[Rebuild index (b)]".into(),"[Clear index (c)]".into(),
            "Native indexing — no Java required".into(),
            if status.is_empty() {"Index unavailable: reopen the workspace to start its native worker".into()} else {status.into()},
            format!("Cache: {}",cache.display()),
            "Clear removes only rebuildable Rust index data. JDBC connections, credentials, SQL buffers and legacy H2 files stay separate.".into(),
            "Clear keeps automatic disk rescanning paused; Rebuild resumes it. Unsaved documents remain available to native intelligence.".into(),
            "Up/Down/Page/Home/End or wheel scroll this panel.".into(),
        ]);
        self.text.render(area, buffer, theme);
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        let action = match event {
            InputEvent::Key(input)
                if focus.focused == Some(self.id())
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Index,
                    *input,
                ) else {
                    return self.text.dispatch(event, focus);
                };
                match input.key {
                    Key::Character('r' | 'R') => Some(ProjectWork::Rescan),
                    Key::Character('b' | 'B') => Some(ProjectWork::RebuildIndex),
                    Key::Character('c' | 'C') => Some(ProjectWork::ClearIndex),
                    _ => None,
                }
            }
            InputEvent::Pointer(pointer)
                if pointer.action == PointerAction::Down(MouseButton::Left) =>
            {
                match self.text.line_at(Position::new(pointer.x, pointer.y)) {
                    Some(0) => Some(ProjectWork::Rescan),
                    Some(1) => Some(ProjectWork::RebuildIndex),
                    Some(2) => Some(ProjectWork::ClearIndex),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(action) = action {
            focus.focused = Some(self.id());
            self.work.push(action);
            true
        } else {
            self.text.dispatch(event, focus)
        }
    }
}
