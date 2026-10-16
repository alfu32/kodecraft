//! File browser reducer. Owned worker commands are consumed by the shell.
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::{
        controls::draw_text,
        dialog::{DialogAction, TextDialog},
    },
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result};
use kode_workspace::{
    exclusions::FolderStatus,
    project::Project,
    tree::{
        EntryKind, FileAction, FileChange, FileTree, FlatEntry, TreeJob, TreeSnapshot, human_size,
        moved_path,
    },
    watch::{RefreshTimer, TreeWatcher},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RecentEntry {
    pub path: PathBuf,
    pub dirty: bool,
}
pub enum FilesWork {
    Refresh(TreeJob),
    Action { tag: JobTag, action: FileAction },
    Open(PathBuf),
    RemoveRecent(PathBuf),
    FullPaths(bool),
    ChangeWorkspace,
    ClipboardCopy(String),
    ClipboardRead { tag: JobTag },
}
#[derive(Debug, Clone)]
enum ButtonAction {
    Change,
    FullPaths,
    NewFile(PathBuf),
    NewDirectory(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
}
struct Hit {
    area: Rect,
    action: ButtonAction,
}
enum NameAction {
    NewFile(PathBuf),
    NewDirectory(PathBuf),
    Rename(PathBuf),
}
struct NameDialog {
    dialog: TextDialog,
    action: NameAction,
    previous_focus: Option<FocusId>,
}
struct NamePaste {
    tag: JobTag,
    value: String,
    cursor: usize,
    anchor: Option<usize>,
}
pub struct FilesPanel {
    pub id: FocusId,
    pub tree: FileTree,
    pub project: Project,
    pub recents: Vec<RecentEntry>,
    pub full_paths: bool,
    pub selected: Option<PathBuf>,
    pub active_path: Option<PathBuf>,
    pub scroll: usize,
    pub recent_scroll: usize,
    pub error: Option<String>,
    hovered: Option<PathBuf>,
    viewport: Rect,
    tree_area: Rect,
    recent_area: Rect,
    hits: Vec<Hit>,
    modal: Option<NameDialog>,
    pending_paste: Option<NamePaste>,
    pending_action: Option<JobTag>,
    refreshing: bool,
    timer: RefreshTimer,
    work: Vec<FilesWork>,
}
impl FilesPanel {
    pub fn new(project: Project) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            tree: FileTree::new(&project),
            project,
            recents: vec![],
            full_paths: false,
            selected: None,
            active_path: None,
            scroll: 0,
            recent_scroll: 0,
            error: None,
            hovered: None,
            viewport: Rect::default(),
            tree_area: Rect::default(),
            recent_area: Rect::default(),
            hits: vec![],
            modal: None,
            pending_paste: None,
            pending_action: None,
            refreshing: false,
            timer: RefreshTimer::default(),
            work: vec![],
        })
    }
    pub fn take_work(&mut self) -> Vec<FilesWork> {
        std::mem::take(&mut self.work)
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.work
            .push(FilesWork::Refresh(self.tree.begin_refresh(&self.project)?));
        self.refreshing = true;
        Ok(())
    }
    pub fn poll_refresh(&mut self, now: u64, watcher: &TreeWatcher) -> Result<()> {
        if watcher.take_dirty() {
            self.timer.changed(now);
        }
        if let Some(error) = watcher.take_error() {
            self.error = Some(error.to_string());
        }
        if self
            .timer
            .due(now, self.refreshing || self.pending_action.is_some())
        {
            self.refresh()?;
        }
        Ok(())
    }
    pub fn complete_refresh(&mut self, tag: JobTag, result: Result<TreeSnapshot>) -> bool {
        if !self.tree.publish(tag, result) {
            return false;
        }
        self.refreshing = false;
        if let Some(error) = self.tree.errors.first() {
            self.error = Some(error.clone());
        }
        let entries = self.tree.flattened();
        // Preserve existing selection and scroll, including a name being edited.
        if let Some(selected) = &self.selected
            && !self.tree.contains_path(selected)
        {
            self.selected = selected
                .ancestors()
                .skip(1)
                .find(|path| entries.iter().any(|entry| entry.entry.path == *path))
                .map(Path::to_path_buf);
        }
        self.scroll = self
            .scroll
            .min(entries.len().saturating_sub(self.tree_area.height as usize));
        true
    }
    pub fn complete_action(
        &mut self,
        tag: JobTag,
        result: Result<FileChange>,
    ) -> Result<Option<FileChange>> {
        if self.pending_action != Some(tag) {
            return Ok(None);
        }
        self.pending_action = None;
        match result {
            Err(error) => {
                self.error = Some(error.to_string());
                // A recursive deletion can fail after removing some entries.
                self.refresh()?;
                Ok(None)
            }
            Ok(change) => {
                self.error = None;
                match &change {
                    FileChange::Created(path) => {
                        self.selected = Some(path.clone());
                        self.tree.open_path(path)?;
                    }
                    FileChange::Renamed { from, to } => {
                        self.tree.reconcile_move(from, to);
                        self.project.reconcile_move(from, to);
                        for path in [&mut self.selected, &mut self.active_path, &mut self.hovered] {
                            if let Some(old) = path.as_ref()
                                && let Some(new) = moved_path(old, from, to)
                            {
                                *path = Some(new);
                            }
                        }
                        for recent in &mut self.recents {
                            if let Some(path) = moved_path(&recent.path, from, to) {
                                recent.path = path;
                            }
                        }
                    }
                    FileChange::Deleted(path) => {
                        if self
                            .selected
                            .as_ref()
                            .is_some_and(|selected| selected.starts_with(path))
                        {
                            self.selected = None;
                        }
                    }
                }
                self.refresh()?;
                Ok(Some(change))
            }
        }
    }
    fn queue_action(&mut self, action: FileAction) -> Result<()> {
        if self.pending_action.is_some() {
            return Ok(());
        }
        let tag = JobTag {
            workspace: self.project.generation,
            request: RequestId::fresh()?,
            document: None,
        };
        self.pending_action = Some(tag);
        self.work.push(FilesWork::Action { tag, action });
        Ok(())
    }
    fn open_dialog(&mut self, action: NameAction, focus: &mut FocusState) {
        if self.pending_action.is_some() {
            return;
        }
        let (title, initial) = match &action {
            NameAction::NewFile(_) => ("Create file".into(), "new-file.txt".into()),
            NameAction::NewDirectory(_) => ("Create folder".into(), "new-dir".into()),
            NameAction::Rename(path) => (
                format!(
                    "Rename {}",
                    display_text(&path.file_name().unwrap_or_default().to_string_lossy())
                ),
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        self.modal = Some(NameDialog {
            dialog: TextDialog::new(title, initial),
            action,
            previous_focus: focus.focused,
        });
        focus.focused = Some(self.id);
        focus.captured = None;
    }
    fn action(&mut self, action: ButtonAction, focus: &mut FocusState) -> Result<()> {
        match action {
            ButtonAction::Change => self.work.push(FilesWork::ChangeWorkspace),
            ButtonAction::FullPaths => self.toggle_full_paths(),
            ButtonAction::NewFile(parent) => self.open_dialog(NameAction::NewFile(parent), focus),
            ButtonAction::NewDirectory(parent) => {
                self.open_dialog(NameAction::NewDirectory(parent), focus)
            }
            ButtonAction::Rename(path) => self.open_dialog(NameAction::Rename(path), focus),
            ButtonAction::Delete(path) => self.queue_action(FileAction::Delete { path })?,
        }
        Ok(())
    }
    fn toggle_full_paths(&mut self) {
        self.full_paths = !self.full_paths;
        self.work.push(FilesWork::FullPaths(self.full_paths));
    }
    fn recent_label(&self, path: &Path) -> String {
        let label = if self.full_paths {
            path
        } else {
            path.strip_prefix(&self.project.root).unwrap_or(path)
        };
        display_text(&if label.as_os_str().is_empty() {
            ".".into()
        } else {
            label.to_string_lossy()
        })
    }
    fn visible(&mut self, index: usize, total: usize) {
        let rows = self.tree_area.height as usize;
        if rows == 0 {
            return;
        }
        if index < self.scroll {
            self.scroll = index;
        } else if index >= self.scroll + rows {
            self.scroll = index + 1 - rows;
        }
        self.scroll = self.scroll.min(total.saturating_sub(rows));
    }
    pub fn select_path(&mut self, path: PathBuf) -> Result<()> {
        self.tree.open_path(&path)?;
        self.selected = Some(path);
        self.refresh()
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some()
    }
    pub fn complete_name_clipboard(
        &mut self,
        tag: JobTag,
        result: Result<String>,
        focus: &FocusState,
    ) -> bool {
        let Some(stamp) = self.pending_paste.take() else {
            return false;
        };
        if stamp.tag != tag {
            self.pending_paste = Some(stamp);
            return false;
        }
        let Some(modal) = &mut self.modal else {
            return false;
        };
        if focus.focused != Some(self.id)
            || modal.dialog.focus != 0
            || modal.dialog.field.value != stamp.value
            || modal.dialog.field.cursor != stamp.cursor
            || modal.dialog.field.anchor != stamp.anchor
        {
            return false;
        }
        match result {
            Ok(text) => modal.dialog.handle(&InputEvent::Paste(text)),
            Err(error) => self.error = Some(error.to_string()),
        }
        true
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<bool> {
        if let Some(modal) = &mut self.modal {
            if matches!(
                event,
                InputEvent::Key(_) | InputEvent::Paste(_) | InputEvent::Focus(false)
            ) || matches!(event, InputEvent::Pointer(pointer) if matches!(pointer.action, PointerAction::Down(_)))
            {
                self.pending_paste = None;
            }
            modal.dialog.handle(event);
            if modal.dialog.finished {
                self.pending_paste = None;
                let mut modal = self.modal.take().expect("finished dialog");
                focus.focused = modal.previous_focus;
                focus.captured = None;
                if let Some(DialogAction::Submitted(name)) = modal.dialog.action.take() {
                    let action = match modal.action {
                        NameAction::NewFile(parent) => FileAction::CreateFile { parent, name },
                        NameAction::NewDirectory(parent) => {
                            FileAction::CreateDirectory { parent, name }
                        }
                        NameAction::Rename(path) => FileAction::Rename { path, name },
                    };
                    self.queue_action(action)?;
                }
            } else if let Some(DialogAction::Field(action)) = modal.dialog.action.take() {
                match action {
                    crate::widgets::field::FieldAction::Copy(text) => {
                        self.work.push(FilesWork::ClipboardCopy(text))
                    }
                    crate::widgets::field::FieldAction::PasteRequested => {
                        let tag = JobTag {
                            workspace: self.project.generation,
                            request: RequestId::fresh()?,
                            document: None,
                        };
                        self.pending_paste = Some(NamePaste {
                            tag,
                            value: modal.dialog.field.value.clone(),
                            cursor: modal.dialog.field.cursor,
                            anchor: modal.dialog.field.anchor,
                        });
                        self.work.push(FilesWork::ClipboardRead { tag });
                    }
                    _ => {
                        self.pending_paste = None;
                    }
                }
            }
            return Ok(true);
        }
        let entries = self.tree.flattened();
        match event {
            InputEvent::Pointer(pointer) => {
                let pos = Position::new(pointer.x, pointer.y);
                if !self.viewport.contains(pos) {
                    if pointer.action == PointerAction::Move {
                        self.hovered = None;
                    }
                    return Ok(false);
                }
                if pointer.action == PointerAction::Move {
                    self.hovered = self
                        .tree_area
                        .contains(pos)
                        .then(|| entries.get(self.scroll + (pointer.y - self.tree_area.y) as usize))
                        .flatten()
                        .map(|entry| entry.entry.path.clone());
                    return Ok(true);
                }
                if let PointerAction::Scroll { vertical, .. } = pointer.action {
                    if self.recent_area.contains(pos) {
                        self.recent_scroll = self
                            .recent_scroll
                            .saturating_add_signed(vertical as isize)
                            .min(
                                self.recents
                                    .len()
                                    .saturating_sub(self.recent_area.height as usize),
                            );
                    } else if self.tree_area.contains(pos) {
                        self.scroll = self
                            .scroll
                            .saturating_add_signed(vertical as isize)
                            .min(entries.len().saturating_sub(self.tree_area.height as usize));
                    }
                    return Ok(true);
                }
                if pointer.action != PointerAction::Down(MouseButton::Left) {
                    return Ok(false);
                }
                focus.focused = Some(self.id);
                if let Some(hit) = self.hits.iter().find(|hit| hit.area.contains(pos)) {
                    self.action(hit.action.clone(), focus)?;
                    return Ok(true);
                }
                if self.recent_area.contains(pos) {
                    if let Some(recent) = self
                        .recents
                        .get(self.recent_scroll + (pointer.y - self.recent_area.y) as usize)
                    {
                        self.work.push(if pointer.x == self.recent_area.x {
                            FilesWork::RemoveRecent(recent.path.clone())
                        } else {
                            FilesWork::Open(recent.path.clone())
                        });
                    }
                    return Ok(true);
                }
                if self.tree_area.contains(pos)
                    && let Some(entry) =
                        entries.get(self.scroll + (pointer.y - self.tree_area.y) as usize)
                {
                    self.activate(entry)?;
                    return Ok(true);
                }
            }
            InputEvent::Key(input)
                if focus.focused == Some(self.id)
                    && !input.modifiers.ctrl
                    && !input.modifiers.alt
                    && !input.modifiers.meta =>
            {
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Files,
                    *input,
                ) else {
                    return Ok(false);
                };
                if input.key == Key::Character('p') {
                    self.toggle_full_paths();
                    return Ok(true);
                }
                if entries.is_empty() {
                    return Ok(false);
                }
                let mut index = self
                    .selected
                    .as_ref()
                    .and_then(|path| entries.iter().position(|entry| &entry.entry.path == path))
                    .unwrap_or(0);
                match input.key {
                    Key::Up => index = index.saturating_sub(1),
                    Key::Down => index = (index + 1).min(entries.len() - 1),
                    Key::PageUp => index = index.saturating_sub(self.tree_area.height as usize),
                    Key::PageDown => {
                        index = (index + self.tree_area.height as usize).min(entries.len() - 1)
                    }
                    Key::Home => index = 0,
                    Key::End => index = entries.len() - 1,
                    Key::Left => {
                        if entries[index].open {
                            self.tree.toggle(&entries[index].entry.path);
                        }
                    }
                    Key::Right | Key::Enter => self.activate(&entries[index])?,
                    Key::Function(2) => self
                        .open_dialog(NameAction::Rename(entries[index].entry.path.clone()), focus),
                    Key::Delete => self.queue_action(FileAction::Delete {
                        path: entries[index].entry.path.clone(),
                    })?,
                    _ => return Ok(false),
                }
                self.selected = Some(entries[index].entry.path.clone());
                self.visible(index, entries.len());
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }
    fn activate(&mut self, entry: &FlatEntry) -> Result<()> {
        self.selected = Some(entry.entry.path.clone());
        if entry.entry.kind == EntryKind::Directory {
            if self.tree.toggle(&entry.entry.path) && self.tree.is_open(&entry.entry.path) {
                self.refresh()?;
            }
        } else {
            self.work.push(FilesWork::Open(entry.entry.path.clone()));
        }
        Ok(())
    }
    fn buttons(
        &mut self,
        area: Rect,
        buttons: Vec<(&str, ButtonAction)>,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        let width: usize = buttons.iter().map(|(label, _)| label.len()).sum();
        let mut x = area
            .right()
            .saturating_sub(width.min(area.width as usize) as u16);
        for (label, action) in buttons {
            let width = (label.len() as u16).min(area.right().saturating_sub(x));
            if width == 0 {
                break;
            }
            let hit = Rect::new(x, area.y, width, 1);
            draw_text(hit, buffer, label, theme.style("lsp-button"));
            self.hits.push(Hit { area: hit, action });
            x += width;
        }
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Option<Position> {
        self.viewport = area.intersection(buffer.area);
        let area = self.viewport;
        self.hits.clear();
        self.tree_area = Rect::default();
        self.recent_area = Rect::default();
        Clear.render(area, buffer);
        if area.is_empty() {
            return None;
        }
        buffer.set_style(area, theme.style("content"));
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            &display_text(&self.tree.root.to_string_lossy()),
            theme.style("file-entry"),
        );
        self.buttons(
            Rect::new(area.x, area.y, area.width, 1),
            vec![
                ("[change]", ButtonAction::Change),
                ("[+d]", ButtonAction::NewDirectory(self.tree.root.clone())),
                ("[+f]", ButtonAction::NewFile(self.tree.root.clone())),
            ],
            buffer,
            theme,
        );
        let mut y = area.y + 1;
        if let Some(error) = &self.error
            && y < area.bottom()
        {
            draw_text(
                Rect::new(area.x, y, area.width, 1),
                buffer,
                &display_text(error),
                theme.style("file-entry:folder:excluded"),
            );
            y += 1;
        }
        if !self.recents.is_empty() && area.bottom().saturating_sub(y) >= 3 {
            let rows = ((area.bottom() - y) * 3 / 10).max(3).min(area.bottom() - y);
            draw_text(
                Rect::new(area.x, y, area.width, 1),
                buffer,
                "Recent",
                theme.style("file-entry"),
            );
            self.buttons(
                Rect::new(area.x, y, area.width, 1),
                vec![(
                    if self.full_paths {
                        "[paths:full]"
                    } else {
                        "[paths:rel]"
                    },
                    ButtonAction::FullPaths,
                )],
                buffer,
                theme,
            );
            self.recent_area = Rect::new(area.x, y + 1, area.width, rows.saturating_sub(2));
            self.recent_scroll = self.recent_scroll.min(
                self.recents
                    .len()
                    .saturating_sub(self.recent_area.height as usize),
            );
            for (row, recent) in self
                .recents
                .iter()
                .skip(self.recent_scroll)
                .take(self.recent_area.height as usize)
                .enumerate()
            {
                let style = theme.style(if self.active_path.as_ref() == Some(&recent.path) {
                    "file-entry:selected"
                } else {
                    "file-entry"
                });
                draw_text(
                    Rect::new(area.x, y + 1 + row as u16, area.width, 1),
                    buffer,
                    &format!(
                        "{} {}",
                        if recent.dirty { '*' } else { 'x' },
                        self.recent_label(&recent.path)
                    ),
                    style,
                );
            }
            draw_text(
                Rect::new(area.x, y + rows - 1, area.width, 1),
                buffer,
                &"-".repeat(area.width as usize),
                theme.style("splitter"),
            );
            y += rows;
        }
        self.tree_area = Rect::new(area.x, y, area.width, area.bottom().saturating_sub(y));
        let entries = self.tree.flattened();
        self.scroll = self
            .scroll
            .min(entries.len().saturating_sub(self.tree_area.height as usize));
        for (row, entry) in entries
            .iter()
            .skip(self.scroll)
            .take(self.tree_area.height as usize)
            .enumerate()
        {
            let line = Rect::new(area.x, y + row as u16, area.width, 1);
            let selected = self.selected.as_ref() == Some(&entry.entry.path);
            let style = theme.style(&entry_style(entry, selected));
            let prefix = if entry.entry.kind != EntryKind::Directory {
                "[=]"
            } else if entry.open {
                "[-]"
            } else {
                "[+]"
            };
            let label = format!(
                "{}{} {}",
                "  ".repeat(entry.depth.min(area.width as usize / 2)),
                prefix,
                display_text(&entry.entry.name)
            );
            draw_text(line, buffer, &label, style);
            if self.hovered.as_ref() == Some(&entry.entry.path) {
                let mut buttons = vec![("[r]", ButtonAction::Rename(entry.entry.path.clone()))];
                if entry.entry.kind == EntryKind::Directory {
                    buttons.extend([
                        ("[+d]", ButtonAction::NewDirectory(entry.entry.path.clone())),
                        ("[+f]", ButtonAction::NewFile(entry.entry.path.clone())),
                    ]);
                }
                buttons.push(("[-]", ButtonAction::Delete(entry.entry.path.clone())));
                self.buttons(line, buttons, buffer, theme);
            } else {
                let metadata = if entry.entry.kind == EntryKind::Directory {
                    entry
                        .entry
                        .child_count
                        .map_or_else(|| "? items".into(), |count| format!("{count} items"))
                } else {
                    human_size(entry.entry.bytes)
                };
                let width = (metadata.len() as u16).min(line.width);
                draw_text(
                    Rect::new(line.right() - width, line.y, width, 1),
                    buffer,
                    &metadata,
                    style,
                );
            }
        }
        self.modal
            .as_mut()
            .and_then(|modal| modal.dialog.render(area, buffer, theme))
    }
}
fn entry_style(entry: &FlatEntry, selected: bool) -> String {
    let mut style = if entry.entry.kind != EntryKind::Directory {
        "file-entry".into()
    } else {
        format!(
            "file-entry:folder{}",
            match entry.entry.status {
                Some(FolderStatus::GitIgnored) => ":ignored",
                Some(FolderStatus::InSource) => ":source",
                Some(FolderStatus::OutsideSource) => ":outside-source",
                Some(FolderStatus::ExcludedSource) => ":excluded",
                None => "",
            }
        )
    };
    if selected {
        style.push_str(":selected");
    }
    style
}
