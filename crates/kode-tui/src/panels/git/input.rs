use super::*;
use crate::{
    input::terminal::*,
    layout::centered,
    widgets::{dialog::DialogAction, field::FieldAction},
};
use crossterm::event::MouseButton;
impl GitPanel {
    fn set_part(&mut self, part: Part, focus: &mut FocusState) {
        if part != Part::Message {
            self.dismiss_popups();
        }
        self.part = part;
        focus.focused = Some(self.focus_id());
    }
    fn open_branches(&mut self, focus: &mut FocusState) -> Result<()> {
        let Some(snapshot) = self.read.snapshot.clone() else {
            return Ok(());
        };
        if snapshot.branches.entries.is_empty() {
            self.message = "No local branches available".into();
            return Ok(());
        }
        let selected = snapshot
            .branches
            .entries
            .iter()
            .position(|branch| branch.current)
            .unwrap_or(0);
        self.modal_id = Some(RequestId::fresh()?);
        self.modal_previous_focus = focus.focused;
        self.menu = Some(Menu::Branches {
            snapshot,
            selected,
            scroll: 0,
        });
        Ok(())
    }
    fn open_tag(&mut self, move_existing: bool, focus: &mut FocusState) -> Result<()> {
        let Some(snapshot) = &self.read.snapshot else {
            return Ok(());
        };
        let commit = self
            .selected_commit
            .as_ref()
            .and_then(|hash| {
                snapshot
                    .commits
                    .entries
                    .iter()
                    .find(|commit| &commit.hash == hash)
            })
            .or_else(|| snapshot.commits.entries.first());
        let Some(commit) = commit else {
            self.message = "Select a commit to tag".into();
            return Ok(());
        };
        let title = format!(
            "{} tag: {}",
            if move_existing {
                "Create/move"
            } else {
                "Create"
            },
            commit.hash.chars().take(8).collect::<String>()
        );
        let initial = snapshot
            .tags
            .entries
            .iter()
            .find(|tag| {
                tag.annotated
                    && tag.commit.as_deref() == Some(commit.hash.as_str())
                    && tag.name.rsplit('/').next() == commit.tag.as_deref()
            })
            .map(|tag| tag.name.clone())
            .unwrap_or_default();
        let mut dialog = TextDialog::new(title, initial);
        dialog.field.max_chars = 1024;
        self.modal_id = Some(RequestId::fresh()?);
        self.modal_previous_focus = focus.focused;
        self.menu = Some(Menu::Tag {
            dialog,
            revision: commit.hash.clone(),
            move_existing,
        });
        Ok(())
    }
    fn close_menu(&mut self, focus: &mut FocusState) {
        self.menu = None;
        self.modal_id = None;
        self.paste = None;
        focus.focused = self.modal_previous_focus.take().or(Some(self.focus_id()));
    }
    fn menu_input(&mut self, event: &InputEvent, focus: &mut FocusState) -> Result<bool> {
        let Some(menu) = self.menu.take() else {
            return Ok(false);
        };
        match menu {
            Menu::Branches {
                snapshot,
                mut selected,
                mut scroll,
            } => {
                let branches = &snapshot.branches.entries;
                let area = centered(self.modal_viewport, 56, (branches.len() + 2).min(18) as u16);
                let rows = area.height.saturating_sub(2) as usize;
                let mut chosen = None;
                let mut close = false;
                match event {
                    InputEvent::Key(input) => match input.key {
                        Key::Escape => close = true,
                        Key::Up => selected = selected.saturating_sub(1),
                        Key::Down => {
                            selected = (selected + 1).min(branches.len().saturating_sub(1))
                        }
                        Key::PageUp => selected = selected.saturating_sub(rows.max(1)),
                        Key::PageDown => {
                            selected =
                                (selected + rows.max(1)).min(branches.len().saturating_sub(1))
                        }
                        Key::Home => selected = 0,
                        Key::End => selected = branches.len().saturating_sub(1),
                        Key::Enter if !input.repeat => {
                            chosen = branches.get(selected).map(|branch| branch.name.clone())
                        }
                        _ => {}
                    },
                    InputEvent::Pointer(pointer) => {
                        let point = Position::new(pointer.x, pointer.y);
                        if pointer.action == PointerAction::Down(MouseButton::Left) {
                            if !area.contains(point) {
                                close = true;
                            } else if pointer.y > area.y
                                && pointer.y < area.bottom().saturating_sub(1)
                            {
                                selected = scroll + (pointer.y - area.y - 1) as usize;
                                chosen = branches.get(selected).map(|branch| branch.name.clone());
                            }
                        } else if let PointerAction::Scroll { vertical, .. } = pointer.action {
                            selected = selected
                                .saturating_add_signed(vertical as isize)
                                .min(branches.len().saturating_sub(1));
                        }
                    }
                    _ => {}
                }
                if let Some(branch) = chosen {
                    self.close_menu(focus);
                    self.queue_write(GitCommand::Checkout(branch));
                } else if close {
                    self.close_menu(focus);
                } else {
                    if selected < scroll {
                        scroll = selected;
                    } else if selected >= scroll + rows.max(1) {
                        scroll = selected.saturating_sub(rows.saturating_sub(1));
                    }
                    self.menu = Some(Menu::Branches {
                        snapshot,
                        selected,
                        scroll,
                    });
                }
            }
            Menu::Tag {
                mut dialog,
                revision,
                move_existing,
            } => {
                dialog.handle(event);
                match dialog.action.take() {
                    Some(DialogAction::Submitted(name)) => {
                        self.close_menu(focus);
                        self.queue_write(GitCommand::Tag {
                            name,
                            revision,
                            move_existing,
                        });
                        return Ok(true);
                    }
                    Some(DialogAction::Dismissed) => {
                        self.close_menu(focus);
                        return Ok(true);
                    }
                    Some(DialogAction::Field(FieldAction::Copy(text))) => {
                        let epoch = self.clipboard.remember(&text)?;
                        let tag = self.gate.request("git-copy".into(), None);
                        self.work.push_back(GitWork::Clipboard {
                            tag,
                            job: ClipboardJob::Publish(epoch),
                            read: false,
                        });
                    }
                    Some(DialogAction::Field(FieldAction::PasteRequested)) => {
                        if let Some(modal) = self.modal_id {
                            let tag = self.gate.request("git-paste".into(), None);
                            self.paste = Some(FieldPaste {
                                tag,
                                modal,
                                value: dialog.field.value.clone(),
                                cursor: dialog.field.cursor,
                                anchor: dialog.field.anchor,
                            });
                            self.work.push_back(GitWork::Clipboard {
                                tag,
                                job: ClipboardJob::Read,
                                read: true,
                            });
                        }
                    }
                    Some(DialogAction::Field(FieldAction::LimitReached)) => {
                        self.message = "Tag field is too long".into()
                    }
                    _ => {}
                }
                self.menu = Some(Menu::Tag {
                    dialog,
                    revision,
                    move_existing,
                });
            }
        }
        Ok(true)
    }
    fn history_action(
        &mut self,
        index: usize,
        toggle: bool,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<()> {
        let Some(row) = self.history_rows().get(index).cloned() else {
            return Ok(());
        };
        self.history_selected = index;
        self.selected_commit = Some(row.commit.clone());
        if let Some(path) = row.path {
            self.queue_diff(ReadRequest::HistoricalDiff {
                revision: row.commit,
                path,
            });
        } else if toggle {
            self.toggle_commit(row.commit);
        } else if let Some(message) = self
            .read
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .commits
                    .entries
                    .iter()
                    .find(|commit| commit.hash == row.commit)
            })
            .map(|commit| &commit.message)
        {
            if message.len() > 128 * 1024 {
                self.message = "Commit message exceeds editor copy limit".into();
                return Ok(());
            }
            if let Some(view) = &mut self.commit_editor {
                let length = documents
                    .document(view.document_id())?
                    .snapshot()
                    .len_bytes();
                view.programmatic_replace(
                    TextRange::new(kode_core::ByteOffset(0), kode_core::ByteOffset(length))?,
                    message,
                    documents,
                    self.geometry.editor,
                )?;
            }
            self.set_part(Part::Message, focus);
        }
        Ok(())
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        documents: &mut EditorDocuments,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if matches!(event, InputEvent::Focus(false)) {
            self.commit_arm = false;
            self.divider_drag = None;
            if let Some(view) = &mut self.commit_editor {
                view.dispatch(event, documents, focus, self.geometry.editor)?;
            }
            return Ok(false);
        }
        if self.menu_input(event, focus)? {
            return Ok(true);
        }
        if self.has_popup()
            && let Some(view) = &mut self.commit_editor
            && view.dispatch(event, documents, focus, self.geometry.editor)?
        {
            return Ok(true);
        }
        if let InputEvent::Pointer(pointer) = event {
            if let Some(first) = self.divider_drag {
                match pointer.action {
                    PointerAction::Up(_) => {
                        self.divider_drag = None;
                        focus.captured = None;
                    }
                    PointerAction::Drag(_) | PointerAction::Move => {
                        let relative = pointer.y.saturating_sub(self.geometry.status.y);
                        if first {
                            self.status_height = Some(relative);
                        } else {
                            self.message_height =
                                Some(pointer.y.saturating_sub(self.geometry.editor.y));
                        }
                    }
                    _ => {}
                }
                return Ok(true);
            }
            if self
                .commit_editor
                .as_ref()
                .is_some_and(|view| view.input.is_dragging(focus))
                && let Some(view) = &mut self.commit_editor
            {
                return view.dispatch(event, documents, focus, self.geometry.editor);
            }
            let point = Position::new(pointer.x, pointer.y);
            if pointer.action == PointerAction::Up(MouseButton::Left) {
                let submit = self.commit_arm && self.geometry.commit.contains(point);
                self.commit_arm = false;
                if submit {
                    self.commit(documents)?;
                    return Ok(true);
                }
            }
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                self.commit_arm = false;
                if self.geometry.first_divider.contains(point)
                    || self.geometry.second_divider.contains(point)
                {
                    self.divider_drag = Some(self.geometry.first_divider.contains(point));
                    focus.captured = Some(self.id());
                    return Ok(true);
                }
                if self.geometry.commit.contains(point) {
                    self.commit_arm = true;
                    self.set_part(Part::Message, focus);
                    return Ok(true);
                }
                if self.geometry.editor.contains(point) {
                    self.set_part(Part::Message, focus);
                } else if self.geometry.status.contains(point) {
                    self.set_part(Part::Status, focus);
                    if self.repo_available == Some(false) {
                        if self.init_button.contains(point) {
                            self.queue_write(GitCommand::Init);
                        }
                        return Ok(true);
                    }
                    let header = 1 + usize::from(self.read.error.is_some());
                    let relative = (pointer.y - self.geometry.status.y) as usize;
                    if relative >= header {
                        let index = self.read.status_scroll + relative - header;
                        if self
                            .read
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| index < snapshot.status.entries.len())
                        {
                            self.read.selected_status = index;
                            if pointer.x < self.geometry.status.x.saturating_add(3) {
                                self.toggle_status();
                            } else if let Some(entry) = self
                                .read
                                .snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.status.entries.get(index))
                            {
                                self.queue_diff(ReadRequest::Diff {
                                    path: entry.path.clone(),
                                    staged: entry.staged(),
                                });
                            }
                        }
                    }
                    return Ok(true);
                } else if self.geometry.history.contains(point) {
                    self.set_part(Part::History, focus);
                    if pointer.y > self.geometry.history.y {
                        let index = self.read.history_scroll
                            + (pointer.y - self.geometry.history.y - 1) as usize;
                        self.history_action(
                            index,
                            pointer.x < self.geometry.history.x.saturating_add(3),
                            documents,
                            focus,
                        )?;
                    }
                    return Ok(true);
                } else if self.geometry.toolbar.contains(point) {
                    let x = pointer.x - self.geometry.toolbar.x;
                    match x {
                        0..=2 => self.request_refresh(),
                        4..=6 => self.open_branches(focus)?,
                        8..=10 => self.open_tag(false, focus)?,
                        12..=14 => self.open_tag(true, focus)?,
                        _ => {}
                    }
                    return Ok(true);
                }
            }
            if let PointerAction::Scroll { vertical, .. } = pointer.action {
                if self.geometry.status.contains(point) {
                    let total = self
                        .read
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| snapshot.status.entries.len());
                    let visible = self.geometry.status.height.saturating_sub(2) as usize;
                    self.read.status_scroll = self
                        .read
                        .status_scroll
                        .saturating_add_signed(vertical as isize * 3)
                        .min(total.saturating_sub(visible));
                    return Ok(true);
                }
                if self.geometry.history.contains(point) {
                    let total = self.history_rows().len();
                    let visible = self.geometry.history.height.saturating_sub(1) as usize;
                    self.read.history_scroll = self
                        .read
                        .history_scroll
                        .saturating_add_signed(vertical as isize * 3)
                        .min(total.saturating_sub(visible));
                    return Ok(true);
                }
            }
        }
        if let InputEvent::Key(input) = event {
            if !self.contains_focus(focus) {
                return Ok(false);
            }
            if input.key == Key::Tab
                && (self.part != Part::Message || input.modifiers.ctrl)
                && self
                    .commit_editor
                    .as_ref()
                    .is_none_or(|view| !view.input.state.search_has_focus)
            {
                let next = match (self.part, input.modifiers.shift) {
                    (Part::Status, false) | (Part::History, true) => Part::Message,
                    (Part::Message, false) | (Part::Status, true) => Part::History,
                    _ => Part::Status,
                };
                self.set_part(next, focus);
                return Ok(true);
            }
            if input.key == Key::Enter
                && input.modifiers.ctrl
                && !input.modifiers.alt
                && !input.modifiers.meta
                && self
                    .commit_editor
                    .as_ref()
                    .is_none_or(|view| !view.input.state.search_has_focus)
            {
                if !input.repeat {
                    self.commit(documents)?;
                }
                return Ok(true);
            }
            if self.part != Part::Message {
                if !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta {
                    match input.key {
                        Key::Character('r' | 'R') => {
                            self.request_refresh();
                            return Ok(true);
                        }
                        Key::Character('b' | 'B') => {
                            self.open_branches(focus)?;
                            return Ok(true);
                        }
                        Key::Character('t') => {
                            self.open_tag(false, focus)?;
                            return Ok(true);
                        }
                        Key::Character('T') => {
                            self.open_tag(true, focus)?;
                            return Ok(true);
                        }
                        Key::Character('i' | 'I') if self.repo_available == Some(false) => {
                            self.queue_write(GitCommand::Init);
                            return Ok(true);
                        }
                        _ => {}
                    }
                }
                if self.part == Part::Status {
                    let total = self
                        .read
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| snapshot.status.entries.len());
                    let step = self.geometry.status.height.saturating_sub(2).max(1) as usize;
                    match input.key {
                        Key::Up => {
                            self.read.selected_status = self.read.selected_status.saturating_sub(1)
                        }
                        Key::Down => {
                            self.read.selected_status =
                                (self.read.selected_status + 1).min(total.saturating_sub(1))
                        }
                        Key::PageUp => {
                            self.read.selected_status =
                                self.read.selected_status.saturating_sub(step)
                        }
                        Key::PageDown => {
                            self.read.selected_status =
                                (self.read.selected_status + step).min(total.saturating_sub(1))
                        }
                        Key::Home => self.read.selected_status = 0,
                        Key::End => self.read.selected_status = total.saturating_sub(1),
                        Key::Character(' ') if !input.modifiers.ctrl => self.toggle_status(),
                        Key::Character('s') if !input.modifiers.ctrl => {
                            if let Some(entry) = self.read.snapshot.as_ref().and_then(|snapshot| {
                                snapshot.status.entries.get(self.read.selected_status)
                            }) {
                                self.queue_write(GitCommand::Stage(vec![entry.path.clone()]));
                            }
                        }
                        Key::Character('u') if !input.modifiers.ctrl => {
                            if let Some(entry) = self.read.snapshot.as_ref().and_then(|snapshot| {
                                snapshot.status.entries.get(self.read.selected_status)
                            }) {
                                self.queue_write(GitCommand::Unstage(vec![entry.path.clone()]));
                            }
                        }
                        Key::Character('a' | 'A') if !input.modifiers.ctrl => {
                            let unstage = matches!(input.key, Key::Character('A'));
                            let paths = self
                                .read
                                .snapshot
                                .as_ref()
                                .map(|snapshot| {
                                    snapshot
                                        .status
                                        .entries
                                        .iter()
                                        .filter(|entry| entry.staged() == unstage)
                                        .map(|entry| entry.path.clone())
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default();
                            if !paths.is_empty() {
                                self.queue_write(if unstage {
                                    GitCommand::Unstage(paths)
                                } else {
                                    GitCommand::Stage(paths)
                                });
                            }
                        }
                        Key::Enter => {
                            if let Some(entry) = self.read.snapshot.as_ref().and_then(|snapshot| {
                                snapshot.status.entries.get(self.read.selected_status)
                            }) {
                                self.queue_diff(ReadRequest::Diff {
                                    path: entry.path.clone(),
                                    staged: entry.staged(),
                                });
                            }
                        }
                        _ => return Ok(false),
                    }
                    if self.read.selected_status < self.read.status_scroll {
                        self.read.status_scroll = self.read.selected_status;
                    } else if self.read.selected_status >= self.read.status_scroll + step {
                        self.read.status_scroll = self.read.selected_status + 1 - step;
                    }
                    return Ok(true);
                }
                let rows = self.history_rows();
                let step = self.geometry.history.height.saturating_sub(1).max(1) as usize;
                match input.key {
                    Key::Up => self.history_selected = self.history_selected.saturating_sub(1),
                    Key::Down => {
                        self.history_selected =
                            (self.history_selected + 1).min(rows.len().saturating_sub(1))
                    }
                    Key::PageUp => {
                        self.history_selected = self.history_selected.saturating_sub(step)
                    }
                    Key::PageDown => {
                        self.history_selected =
                            (self.history_selected + step).min(rows.len().saturating_sub(1))
                    }
                    Key::Home => self.history_selected = 0,
                    Key::End => self.history_selected = rows.len().saturating_sub(1),
                    Key::Enter | Key::Character(' ') => {
                        self.history_action(self.history_selected, true, documents, focus)?;
                        return Ok(true);
                    }
                    Key::Character('m') => {
                        self.history_action(self.history_selected, false, documents, focus)?;
                        return Ok(true);
                    }
                    _ => return Ok(false),
                }
                if let Some(row) = rows.get(self.history_selected) {
                    self.selected_commit = Some(row.commit.clone());
                }
                if self.history_selected < self.read.history_scroll {
                    self.read.history_scroll = self.history_selected;
                } else if self.history_selected >= self.read.history_scroll + step {
                    self.read.history_scroll = self.history_selected + 1 - step;
                }
                return Ok(true);
            }
        }
        if self.part == Part::Message
            && let Some(view) = &mut self.commit_editor
        {
            return view.dispatch(event, documents, focus, self.geometry.editor);
        }
        Ok(false)
    }
}
