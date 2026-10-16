use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    layout::centered,
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result};
use kode_platform::jobs::{CancellationToken, Progress};
use kode_workspace::project::Project;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectWork {
    AddSource,
    AddExclusion,
    RemoveSource(PathBuf),
    RemoveExclusion(PathBuf),
    Rescan,
    RebuildIndex,
    ClearIndex,
}
pub struct ProjectPanel {
    pub id: FocusId,
    pub selected: usize,
    pub scroll: usize,
    pub message: String,
    area: Rect,
    buttons: Vec<(Rect, ProjectWork)>,
    work: Vec<ProjectWork>,
}
impl ProjectPanel {
    pub fn new() -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            selected: 0,
            scroll: 0,
            message: String::new(),
            area: Rect::default(),
            buttons: vec![],
            work: vec![],
        })
    }
    pub fn take_work(&mut self) -> Vec<ProjectWork> {
        std::mem::take(&mut self.work)
    }
    fn entries(project: &Project) -> Vec<(PathBuf, bool)> {
        project
            .config
            .source_roots
            .iter()
            .map(|path| (path.clone(), false))
            .chain(
                project
                    .config
                    .exclusions
                    .iter()
                    .map(|path| (path.clone(), true)),
            )
            .collect()
    }
    fn rows(&self) -> Rect {
        Rect::new(
            self.area.x,
            self.area.y + self.area.height.min(5),
            self.area.width,
            self.area.height.saturating_sub(5),
        )
    }
    fn remove(&mut self, entries: &[(PathBuf, bool)]) {
        if let Some((path, excluded)) = entries.get(self.selected) {
            self.work.push(if *excluded {
                ProjectWork::RemoveExclusion(path.clone())
            } else {
                ProjectWork::RemoveSource(path.clone())
            });
        }
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        project: &Project,
        focus: &mut FocusState,
    ) -> bool {
        let entries = Self::entries(project);
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        let rows = self.rows();
        match event {
            InputEvent::Pointer(pointer)
                if self.area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                match pointer.action {
                    PointerAction::Down(MouseButton::Left) => {
                        focus.focused = Some(self.id);
                        if let Some((_, command)) = self
                            .buttons
                            .iter()
                            .find(|(area, _)| area.contains(Position::new(pointer.x, pointer.y)))
                        {
                            self.work.push(command.clone());
                        } else if rows.contains(Position::new(pointer.x, pointer.y)) {
                            self.selected = (self.scroll + (pointer.y - rows.y) as usize)
                                .min(entries.len().saturating_sub(1));
                        }
                    }
                    PointerAction::Scroll { vertical, .. } => {
                        self.scroll = self
                            .scroll
                            .saturating_add_signed(vertical as isize)
                            .min(entries.len().saturating_sub(rows.height as usize))
                    }
                    _ => return false,
                }
            }
            InputEvent::Key(input)
                if focus.focused == Some(self.id)
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                let page = rows.height as usize;
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Project,
                    *input,
                ) else {
                    return false;
                };
                match input.key {
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::Down => {
                        self.selected = (self.selected + 1).min(entries.len().saturating_sub(1))
                    }
                    Key::PageUp => self.selected = self.selected.saturating_sub(page),
                    Key::PageDown => {
                        self.selected = (self.selected + page).min(entries.len().saturating_sub(1))
                    }
                    Key::Home => self.selected = 0,
                    Key::End => self.selected = entries.len().saturating_sub(1),
                    Key::Character('a' | 'A') => self.work.push(ProjectWork::AddSource),
                    Key::Character('x' | 'X') => self.work.push(ProjectWork::AddExclusion),
                    Key::Character('r' | 'R') => self.work.push(ProjectWork::Rescan),
                    Key::Character('b' | 'B') => self.work.push(ProjectWork::RebuildIndex),
                    Key::Character('c' | 'C') => self.work.push(ProjectWork::ClearIndex),
                    Key::Character('d' | 'D') | Key::Backspace | Key::Delete => {
                        self.remove(&entries)
                    }
                    _ => return false,
                }
                if page > 0 {
                    if self.selected < self.scroll {
                        self.scroll = self.selected;
                    } else if self.selected >= self.scroll + page {
                        self.scroll = self.selected + 1 - page;
                    }
                }
            }
            _ => return false,
        }
        true
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, project: &Project) {
        self.area = area.intersection(buffer.area);
        let area = self.area;
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("content"));
        self.buttons.clear();
        let header = [
            "Project Settings".into(),
            format!("Root: {}", display_text(&project.root.to_string_lossy())),
            format!(
                "Source roots: {}   Scan exclusions: {}",
                project.config.source_roots.len(),
                project.config.exclusions.len()
            ),
            display_text(&self.message),
        ];
        for (row, text) in header.iter().take(area.height as usize).enumerate() {
            draw_text(
                Rect::new(area.x, area.y + row as u16, area.width, 1),
                buffer,
                text,
                theme.style("content"),
            );
        }
        if area.height > 3 && self.message.is_empty() {
            let mut x = area.x;
            for (label, command) in [
                ("[rebuild(b)]", ProjectWork::RebuildIndex),
                ("[clear(c)]", ProjectWork::ClearIndex),
            ] {
                if x + label.len() as u16 > area.right() {
                    continue;
                }
                let hit = Rect::new(x, area.y + 3, label.len() as u16, 1);
                draw_text(hit, buffer, label, theme.style("lsp-button"));
                self.buttons.push((hit, command));
                x += (label.len() + 1) as u16;
            }
        }
        let entries = Self::entries(project);
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        if area.height > 4 {
            let remove = entries.get(self.selected).map(|(path, excluded)| {
                if *excluded {
                    ProjectWork::RemoveExclusion(path.clone())
                } else {
                    ProjectWork::RemoveSource(path.clone())
                }
            });
            let buttons = [
                ("[rescan(r)]", Some(ProjectWork::Rescan)),
                ("[source(a)]", Some(ProjectWork::AddSource)),
                ("[exclude(x)]", Some(ProjectWork::AddExclusion)),
                ("[remove(d)]", remove),
            ];
            let mut x = area.x;
            for (label, command) in buttons {
                if x + label.len() as u16 > area.right() {
                    continue;
                }
                let hit = Rect::new(x, area.y + 4, label.len() as u16, 1);
                draw_text(hit, buffer, label, theme.style("lsp-button"));
                if let Some(command) = command {
                    self.buttons.push((hit, command));
                }
                x += (label.len() + 1) as u16;
            }
        }
        let rows = self.rows();
        self.scroll = self
            .scroll
            .min(entries.len().saturating_sub(rows.height as usize));
        if entries.is_empty() {
            draw_text(
                rows,
                buffer,
                "No source roots or scan exclusions",
                theme.style("content"),
            );
        }
        for (row, (path, excluded)) in entries
            .iter()
            .skip(self.scroll)
            .take(rows.height as usize)
            .enumerate()
        {
            let label = if path.as_os_str().is_empty() || path == std::path::Path::new(".") {
                "(project root)".into()
            } else {
                display_text(&path.to_string_lossy())
            };
            draw_text(
                Rect::new(rows.x, rows.y + row as u16, rows.width, 1),
                buffer,
                &format!(
                    "{} {}",
                    if *excluded { "[exclude]" } else { "[source] " },
                    label
                ),
                theme.style(if row + self.scroll == self.selected {
                    "file-entry:selected"
                } else {
                    "file-entry"
                }),
            );
        }
    }
}

