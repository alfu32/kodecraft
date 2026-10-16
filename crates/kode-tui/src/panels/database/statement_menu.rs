use crate::{
    editor::host::ExecutionAction,
    input::terminal::{InputEvent, Key},
    layout::centered,
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Clear, Widget},
};
#[derive(Default)]
pub struct StatementMenu {
    pub selected: usize,
    pub finished: bool,
    pub action: Option<ExecutionAction>,
    hits: [Rect; 3],
}
impl StatementMenu {
    fn choose(&mut self) {
        self.action = match self.selected {
            0 => Some(ExecutionAction::SqlSelectionOrStatement),
            1 => Some(ExecutionAction::SqlScript),
            _ => None,
        };
        self.finished = true;
    }
    pub fn dispatch(&mut self, event: &InputEvent) {
        if let InputEvent::Key(input) = event {
            match input.key {
                Key::Escape => self.finished = true,
                Key::Up => self.selected = self.selected.saturating_sub(1),
                Key::Down => self.selected = (self.selected + 1).min(2),
                Key::Enter => self.choose(),
                _ => {}
            }
        }
        if let Some(index) = self.hits.iter().position(|hit| button(event, *hit, false)) {
            self.selected = index;
            self.choose();
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let modal = centered(area.intersection(buffer.area), 48, 5);
        self.hits = [Rect::default(); 3];
        if modal.is_empty() {
            return;
        }
        Clear.render(modal, buffer);
        Block::default()
            .title("SQL statement")
            .borders(Borders::ALL)
            .style(theme.style("project-search-dialog"))
            .render(modal, buffer);
        let inner = modal.inner(ratatui::layout::Margin::new(
            u16::from(modal.width > 2),
            u16::from(modal.height > 2),
        ));
        let start = self
            .selected
            .saturating_sub(inner.height.saturating_sub(1) as usize);
        for (index, label) in [
            "Execute current / selection",
            "Execute entire script",
            "Cancel menu",
        ]
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height as usize)
        {
            let rect = Rect::new(inner.x, inner.y + (index - start) as u16, inner.width, 1);
            self.hits[index] = rect;
            draw_text(
                rect,
                buffer,
                label,
                theme.style(if index == self.selected {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
        }
    }
}
