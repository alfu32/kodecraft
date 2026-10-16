//! Read-only diagnostic snapshot with wrapped navigation and explicit copy.
use super::{
    controls::{button, draw_text},
    text_panel::TextPanel,
};
use crate::{
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key},
    layout::centered,
    theme::Theme,
};
use kode_core::Result;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Clear, Widget},
};
#[derive(PartialEq, Eq)]
pub enum Action {
    Copy,
    Dismiss,
}
pub struct Diagnostic {
    pub title: String,
    pub text: String,
    pub previous: Option<FocusId>,
    pub panel: TextPanel,
    copy: Rect,
    close: Rect,
}
impl Diagnostic {
    pub fn new(title: String, text: String, previous: Option<FocusId>) -> Result<Self> {
        let text = format!("{title}\n\n{text}");
        let panel = TextPanel::new(text.lines().map(str::to_owned).collect())?;
        Ok(Self {
            title,
            text,
            previous,
            panel,
            copy: Rect::default(),
            close: Rect::default(),
        })
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Option<Action> {
        if let InputEvent::Key(input) = event {
            if input.key == Key::Escape || input.key == Key::Enter {
                return Some(Action::Dismiss);
            }
            if input.modifiers.ctrl && matches!(input.key, Key::Character('c' | 'C')) {
                return Some(Action::Copy);
            }
        }
        if button(event, self.copy, false) {
            return Some(Action::Copy);
        }
        if button(event, self.close, false) {
            return Some(Action::Dismiss);
        }
        // Dialog owns keyboard navigation even after a click on its frame.
        focus.focused = Some(self.panel.id);
        self.panel.dispatch(event, focus);
        None
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        let modal = centered(area, 100, area.height.saturating_sub(2).max(1));
        self.copy = Rect::default();
        self.close = Rect::default();
        if modal.is_empty() {
            return;
        }
        Clear.render(modal, buffer);
        let inner = if modal.width >= 4 && modal.height >= 4 {
            Block::default()
                .title(crate::editor::display::display_text(&self.title))
                .borders(Borders::ALL)
                .style(theme.style("project-search-dialog"))
                .render(modal, buffer);
            modal.inner(ratatui::layout::Margin::new(1, 1))
        } else {
            modal
        };
        let footer = u16::from(inner.height > 1);
        self.panel.render(
            Rect::new(inner.x, inner.y, inner.width, inner.height - footer),
            buffer,
            theme,
        );
        if footer > 0 {
            let y = inner.bottom() - 1;
            let copy = inner.width.min(13);
            let close = inner.width.saturating_sub(copy).min(11);
            self.copy = Rect::new(inner.x, y, copy, 1);
            self.close = Rect::new(inner.x + copy, y, close, 1);
            draw_text(
                self.copy,
                buffer,
                "[Copy Ctrl+C]",
                theme.style("lsp-button"),
            );
            draw_text(self.close, buffer, "[Close Esc]", theme.style("lsp-button"));
            let rest = inner.width.saturating_sub(copy + close);
            draw_text(
                Rect::new(inner.x + copy + close, y, rest, 1),
                buffer,
                " Up/Down PgUp/PgDn Home/End",
                theme.style("content"),
            );
        }
    }
}
/// One-line summary only; the full text remains in the diagnostic snapshot.
pub fn summary(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let line = if line.starts_with("SQL ") {
        line.split_once(": Error: ")
            .map_or(line, |(_, message)| message)
    } else {
        line
    };
    line.chars().take(160).collect()
}