/// The actual scan/index job supplies this token and tag. Aborting is a request;
/// completion is displayed only after its worker has acknowledged cancellation.
pub struct RescanDialog {
    pub tag: JobTag,
    pub status: String,
    pub file: String,
    pub done: bool,
    pub aborted: bool,
    pub finished: bool,
    token: CancellationToken,
    button: Rect,
}
impl RescanDialog {
    pub fn new(tag: JobTag, token: CancellationToken) -> Self {
        Self {
            tag,
            token,
            status: "Scanning project".into(),
            file: String::new(),
            done: false,
            aborted: false,
            finished: false,
            button: Rect::default(),
        }
    }
    pub fn progress(&mut self, progress: &Progress) -> bool {
        if self.done || progress.tag != self.tag {
            return false;
        }
        self.status = match progress.total {
            Some(total) => format!("{}/{}", progress.completed, total),
            None => format!("{} processed", progress.completed),
        };
        self.file = progress.message.chars().take(512).collect();
        true
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<()>) -> bool {
        if self.done || tag != self.tag {
            return false;
        }
        self.done = true;
        self.aborted =
            self.token.is_cancelled() || matches!(result, Err(kode_core::DomainError::Cancelled));
        self.status = match result {
            Ok(()) => if self.aborted {
                "Scan aborted"
            } else {
                "Scan complete"
            }
            .into(),
            Err(error) => error.to_string(),
        };
        true
    }
    fn activate(&mut self) {
        if self.done {
            self.finished = true;
        } else {
            self.token.cancel();
            self.status = "Aborting scan…".into();
        }
    }
    pub fn dispatch(&mut self, event: &InputEvent) -> bool {
        if self.finished {
            return false;
        }
        match event {
            InputEvent::Key(input)
                if input.key == Key::Escape || (input.key == Key::Enter && self.done) =>
            {
                self.activate()
            }
            InputEvent::Pointer(pointer)
                if pointer.action == PointerAction::Down(MouseButton::Left)
                    && self.button.contains(Position::new(pointer.x, pointer.y)) =>
            {
                self.activate()
            }
            _ => {}
        }
        true
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.button = Rect::default();
        if self.finished {
            return;
        }
        let modal = centered(area.intersection(buffer.area), 72, 9);
        Clear.render(modal, buffer);
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Project rescan")
            .style(theme.style("project-search-dialog"));
        let inner = block.inner(modal);
        block.render(modal, buffer);
        if inner.is_empty() {
            return;
        }
        if inner.height >= 2 {
            draw_text(
                Rect::new(inner.x, inner.y, inner.width, 1),
                buffer,
                &display_text(&self.status),
                theme.style("content"),
            );
        }
        if inner.height >= 3 {
            draw_text(
                Rect::new(inner.x, inner.y + 1, inner.width, 1),
                buffer,
                &display_text(&self.file),
                theme.style("content"),
            );
        }
        self.button = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        draw_text(
            self.button,
            buffer,
            if self.done {
                "[close (Enter)]"
            } else {
                "[abort (Esc)]"
            },
            theme.style("lsp-button"),
        );
    }
}
