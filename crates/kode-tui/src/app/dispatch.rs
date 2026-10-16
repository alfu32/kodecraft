use super::state::{AppState, AppWork};
use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    panels::project_search::ProjectSearchDialog,
};
use crossterm::event::MouseButton;
use kode_core::Result;
use ratatui::layout::{Position, Rect};

impl AppState {
    pub(super) fn request_quit(&mut self) -> Result<()> {
        self.quit_requested = true;
        self.closing = true;
        self.shell_requested = false;
        self.pending_open = None;
        self.rest.pending_open = None;
        self.rest.ready_open = None;
        if let Some(form) = &mut self.rest.form {
            form.invalidate(&mut self.focus);
        }
        self.dismiss_sql_dialog()?;
        self.rest.sidebar.dismiss(&mut self.focus);
        self.drain_ui()
    }
    pub fn dispatch(&mut self, event: &InputEvent) -> Result<()> {
        let before = self
            .editor
            .as_ref()
            .map(|view| (view.input.state.caret.clone(), view.input.state.scroll_rows));
        self.cache_search_view();
        let result = self.dispatch_event(event);
        self.cache_search_view();
        let after = self
            .editor
            .as_ref()
            .map(|view| (view.input.state.caret.clone(), view.input.state.scroll_rows));
        if before != after {
            self.changed()?;
        }
        result
    }
    fn cache_search_view(&mut self) {
        if let Some(dialog) = &self.search
            && let Some(view) = &dialog.editor
        {
            let id = view.document_id();
            let metadata = self.views.get(&id).and_then(|view| view.metadata.clone());
            self.views.insert(
                id,
                super::state::StoredView {
                    caret: view.input.state.caret.clone(),
                    scroll_rows: view.input.state.scroll_rows,
                    metadata,
                },
            );
        }
    }
    fn dispatch_event(&mut self, event: &InputEvent) -> Result<()> {
        if let InputEvent::Resize { width, height } = event {
            self.geometry = self.layout(Rect::new(0, 0, *width, *height));
        }
        let editor_area = self.rest_editor_area();
        if let InputEvent::Focus(active) = event {
            if let Some(form) = &mut self.rest.form {
                form.dispatch(event, &mut self.rest.model, &mut self.focus)?;
            }
            if let Some(view) = &mut self.rest.response.editor {
                view.dispatch(
                    event,
                    &mut self.documents,
                    &mut self.focus,
                    self.rest.response.body,
                )?;
            }
            if !active {
                self.rest.response.splitter = crate::layout::Splitter::new(
                    crate::layout::Axis::Vertical,
                    self.rest.response.splitter.size,
                );
            }
            self.window_focused = *active;
            self.rest
                .sidebar
                .dispatch(event, &mut self.rest.model, &mut self.focus)?;
            if !active {
                self.focus.captured = None;
            }
            if let Some(view) = &mut self.editor {
                view.dispatch(event, &mut self.documents, &mut self.focus, editor_area)?;
            }
            if let Some(dialog) = &mut self.search {
                dialog.dispatch(event, &mut self.documents, &mut self.focus)?;
            }
            if let Some(view) = &mut self.image {
                view.dispatch(event, &mut self.focus);
            }
            if let Some(path) = &self.active_hex
                && let Some(view) = self.hexes.get_mut(path)
            {
                view.dispatch(event, &mut self.focus);
            }
            self.files.dispatch(event, &mut self.focus)?;
            self.git
                .dispatch(event, &mut self.documents, &mut self.focus)?;
            if let Some(diff) = &mut self.diff {
                diff.dispatch(event, &mut self.documents, &mut self.focus)?;
            }
            return self.drain_ui();
        }
        if !self.window_focused || self.closing {
            return Ok(());
        }
        let global = match event {
            InputEvent::Key(input) => {
                crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Global, *input)
            }
            _ => None,
        };
        if matches!(global, Some(input) if input.modifiers.ctrl && matches!(input.key, Key::Character('q')))
        {
            return self.request_quit();
        }
        if matches!(global,Some(input) if input.modifiers.ctrl && matches!(input.key,Key::Character('t')))
        {
            self.shell_requested = true;
            return self.drain_ui();
        }
        if let Some(dialog) = &mut self.rescan {
            dialog.dispatch(event);
            if dialog.finished {
                self.rescan = None;
            }
            return self.drain_ui();
        }
        if self.rest.sidebar.has_modal() {
            self.rest
                .sidebar
                .dispatch(event, &mut self.rest.model, &mut self.focus)?;
            return self.drain_ui();
        }
        if self.rest.form.as_ref().is_some_and(|form| form.has_modal()) {
            self.dispatch_rest_editor(event)?;
            return self.drain_ui();
        }
        if self.database.has_modal() {
            self.database.dispatch(event, &mut self.focus)?;
            return self.drain_ui();
        }
        if self.dispatch_sql_modal(event)? {
            return self.drain_ui();
        }
        if let Some((_, picker)) = &mut self.picker {
            picker.dispatch(event, &mut self.focus)?;
            return self.drain_ui();
        }
        if let Some(dialog) = &mut self.search {
            dialog.dispatch(event, &mut self.documents, &mut self.focus)?;
            let finished = dialog.finished;
            self.drain_ui()?;
            if finished {
                self.search = None;
            }
            return Ok(());
        }
        if self.files.has_modal() {
            self.files.dispatch(event, &mut self.focus)?;
            return self.drain_ui();
        }
        if self.git.has_modal() {
            self.git
                .dispatch(event, &mut self.documents, &mut self.focus)?;
            return self.drain_ui();
        }
        if matches!(global, Some(input) if input.modifiers.ctrl && (input.modifiers.alt || input.modifiers.shift) && matches!(input.key, Key::Character('f')))
        {
            if self.git.checkout_pending() {
                self.notice = "Wait for branch checkout before opening project search".into();
                return Ok(());
            }
            let (query, filter) = if let Some(view) = &self.editor {
                let document = self.documents.document(view.document_id())?;
                let selected = view.input.state.caret.selected_text(&document.snapshot())?;
                let path = match document.origin() {
                    kode_core::DocumentOrigin::File(path) => Some(path.as_path()),
                    _ => None,
                };
                ProjectSearchDialog::prefill(&selected, path)
            } else {
                (String::new(), String::new())
            };
            self.search = Some(ProjectSearchDialog::new(
                self.project.clone(),
                query,
                filter,
                self.detector.clone(),
                self.clipboard.clone(),
                self.clock.clone(),
                &self.documents,
                &mut self.focus,
            )?);
            return self.drain_ui();
        }
        if let Some(input) = global {
            if input.modifiers.alt
                && let Key::Character(number @ '1'..='8') = input.key
            {
                if number != '5' {
                    self.clear_diff()?;
                    self.git.deactivate(&mut self.documents, &mut self.focus)?;
                }
                self.tabs.selected = super::panel::SHORTCUTS[(number as u8 - b'1') as usize];
                if self.tabs.selected != crate::app::panel::REST {
                    self.rest.sidebar.dismiss(&mut self.focus);
                }
                if self.tabs.selected == crate::app::panel::DATABASE {
                    self.database.initialize()?;
                }
                self.focus_panel();
                return Ok(());
            }
            if input.key == Key::Function(7)
                && (self.rest.form.is_some() || self.tabs.selected == crate::app::panel::REST)
            {
                self.show_rest_runner()?;
                return self.drain_ui();
            }
            if input.key == Key::Function(6) {
                if self
                    .viewer_id()
                    .is_some_and(|id| self.focus.focused == Some(id))
                    || (self.rest.runner.visible && self.focus.focused == Some(self.rest.runner.id))
                    || (self.rest.form.is_some()
                        && (self
                            .editor
                            .as_ref()
                            .is_some_and(|view| self.focus.focused == Some(view.input.id))
                            || self
                                .rest
                                .response
                                .editor
                                .as_ref()
                                .is_some_and(|view| self.focus.focused == Some(view.input.id))))
                {
                    self.focus_panel();
                } else {
                    self.git.deactivate(&mut self.documents, &mut self.focus)?;
                    self.focus.focused = self.viewer_id();
                }
                return Ok(());
            }
            if self.splash
                && input.key == Key::Enter
                && self.focus.focused != Some(self.settings.focus_id(self.lsp_settings.id))
                && !self.git.contains_focus(&self.focus)
            {
                self.splash = false;
                return Ok(());
            }
        }
        let popup = self.editor.as_ref().is_some_and(|view| {
            !view.completion.entries.is_empty()
                || view.navigation.definition.is_some()
                || view.navigation.usages.is_some()
        });
        if (self.git.has_popup() || self.git.has_capture(&self.focus))
            && self
                .git
                .dispatch(event, &mut self.documents, &mut self.focus)?
        {
            return self.drain_ui();
        }
        let captured = self
            .editor
            .as_ref()
            .is_some_and(|view| view.input.is_dragging(&self.focus));
        if (popup || captured)
            && let Some(view) = &mut self.editor
            && view.dispatch(event, &mut self.documents, &mut self.focus, editor_area)?
        {
            return self.drain_ui();
        }
        if let Some(path) = &self.active_hex
            && let Some(view) = self.hexes.get_mut(path)
            && view.is_dragging(&self.focus)
            && view.dispatch(event, &mut self.focus)
        {
            return Ok(());
        }
        if let Some(view) = &mut self.image
            && view.is_dragging(&self.focus)
            && view.dispatch(event, &mut self.focus)
        {
            return Ok(());
        }
        if self.rest.sidebar.is_dragging()
            && self
                .rest
                .sidebar
                .dispatch(event, &mut self.rest.model, &mut self.focus)?
        {
            return self.drain_ui();
        }
        if self.dispatch_rest_editor(event)? {
            return self.drain_ui();
        }
        if self.dispatch_sql(event)? {
            return self.drain_ui();
        }
        if self.splitter.handle(event, self.geometry.body) {
            return Ok(());
        }
        if let InputEvent::Pointer(pointer) = event {
            let position = Position::new(pointer.x, pointer.y);
            self.focus.hovered = if self.geometry.viewer.contains(position) {
                self.viewer_id()
            } else if self.geometry.tabs.contains(position) {
                Some(self.tabs_id)
            } else {
                Some(if self.tabs.selected == crate::app::panel::GIT {
                    self.git.focus_id()
                } else if self.tabs.selected == crate::app::panel::SETTINGS {
                    self.settings.focus_id(self.lsp_settings.id)
                } else if self.tabs.selected == crate::app::panel::FILES {
                    self.files.id
                } else if self.tabs.selected == crate::app::panel::DATABASE {
                    self.database.tree.id
                } else if self.tabs.selected == crate::app::panel::REST {
                    self.rest.sidebar.id
                } else if self.tabs.selected == crate::app::panel::ABOUT {
                    self.about.focus_id()
                } else if self.tabs.selected == crate::app::panel::HELP {
                    self.help.id
                } else {
                    self.project_panel.id
                })
            };
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                if self.geometry.tabs.contains(position) {
                    self.focus.focused = Some(self.tabs_id);
                } else if self.geometry.viewer.contains(position) {
                    self.focus.focused = self.viewer_id();
                    self.splash = false;
                } else if self.geometry.panel.contains(position) {
                    self.focus_panel();
                }
            }
        }
        if self
            .tabs
            .handle(event, self.focus.focused == Some(self.tabs_id))
        {
            if self.tabs.selected != crate::app::panel::REST {
                self.rest.sidebar.dismiss(&mut self.focus);
            }
            if self.tabs.selected == crate::app::panel::DATABASE {
                self.database.initialize()?;
            }
            if self.tabs.selected != crate::app::panel::GIT {
                self.clear_diff()?;
                self.git.deactivate(&mut self.documents, &mut self.focus)?;
            }
            if matches!(event,InputEvent::Pointer(pointer)if pointer.action==PointerAction::Down(MouseButton::Left))
            {
                self.focus_panel();
            }
            return Ok(());
        }
        let in_panel = match event {
            InputEvent::Pointer(pointer) => self
                .geometry
                .panel
                .contains(Position::new(pointer.x, pointer.y)),
            _ => {
                if self.tabs.selected == crate::app::panel::GIT {
                    self.git.contains_focus(&self.focus)
                } else {
                    self.focus.focused
                        == Some(if self.tabs.selected == crate::app::panel::SETTINGS {
                            self.settings.focus_id(self.lsp_settings.id)
                        } else if self.tabs.selected == crate::app::panel::FILES {
                            self.files.id
                        } else if self.tabs.selected == crate::app::panel::DATABASE {
                            self.database.tree.id
                        } else if self.tabs.selected == crate::app::panel::REST {
                            self.rest.sidebar.id
                        } else if self.tabs.selected == crate::app::panel::ABOUT {
                            self.about.focus_id()
                        } else if self.tabs.selected == crate::app::panel::HELP {
                            self.help.id
                        } else {
                            self.project_panel.id
                        })
                }
            }
        };
        if in_panel {
            if self.tabs.selected == crate::app::panel::FILES {
                self.files.dispatch(event, &mut self.focus)?;
            } else if self.tabs.selected == crate::app::panel::PROJECT {
                self.project_panel
                    .dispatch(event, &self.project, &mut self.focus);
            } else if self.tabs.selected == crate::app::panel::HELP {
                self.help.dispatch(event, &mut self.focus);
            } else if self.tabs.selected == crate::app::panel::SETTINGS {
                if !self
                    .settings
                    .dispatch_navigation(event, &mut self.focus, self.lsp_settings.id)
                {
                    if self.settings.index_selected {
                        self.settings.native.dispatch(event, &mut self.focus);
                    } else {
                        self.lsp_settings.dispatch(event, &mut self.focus);
                    }
                }
            } else if self.tabs.selected == crate::app::panel::GIT {
                self.git
                    .dispatch(event, &mut self.documents, &mut self.focus)?;
            } else if self.tabs.selected == crate::app::panel::DATABASE {
                self.database.dispatch(event, &mut self.focus)?;
            } else if self.tabs.selected == crate::app::panel::REST {
                if self.rest.persistence_enabled {
                    self.rest
                        .sidebar
                        .dispatch(event, &mut self.rest.model, &mut self.focus)?;
                } else if let Some(warning) = &self.rest.warning {
                    self.notice = warning.clone();
                }
            } else if self.tabs.selected == crate::app::panel::ABOUT {
                self.about.dispatch(event, &mut self.focus);
            }
        } else if let Some(diff) = &mut self.diff {
            diff.dispatch(event, &mut self.documents, &mut self.focus)?;
        } else if let Some(view) = &mut self.editor {
            view.dispatch(event, &mut self.documents, &mut self.focus, editor_area)?;
        } else if let Some(view) = &mut self.image {
            view.dispatch(event, &mut self.focus);
        } else if let Some(path) = &self.active_hex
            && let Some(view) = self.hexes.get_mut(path)
        {
            view.dispatch(event, &mut self.focus);
        }
        self.drain_ui()
    }
    pub fn focus_panel(&mut self) {
        self.focus.focused = Some(match self.tabs.selected {
            crate::app::panel::FILES => self.files.id,
            crate::app::panel::PROJECT => self.project_panel.id,
            crate::app::panel::HELP => self.help.id,
            crate::app::panel::SETTINGS => self.settings.focus_id(self.lsp_settings.id),
            crate::app::panel::GIT => self.git.focus_id(),
            crate::app::panel::DATABASE => self.database.tree.id,
            crate::app::panel::REST => self.rest.sidebar.id,
            crate::app::panel::ABOUT => self.about.focus_id(),
            _ => self.tabs_id,
        });
        if let Some(view) = &mut self.editor {
            view.intelligence_changed();
        }
    }
    pub fn tick(&mut self) -> Result<()> {
        let editor_area = self.rest_editor_area();
        if !self.closing {
            self.git.tick(
                &self.documents,
                &self.focus,
                self.tabs.selected == crate::app::panel::GIT,
            )?;
            if let Some(diff) = &mut self.diff {
                let area = diff.body();
                if let Some(view) = &mut diff.editor {
                    view.tick(&self.documents, &self.focus, area)?;
                }
            }
            if let Some(watcher) = &self.watcher {
                self.files.poll_refresh(self.clock.now_millis(), watcher)?;
            }
            if let Some(view) = &mut self.editor {
                view.tick(&self.documents, &self.focus, editor_area)?;
            }
            if self.rest_response_visible()
                && self.rest.response.installed.is_some()
                && let Some(view) = &mut self.rest.response.editor
            {
                view.tick(&self.documents, &self.focus, self.rest.response.body)?;
            }
            if let Some(dialog) = &mut self.search {
                dialog.tick(&self.documents, &self.focus)?;
            }
        }
        self.drain_ui()?;
        self.request_capture()?;
        if self.sequence == self.captured_sequence
            && self.pending_capture.is_none()
            && let Some(job) = self
                .persistence
                .begin(self.clock.now_millis(), self.closing)?
        {
            self.work.push(AppWork::Persist(job));
        }
        Ok(())
    }
}
