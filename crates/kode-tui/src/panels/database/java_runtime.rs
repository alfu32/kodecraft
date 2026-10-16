//! Captured optional-Java offer and installation progress. No I/O in this reducer.
use crate::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key},
    layout::centered,
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use kode_core::JobTag;
use kode_database::java_runtime::Progress;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};
use tokio::sync::watch;

#[derive(Clone, Copy)]
pub enum Action {
    Download,
    Cancel,
    Hide,
    Details,
}
pub struct JavaRuntimeDialog {
    pub tag: Option<JobTag>,
    pub message: String,
    pub hidden: bool,
    pub done: bool,
    pub success: bool,
    progress: Option<watch::Receiver<Progress>>,
    cancelling: bool,
    selected: usize,
    hits: [Rect; 3],
    action: Option<Action>,
}
impl Default for JavaRuntimeDialog {
    fn default() -> Self {
        Self { tag: None, message: "JDBC needs Java 21 or newer. Download the optional Java runtime? It is installed separately from the application and JDBC drivers. You can also configure a local Java installation and retry Test or Connect.".into(), hidden: false, done: false, success: false, progress: None, cancelling: false, selected: 0, hits: [Rect::default(); 3], action: None }
    }
}
impl JavaRuntimeDialog {
    pub fn start(&mut self, tag: JobTag, progress: watch::Receiver<Progress>) {
        self.tag = Some(tag);
        self.progress = Some(progress);
        self.done = false;
        self.success = false;
        self.cancelling = false;
        self.selected = 0;
        self.message = "Checking whether Java is already available…".into();
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<String, String>) {
        if self.tag != Some(tag) || self.done {
            return;
        }
        self.done = true;
        self.success = result.is_ok();
        self.selected = self.selected.min(self.actions().len() - 1);
        self.message = match result {
            Ok(message) | Err(message) => message,
        };
    }
    pub fn poll(&mut self) -> bool {
        if let Some(progress) = &mut self.progress
            && progress.has_changed().unwrap_or(false)
        {
            let value = progress.borrow_and_update().clone();
            if !self.done && !self.cancelling {
                self.message = if let Some(total) = value.total.filter(|total| *total > 0) {
                    format!(
                        "{}: {:.1} / {:.1} MiB",
                        value.phase,
                        value.completed as f64 / 1048576.0,
                        total as f64 / 1048576.0
                    )
                } else {
                    value.phase.into()
                };
            }
            return true;
        }
        false
    }
    pub fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }
    fn actions(&self) -> &'static [(&'static str, Action)] {
        if self.tag.is_some() && !self.done {
            &[
                ("Cancel", Action::Cancel),
                ("Hide", Action::Hide),
                ("F4 Details", Action::Details),
            ]
        } else if self.success {
            &[("Close", Action::Hide), ("F4 Details", Action::Details)]
        } else if self.done {
            &[
                ("Retry", Action::Download),
                ("Close", Action::Hide),
                ("F4 Details", Action::Details),
            ]
        } else {
            &[
                ("Download", Action::Download),
                ("Close", Action::Hide),
                ("F4 Details", Action::Details),
            ]
        }
    }

    fn activate(&mut self, action: Action) {
        if matches!(action, Action::Cancel) {
            if self.cancelling {
                return;
            }
            self.cancelling = true;
            self.message = "Cancelling; waiting for download and Java processes to stop…".into();
        }
        self.action = Some(action);
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.hits = [Rect::default(); 3];
        let area = centered(area.intersection(buffer.area), 76, 12);
        if area.is_empty() {
            return;
        }
        Clear.render(area, buffer);
        Block::default()
            .borders(Borders::ALL)
            .title("Optional Java runtime")
            .style(theme.style("project-search-dialog"))
            .render(area, buffer);
        let inner = area.inner(ratatui::layout::Margin::new(
            u16::from(area.width > 2),
            u16::from(area.height > 2),
        ));
        if inner.is_empty() {
            return;
        }
        Paragraph::new(crate::editor::display::display_text(&self.message))
            .wrap(Wrap { trim: false })
            .style(theme.style("content"))
            .render(
                Rect::new(
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height.saturating_sub(2),
                ),
                buffer,
            );
        let width = inner.width / self.actions().len() as u16;
        for (index, (label, _)) in self.actions().iter().enumerate() {
            let hit = Rect::new(inner.x + index as u16 * width, inner.bottom() - 1, width, 1);
            self.hits[index] = hit;
            draw_text(
                hit,
                buffer,
                &format!("[{label}]"),
                theme.style(if self.selected == index {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
        }
    }
}
impl EventTarget for JavaRuntimeDialog {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        if let InputEvent::Key(input) = event {
            match input.key {
                Key::Escape => {
                    self.activate(Action::Hide);
                    return true;
                }
                Key::Tab => {
                    self.selected = (self.selected
                        + if input.modifiers.shift {
                            self.actions().len() - 1
                        } else {
                            1
                        })
                        % self.actions().len();
                    return true;
                }
                _ => {}
            }
        }
        for (index, (_, action)) in self.actions().iter().copied().enumerate() {
            if button(event, self.hits[index], self.selected == index) {
                self.activate(action);
                break;
            }
        }
        true
    }
}
