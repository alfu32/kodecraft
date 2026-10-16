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
use kode_workspace::{
    project::Project,
    tree::{EntryKind, FileTree, TreeJob, TreeSnapshot},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Block, Borders, Clear, Widget},
};
use std::path::PathBuf;

pub enum PickerWork {
    Refresh(TreeJob),
    Confirmed(PathBuf),
    Cancelled,
}
pub struct FolderPicker {
    pub title: String,
    pub tree: FileTree,
    pub selected: PathBuf,
    pub folders_only: bool,
    pub scroll: usize,
    pub finished: bool,
    pub error: Option<String>,
    pub id: FocusId,
    previous_focus: Option<FocusId>,
    project: Project,
    rows: Rect,
    work: Vec<PickerWork>,
    needs_focus: bool,
}
impl FolderPicker {
    pub fn new(
        project: Project,
        title: String,
        folders_only: bool,
        focus: &mut FocusState,
    ) -> Result<Self> {
        let id = FocusId(RequestId::fresh()?.0);
        let previous_focus = focus.focused;
        focus.focused = Some(id);
        focus.captured = None;
        let mut tree = FileTree::new(&project);
        let initial = tree.begin_refresh(&project)?;
        Ok(Self {
            title,
            selected: project.root.clone(),
            project,
            tree,
            folders_only,
            id,
            previous_focus,
            scroll: 0,
            finished: false,
            error: None,
            rows: Rect::default(),
            work: vec![PickerWork::Refresh(initial)],
            needs_focus: true,
        })
    }
    pub fn take_work(&mut self) -> Vec<PickerWork> {
        std::mem::take(&mut self.work)
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<TreeSnapshot>) -> bool {
        if self.finished || !self.tree.publish(tag, result) {
            return false;
        }
        self.error = self.tree.errors.first().cloned();
        if !self
            .entries()
            .iter()
            .any(|(path, _, _, _)| path == &self.selected)
        {
            self.selected = self.tree.root.clone();
        }
        true
    }
    pub fn focus_path(&mut self, path: PathBuf) -> Result<()> {
        self.tree.open_path(&path)?;
        self.selected = path;
        self.needs_focus = true;
        self.work
            .push(PickerWork::Refresh(self.tree.begin_refresh(&self.project)?));
        Ok(())
    }
    fn entries(&self) -> Vec<(PathBuf, String, usize, bool)> {
        let mut entries = vec![(
            self.tree.root.clone(),
            self.tree
                .root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            0,
            true,
        )];
        entries.extend(
            self.tree
                .flattened()
                .into_iter()
                .filter(|entry| !self.folders_only || entry.entry.kind == EntryKind::Directory)
                .map(|entry| {
                    (
                        entry.entry.path,
                        entry.entry.name,
                        entry.depth + 1,
                        entry.entry.kind == EntryKind::Directory,
                    )
                }),
        );
        entries
    }
    fn reveal(&mut self, index: usize, total: usize) {
        let rows = self.rows.height as usize;
        if rows == 0 {
            return;
        }
        if index < self.scroll {
            self.scroll = index;
        }
        if index >= self.scroll + rows {
            self.scroll = index + 1 - rows;
        }
        self.scroll = self.scroll.min(total.saturating_sub(rows));
    }
    fn finish(&mut self, confirm: bool, focus: &mut FocusState) {
        self.finished = true;
        focus.focused = self.previous_focus;
        focus.captured = None;
        self.work
            .retain(|work| !matches!(work, PickerWork::Refresh(_)));
        self.work.push(if confirm {
            PickerWork::Confirmed(self.selected.clone())
        } else {
            PickerWork::Cancelled
        });
    }
    fn toggle(&mut self) -> Result<()> {
        if self.tree.toggle(&self.selected) && self.tree.is_open(&self.selected) {
            self.work
                .push(PickerWork::Refresh(self.tree.begin_refresh(&self.project)?));
        }
        Ok(())
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<bool> {
        if self.finished {
            return Ok(false);
        }
        let entries = self.entries();
        let mut index = entries
            .iter()
            .position(|(path, _, _, _)| path == &self.selected)
            .unwrap_or(0);
        match event {
            InputEvent::Key(input) => {
                match input.key {
                    Key::Escape => {
                        self.finish(false, focus);
                        return Ok(true);
                    }
                    Key::Enter => {
                        self.finish(true, focus);
                        return Ok(true);
                    }
                    Key::Up => index = index.saturating_sub(1),
                    Key::Down => index = (index + 1).min(entries.len() - 1),
                    Key::PageUp => index = index.saturating_sub(self.rows.height as usize),
                    Key::PageDown => {
                        index = (index + self.rows.height as usize).min(entries.len() - 1)
                    }
                    Key::Home => index = 0,
                    Key::End => index = entries.len() - 1,
                    Key::Left | Key::Right => self.toggle()?,
                    _ => return Ok(true),
                }
                self.selected = entries[index].0.clone();
                self.reveal(index, entries.len());
            }
            InputEvent::Pointer(pointer)
                if self.rows.contains(Position::new(pointer.x, pointer.y)) =>
            {
                match pointer.action {
                    PointerAction::Scroll { vertical, .. } => {
                        self.scroll = self
                            .scroll
                            .saturating_add_signed(vertical as isize)
                            .min(entries.len().saturating_sub(self.rows.height as usize))
                    }
                    PointerAction::Down(MouseButton::Left) => {
                        if let Some((path, _, _, directory)) =
                            entries.get(self.scroll + (pointer.y - self.rows.y) as usize)
                        {
                            self.selected = path.clone();
                            if *directory {
                                self.toggle()?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(true)
    }
    pub fn render_rows(&mut self, rows: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.rows = rows.intersection(buffer.area);
        let entries = self.entries();
        let index = entries
            .iter()
            .position(|(path, _, _, _)| path == &self.selected)
            .unwrap_or(0);
        if self.needs_focus {
            self.reveal(index, entries.len());
            self.needs_focus = false;
        } else {
            self.scroll = self
                .scroll
                .min(entries.len().saturating_sub(self.rows.height as usize));
        }
        for (row, (path, name, depth, directory)) in entries
            .iter()
            .skip(self.scroll)
            .take(self.rows.height as usize)
            .enumerate()
        {
            let prefix = if !directory {
                "[=]"
            } else if self.tree.is_open(path) {
                "[-]"
            } else {
                "[+]"
            };
            draw_text(
                Rect::new(self.rows.x, self.rows.y + row as u16, self.rows.width, 1),
                buffer,
                &format!(
                    "{}{} {}",
                    "  ".repeat((*depth).min(self.rows.width as usize / 2)),
                    prefix,
                    display_text(name)
                ),
                theme.style(if path == &self.selected {
                    "file-entry:selected"
                } else {
                    "file-entry"
                }),
            );
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        self.rows = Rect::default();
        if self.finished {
            return;
        }
        let area = area.intersection(buffer.area);
        let modal = centered(
            area,
            (area.width * 7 / 10).max(20),
            (area.height * 7 / 10).max(6),
        );
        Clear.render(modal, buffer);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(display_text(&self.title))
            .style(theme.style("project-search-dialog"));
        let inner = block.inner(modal);
        block.render(modal, buffer);
        if inner.is_empty() {
            return;
        }
        let header = if inner.height >= 3 { 1 } else { 0 };
        let footer = u16::from(inner.height >= 3);
        if header > 0 {
            draw_text(
                Rect::new(inner.x, inner.y, inner.width, 1),
                buffer,
                "Enter: select   Esc: cancel",
                theme.style("project-search-dialog"),
            );
        }
        self.rows = Rect::new(
            inner.x,
            inner.y + header,
            inner.width,
            inner.height - header - footer,
        );
        self.render_rows(self.rows, buffer, theme);
        if footer > 0 {
            draw_text(
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                buffer,
                &display_text(
                    self.error
                        .as_deref()
                        .unwrap_or(&self.selected.to_string_lossy()),
                ),
                theme.style("project-search-dialog"),
            );
        }
    }
}
