//! Main-thread LSP settings reducer; no filesystem/network/process work in paint.
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result, WorkspaceGeneration};
use kode_lsp::discovery::{InstallState, ServerStatus};
use kode_platform::jobs::{CancellationToken, Progress};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};
use std::path::PathBuf;
const CARD_HEIGHT: usize = 6;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Start,
    Stop,
    Restart,
}
/// P02 registers its real process registry here. Settings never toggles a local
/// `running` flag or reports a start before the callback acknowledges it.
pub trait ServerLifecycle: Send + Sync {
    fn running(&self, id: &str) -> bool;
    fn apply(
        &self,
        id: &str,
        action: LifecycleAction,
        cancel: &CancellationToken,
    ) -> Result<String>;
    fn stop_all(&self) -> Result<()>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    Refresh,
    Install(String),
    Uninstall(String),
    Lifecycle(String, LifecycleAction),
}
#[derive(Debug)]
pub struct SettingsJob {
    pub tag: JobTag,
    pub action: SettingsAction,
    pub cancel: CancellationToken,
}
pub struct SettingsReport {
    pub statuses: Vec<ServerStatus>,
    pub catalog_path: Option<PathBuf>,
    pub install_root: PathBuf,
    pub message: String,
}
pub struct LspSettings {
    pub id: FocusId,
    pub statuses: Vec<ServerStatus>,
    pub selected: usize,
    pub scroll: usize,
    pub message: String,
    pub install_root: Option<PathBuf>,
    pub catalog_path: Option<PathBuf>,
    pub busy: Option<JobTag>,
    cancel: Option<CancellationToken>,
    work: Vec<SettingsAction>,
    area: Rect,
    hits: Vec<(Rect, usize, SettingsAction)>,
    rows: Vec<(Rect, usize)>,
}
impl LspSettings {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            statuses: vec![],
            selected: 0,
            scroll: 0,
            message: "Language-server catalog has not been loaded".into(),
            install_root: None,
            catalog_path: None,
            busy: None,
            cancel: None,
            work: vec![],
            area: Rect::default(),
            hits: vec![],
            rows: vec![],
        })
    }
    pub fn request_refresh(&mut self) {
        if self.busy.is_none() {
            self.work.push(SettingsAction::Refresh);
        }
    }
    pub fn take_work(&mut self) -> Vec<SettingsAction> {
        std::mem::take(&mut self.work)
    }
    pub fn begin(
        &mut self,
        workspace: WorkspaceGeneration,
        action: SettingsAction,
    ) -> Result<Option<SettingsJob>> {
        if self.busy.is_some() {
            return Ok(None);
        }
        let tag = JobTag {
            workspace,
            request: RequestId::fresh()?,
            document: None,
        };
        let cancel = CancellationToken::new();
        self.busy = Some(tag);
        self.cancel = Some(cancel.clone());
        self.message = format!("Working: {action:?}; Escape cancels");
        Ok(Some(SettingsJob {
            tag,
            action,
            cancel,
        }))
    }
    pub fn cancel(&mut self) {
        if let Some(token) = &self.cancel {
            token.cancel();
            self.message = "Cancelling; waiting for acknowledgement".into();
        }
    }
    pub fn progress(&mut self, progress: &Progress) {
        if self.busy == Some(progress.tag) {
            self.message = progress.message.clone();
        }
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<SettingsReport>) -> bool {
        if self.busy != Some(tag) {
            return false;
        }
        self.busy = None;
        self.cancel = None;
        match result {
            Ok(report) => {
                let selected = self.statuses.get(self.selected).map(|s| s.entry.id.clone());
                self.statuses = report.statuses;
                self.selected = selected
                    .and_then(|id| self.statuses.iter().position(|s| s.entry.id == id))
                    .unwrap_or(self.selected)
                    .min(self.statuses.len().saturating_sub(1));
                self.install_root = Some(report.install_root);
                self.catalog_path = report.catalog_path;
                self.message = report.message;
            }
            Err(error) => self.message = error.to_string(),
        }
        self.reveal();
        true
    }
    fn selected_action(&self, key: Key) -> Option<SettingsAction> {
        let status = self.statuses.get(self.selected)?;
        let id = status.entry.id.clone();
        Some(match key {
            Key::Enter | Key::Character(' ') => SettingsAction::Install(id),
            Key::Delete | Key::Backspace | Key::Character('y') => SettingsAction::Uninstall(id),
            Key::Character('s') => SettingsAction::Lifecycle(
                id,
                if status.running {
                    LifecycleAction::Stop
                } else {
                    LifecycleAction::Start
                },
            ),
            Key::Character('r') => SettingsAction::Lifecycle(id, LifecycleAction::Restart),
            _ => return None,
        })
    }
    fn reveal(&mut self) {
        let count = (self.area.height.saturating_sub(5) as usize / CARD_HEIGHT).max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + count {
            self.scroll = self.selected + 1 - count;
        }
        self.scroll = self.scroll.min(self.statuses.len().saturating_sub(count));
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        match event {
            InputEvent::Pointer(pointer)
                if pointer.action == PointerAction::Down(MouseButton::Left) =>
            {
                let point = Position::new(pointer.x, pointer.y);
                if !self.area.contains(point) {
                    return false;
                }
                focus.focused = Some(self.id);
                if let Some((_, index, action)) =
                    self.hits.iter().find(|(area, _, _)| area.contains(point))
                {
                    self.selected = *index;
                    if self.busy.is_none() {
                        self.work.push(action.clone());
                    }
                } else if let Some((_, index)) =
                    self.rows.iter().find(|(area, _)| area.contains(point))
                {
                    self.selected = *index;
                }
                true
            }
            InputEvent::Pointer(pointer)
                if self.area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                if let PointerAction::Scroll { vertical, .. } = pointer.action {
                    self.scroll = (self.scroll as i64 + vertical as i64)
                        .clamp(0, self.statuses.len().saturating_sub(1) as i64)
                        as usize;
                    return true;
                }
                false
            }
            InputEvent::Key(input)
                if focus.focused == Some(self.id)
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                let count = self.statuses.len();
                let key = match input.key {
                    Key::Character(ch) => Key::Character(ch.to_ascii_lowercase()),
                    key => key,
                };
                match key {
                    Key::Escape => self.cancel(),
                    Key::Character('f') | Key::Function(5) => self.request_refresh(),
                    Key::Up if count > 0 => {
                        self.selected = (self.selected + count - 1) % count;
                        self.reveal();
                    }
                    Key::Down if count > 0 => {
                        self.selected = (self.selected + 1) % count;
                        self.reveal();
                    }
                    key => {
                        if self.busy.is_none() {
                            if let Some(action) = self.selected_action(key) {
                                self.work.push(action);
                            } else {
                                return false;
                            }
                        } else {
                            return false;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        let area = area.intersection(buffer.area);
        self.area = area;
        self.hits.clear();
        self.rows.clear();
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("lsp-list"));
        if area.is_empty() {
            return;
        }
        let line = |row: u16, text: &str, buffer: &mut Buffer, style| {
            if row < area.height {
                draw_text(
                    Rect::new(area.x, area.y + row, area.width, 1),
                    buffer,
                    &display_text(text),
                    style,
                );
            }
        };
        line(0, "LSP Servers", buffer, theme.style("lsp-name"));
        line(
            1,
            &format!(
                "Root: {}",
                self.install_root
                    .as_ref()
                    .map_or("loading".into(), |path| path.display().to_string())
            ),
            buffer,
            theme.style("lsp-list"),
        );
        line(
            2,
            &format!(
                "Catalog: {}",
                self.catalog_path
                    .as_ref()
                    .map_or("compiled".into(), |path| path.display().to_string())
            ),
            buffer,
            theme.style("lsp-list"),
        );
        line(3, &self.message, buffer, theme.style("lsp-error"));
        line(4, "f refresh | Esc cancel", buffer, theme.style("lsp-list"));
        self.scroll = self.scroll.min(self.statuses.len().saturating_sub(1));
        for (relative, status) in self.statuses.iter().skip(self.scroll).enumerate() {
            let index = self.scroll + relative;
            let row = 5 + relative * CARD_HEIGHT;
            if row >= area.height as usize {
                break;
            }
            let style = theme.style(if focused && index == self.selected {
                "lsp-card-selected"
            } else {
                "lsp-list"
            });
            let card = Rect::new(
                area.x,
                area.y + row as u16,
                area.width,
                (CARD_HEIGHT as u16).min(area.height - row as u16),
            );
            buffer.set_style(card, style);
            self.rows.push((card, index));
            line(
                row as u16,
                &format!(
                    "{} ({})",
                    status.entry.name,
                    status.entry.languages.join(",")
                ),
                buffer,
                style,
            );
            line(
                row as u16 + 1,
                &format!(
                    "{:?} | {} | {}",
                    status.install_state,
                    status
                        .installed_version
                        .as_deref()
                        .unwrap_or("local/unversioned"),
                    if status.running { "running" } else { "stopped" }
                ),
                buffer,
                style,
            );
            line(
                row as u16 + 2,
                status.message.as_deref().unwrap_or(""),
                buffer,
                style,
            );
            let install = match status.install_state {
                InstallState::UpdateAvailable => "update",
                InstallState::Installed => "reinstall",
                _ => "install",
            };
            let groups = [
                (
                    3,
                    vec![
                        (
                            format!("s:{}", if status.running { "stop" } else { "start" }),
                            SettingsAction::Lifecycle(
                                status.entry.id.clone(),
                                if status.running {
                                    LifecycleAction::Stop
                                } else {
                                    LifecycleAction::Start
                                },
                            ),
                        ),
                        (
                            "r:restart".into(),
                            SettingsAction::Lifecycle(
                                status.entry.id.clone(),
                                LifecycleAction::Restart,
                            ),
                        ),
                    ],
                ),
                (
                    4,
                    vec![
                        (
                            format!("Enter:{install}"),
                            SettingsAction::Install(status.entry.id.clone()),
                        ),
                        (
                            "y:remove".into(),
                            SettingsAction::Uninstall(status.entry.id.clone()),
                        ),
                    ],
                ),
            ];
            for (offset, buttons) in groups {
                let y = row + offset;
                if y >= area.height as usize {
                    continue;
                }
                let mut x = area.x;
                for (label, action) in buttons {
                    if x >= area.right() {
                        break;
                    }
                    let width = (label.len() as u16).min(area.right() - x);
                    let hit = Rect::new(x, area.y + y as u16, width, 1);
                    draw_text(hit, buffer, &label, theme.style("lsp-button"));
                    self.hits.push((hit, index, action));
                    x = x.saturating_add(width + 1);
                }
            }
        }
    }
}
