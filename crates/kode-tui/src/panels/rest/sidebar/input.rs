use super::*;
use crate::input::terminal::{Key, PointerAction};
use crossterm::event::MouseButton;
impl Sidebar {
    fn open_cursor(&mut self, model: &mut Workspace, focus: &mut FocusState) -> Result<()> {
        let node = self.visible.iter().find(|node| node.path == self.cursor);
        if node.is_some_and(|node| node.has_children) && !self.expanded.contains(&self.cursor) {
            self.expanded.insert(self.cursor.clone());
            model.select(&self.cursor);
            self.dirty = true;
        } else {
            self.perform(Action::Open(self.cursor.clone()), model, focus)?;
        }
        Ok(())
    }
    pub(super) fn dispatch_input(
        &mut self,
        event: &InputEvent,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if matches!(event, InputEvent::Focus(false)) {
            self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
            self.dragging = false;
            if focus.captured == Some(self.id) {
                focus.captured = None;
            }
            return Ok(false);
        }
        if self.modal_input(event, model, focus)? {
            return Ok(true);
        }
        if matches!(event, InputEvent::Resize { .. }) {
            self.follow_cursor = true;
            self.follow_environment = true;
        }
        self.refresh(model);
        if let InputEvent::Pointer(pointer) = event {
            let point = Position::new(pointer.x, pointer.y);
            if self.dragging {
                match pointer.action {
                    PointerAction::Drag(MouseButton::Left) | PointerAction::Move => {
                        let height = self
                            .geometry
                            .area
                            .bottom()
                            .saturating_sub(pointer.y.saturating_add(1))
                            as i32;
                        if model.ui().environment_panel_height != height.clamp(5, 60) {
                            model.set_environment_panel_height(height);
                        }
                        return Ok(true);
                    }
                    PointerAction::Up(MouseButton::Left) => {
                        self.dragging = false;
                        focus.captured = None;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
            if !self.geometry.area.contains(point) {
                return Ok(false);
            }
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                focus.focused = Some(self.id);
                if self.geometry.divider.contains(point) {
                    self.dragging = true;
                    focus.captured = Some(self.id);
                    return Ok(true);
                }
                if self.filtering
                    && self
                        .filter
                        .hit_test(self.geometry.filter, pointer.x, pointer.y)
                {
                    return Ok(true);
                }
                if let Some(hit) = self
                    .hits
                    .iter()
                    .find(|hit| hit.area.contains(point))
                    .cloned()
                {
                    self.perform(hit.action, model, focus)?;
                    return Ok(true);
                }
            }
            if let PointerAction::Scroll { vertical, .. } = pointer.action {
                if self.geometry.environments.contains(point) {
                    self.follow_environment = false;
                    self.environment_scroll = self
                        .environment_scroll
                        .saturating_add_signed(vertical as isize * 3);
                } else {
                    self.follow_cursor = false;
                    self.scroll = self.scroll.saturating_add_signed(vertical as isize * 3);
                }
                return Ok(true);
            }
            if matches!(
                pointer.action,
                PointerAction::Down(MouseButton::Right | MouseButton::Middle)
            ) && self.geometry.environments.contains(point)
                && let Some(Hit {
                    action: Action::OpenEnvironment(id),
                    ..
                }) = self
                    .hits
                    .iter()
                    .find(|hit| hit.area.contains(point))
                    .cloned()
            {
                self.perform(Action::EnvironmentMenu(id), model, focus)?;
                return Ok(true);
            }
            if matches!(pointer.action, PointerAction::Down(_))
                && self.geometry.tree.contains(point)
            {
                let index = self.scroll + (pointer.y - self.geometry.tree.y) as usize;
                if let Some(node) = self.visible.get(index) {
                    self.cursor = node.path.clone();
                    self.environment_focus = false;
                    if matches!(
                        pointer.action,
                        PointerAction::Down(MouseButton::Right | MouseButton::Middle)
                    ) {
                        self.perform(Action::Menu(self.cursor.clone()), model, focus)?;
                    } else {
                        self.open_cursor(model, focus)?;
                    }
                }
                return Ok(true);
            }
            return Ok(false);
        }
        if focus.focused != Some(self.id) {
            return Ok(false);
        }
        if self.filtering {
            if let InputEvent::Key(input) = event {
                match input.key {
                    Key::Escape => {
                        self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
                        self.filter.value.clear();
                        self.filter.cursor = 0;
                        self.filter.anchor = None;
                        self.filtering = false;
                        self.dirty = true;
                        return Ok(true);
                    }
                    Key::Enter => {
                        self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
                        self.filtering = false;
                        return Ok(true);
                    }
                    _ => {}
                }
            }
            if let Some(action) = self.filter.handle(event) {
                self.filter_action(action);
            }
            self.dirty = true;
            return Ok(true);
        }
        let InputEvent::Key(input) = event else {
            return Ok(false);
        };
        if input.modifiers.ctrl || input.modifiers.alt || input.modifiers.meta {
            return Ok(false);
        }
        if input.key == Key::Tab {
            self.environment_focus = !self.environment_focus;
            return Ok(true);
        }
        if self.environment_focus {
            let environments = model.environments();
            let index = self
                .environment
                .as_ref()
                .and_then(|id| {
                    environments
                        .iter()
                        .position(|environment| &environment.id == id)
                })
                .unwrap_or(0);
            let target = match input.key {
                Key::Up => Some(index.saturating_sub(1)),
                Key::Down => Some((index + 1).min(environments.len().saturating_sub(1))),
                Key::PageUp => Some(index.saturating_sub(10)),
                Key::PageDown => Some(
                    index
                        .saturating_add(10)
                        .min(environments.len().saturating_sub(1)),
                ),
                Key::Home => Some(0),
                Key::End => Some(environments.len().saturating_sub(1)),
                _ => None,
            };
            if let Some(index) = target {
                self.follow_environment = true;
                self.environment = environments
                    .get(index)
                    .map(|environment| environment.id.clone());
                return Ok(true);
            }
            let action = match (input.key, self.environment.clone()) {
                (Key::Character('n' | '+'), _) => Some(Action::NewEnvironment),
                (Key::Enter, Some(id)) => Some(Action::OpenEnvironment(id)),
                (Key::Character(' '), Some(id)) => Some(Action::ActivateEnvironment(id)),
                (Key::Character('c'), Some(id)) => Some(Action::CopyEnvironment(id)),
                (Key::Character('m'), Some(id)) => Some(Action::EnvironmentMenu(id)),
                (Key::Function(2), Some(id)) => Some(Action::RenameEnvironment(id)),
                (Key::Delete, Some(id)) => Some(Action::RemoveEnvironment(id)),
                _ => None,
            };
            if let Some(action) = action {
                self.perform(action, model, focus)?;
                return Ok(true);
            }
            return Ok(false);
        }
        let index = self
            .visible
            .iter()
            .position(|node| node.path == self.cursor)
            .unwrap_or(0);
        let target = match input.key {
            Key::Up => Some(index.saturating_sub(1)),
            Key::Down => Some((index + 1).min(self.visible.len().saturating_sub(1))),
            Key::PageUp => Some(index.saturating_sub(10)),
            Key::PageDown => Some(
                index
                    .saturating_add(10)
                    .min(self.visible.len().saturating_sub(1)),
            ),
            Key::Home => Some(0),
            Key::End => Some(self.visible.len().saturating_sub(1)),
            _ => None,
        };
        if let Some(index) = target {
            self.follow_cursor = true;
            if let Some(node) = self.visible.get(index) {
                self.cursor = node.path.clone();
            }
            return Ok(true);
        }
        let action = match input.key {
            Key::Enter | Key::Right => {
                self.open_cursor(model, focus)?;
                return Ok(true);
            }
            Key::Left => {
                self.expanded.remove(&self.cursor);
                self.dirty = true;
                return Ok(true);
            }
            Key::Character('/') => {
                self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
                self.filtering = true;
                self.filter.cursor = self.filter.value.len();
                return Ok(true);
            }
            Key::Character('n') => Some(Action::AddRequest(self.cursor.clone())),
            Key::Character('f') => Some(Action::AddFolder(self.cursor.clone())),
            Key::Character('r') => Some(Action::Run(self.cursor.clone())),
            Key::Character('i') => Some(Action::ImportPostman),
            Key::Character('e') => Some(Action::Export),
            Key::Character('m') => Some(Action::Menu(self.cursor.clone())),
            Key::Function(2) => Some(Action::Rename(self.cursor.clone())),
            Key::Delete => Some(Action::Delete(self.cursor.clone())),
            _ => None,
        };
        if let Some(action) = action {
            self.perform(action, model, focus)?;
            return Ok(true);
        }
        Ok(false)
    }
}
