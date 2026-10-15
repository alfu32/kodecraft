//! Inert modal reducer. Its owner schedules work; paint/input never start Java
//! or touch the filesystem. D07 composes it into the connection/driver dialogs.
use crate::{
    editor::display::display_text,
    focus::EventTarget,
    input::terminal::{InputEvent, Key},
    layout::centered,
    theme::Theme,
    widgets::controls::{button, draw_text},
};
use kode_core::JobTag;
use kode_database::{
    bridge::client::Result,
    drivers::{Installed, Phase, Progress},
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Block, Borders, Clear, Widget},
};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Cancel,
    Dismiss,
}
pub struct DriverInstallDialog {
    pub tag: JobTag,
    pub driver_name: String,
    pub message: String,
    pub done: bool,
    pub success: bool,
    pub installed: Option<Installed>,
    progress: watch::Receiver<Progress>,
    cancel_requested: bool,
    button: Rect,
    action: Option<Action>,
}
impl DriverInstallDialog {
    pub fn new(tag: JobTag, driver_name: String, progress: watch::Receiver<Progress>) -> Self {
        Self {
            tag,
            driver_name,
            message: "Preparing driver download…".into(),
            done: false,
            success: false,
            installed: None,
            progress,
            cancel_requested: false,
            button: Rect::default(),
            action: None,
        }
    }
    pub fn take_action(&mut self) -> Option<Action> {
        self.action.take()
    }
    pub fn cancel_requested(&self) -> bool {
        self.cancel_requested
    }
    /// Only the matching workspace/request may finish this modal. A cancel
    /// acknowledgement is deliberately not accepted as a completion result.
    pub fn complete(&mut self, tag: JobTag, result: Result<Installed>) -> bool {
        if tag != self.tag || self.done {
            return false;
        }
        self.done = true;
        match result {
            Ok(installed) => {
                self.success = true;
                self.message = format!(
                    "Driver installed: {} ({} JARs){}",
                    installed.resolved_version,
                    installed.classpath.len(),
                    if installed.cleanup_warning {
                        "; temporary-file cleanup needs attention"
                    } else {
                        ""
                    }
                );
                self.installed = Some(installed);
            }
            Err(error) => {
                self.success = false;
                self.message = if error.remote_code == Some(-32800) && !error.unknown_outcome {
                    "Download cancelled.".into()
                } else if error.unknown_outcome {
                    "Installation stopped; refresh driver status to check whether it finished."
                        .into()
                } else {
                    error.to_string()
                };
            }
        }
        true
    }
    fn activate(&mut self) {
        if self.done {
            self.action = Some(Action::Dismiss);
        } else if !self.cancel_requested {
            self.cancel_requested = true;
            self.action = Some(Action::Cancel);
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let modal = centered(area.intersection(buffer.area), 82, 12);
        self.button = Rect::default();
        if modal.is_empty() {
            return;
        }
        Clear.render(modal, buffer);
        let title = format!("Install JDBC driver: {}", display_text(&self.driver_name));
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(theme.style("project-search-dialog"))
            .border_style(theme.style("project-search-dialog-border"))
            .render(modal, buffer);
        let padding = u16::from(modal.width >= 4);
        let width = modal.width.saturating_sub(padding * 2);
        let x = modal.x + padding;
        let content = modal.inner(ratatui::layout::Margin::new(
            padding,
            u16::from(modal.height >= 3),
        ));
        let progress = self.progress.borrow().clone();
        let phase = if self.cancel_requested && !self.done {
            "Cancelling; waiting for the download to stop…"
        } else {
            match progress.phase {
                Phase::Queued => "Preparing driver download…",
                Phase::Resolving => "Finding the driver and its dependencies…",
                Phase::Downloading => "Downloading driver files…",
                Phase::Publishing => "Finishing installation…",
                Phase::Cancelling => "Cancelling; waiting for the download to stop…",
                Phase::Ready => "Waiting for installation confirmation…",
                Phase::Cancelled | Phase::Failed => "Waiting for the operation's final result…",
            }
        };
        if content.height > 0 {
            draw_text(
                Rect::new(content.x, content.y, content.width, 1),
                buffer,
                &display_text(if self.done { &self.message } else { phase }),
                theme.style("project-search-dialog"),
            );
        }
        if content.height > 2 {
            let received = progress.completed_bytes as f64 / (1024.0 * 1024.0);
            let description = if self.done && self.success {
                "100%".into()
            } else if let Some(total) = progress.total_bytes.filter(|total| *total > 0) {
                format!(
                    "{:.1} MiB / {:.1} MiB",
                    received,
                    total as f64 / (1024.0 * 1024.0)
                )
            } else {
                format!("{received:.1} MiB received")
            };
            draw_text(
                Rect::new(content.x, content.y + 2, content.width, 1),
                buffer,
                &description,
                theme.style("project-search-dialog"),
            );
        }
        if content.height > 4 {
            let cells = content.width.saturating_sub(8) as usize;
            let bar = if self.done && self.success {
                format!("[{}] 100%", "#".repeat(cells))
            } else if self.done {
                "Installation incomplete".into()
            } else if let Some(total) = progress.total_bytes.filter(|value| *value > 0) {
                let percent =
                    ((progress.completed_bytes as u128 * 100 / total as u128).min(99)) as usize;
                let fill = cells * percent / 100;
                format!(
                    "[{}{}] {percent}%",
                    "#".repeat(fill),
                    "-".repeat(cells - fill)
                )
            } else {
                format!("[{}]", "·".repeat(cells))
            };
            draw_text(
                Rect::new(content.x, content.y + 4, content.width, 1),
                buffer,
                &bar,
                theme.style("project-search-dialog"),
            );
        }
        let label = if self.done {
            " OK "
        } else if self.cancel_requested {
            " Cancelling "
        } else {
            " Cancel "
        };
        let length = (label.len() as u16).min(width);
        self.button = Rect::new(
            x + width.saturating_sub(length),
            modal
                .bottom()
                .saturating_sub(if modal.height >= 3 { 2 } else { 1 }),
            length,
            u16::from(modal.height > 0),
        );
        draw_text(self.button, buffer, label, theme.style("lsp-button"));
    }
}
impl EventTarget for DriverInstallDialog {
    fn dispatch(&mut self, event: &InputEvent) -> bool {
        if let InputEvent::Key(input) = event
            && input.key == Key::Escape
        {
            if !self.done {
                self.activate();
            }
            return true;
        }
        if button(event, self.button, true) {
            self.activate();
        }
        true
    }
}
