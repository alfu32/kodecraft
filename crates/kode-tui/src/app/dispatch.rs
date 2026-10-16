use super::state::{AppState, AppWork};
use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    panels::project_search::ProjectSearchDialog,
};
use crossterm::event::MouseButton;
use kode_core::Result;
use ratatui::layout::{Position, Rect};

impl AppState {
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
        if let InputEvent::Focus(active) = event {
            self.window_focused = *active;
            self.rest
                .sidebar
                .dispatch(event, &mut self.rest.model, &mut self.focus)?;
            if !active {
                self.focus.captured = None;
            }
            if let Some(view) = &mut self.editor {
                view.dispatch(
                    event,
                    &mut self.documents,
                    &mut self.focus,
                    self.geometry.viewer,
                )?;
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
        if matches!(event, InputEvent::Key(input) if input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta && matches!(input.key, Key::Character('q' | 'Q')))
        {
            self.quit_requested = true;
            self.closing = true;
            self.pending_open = None;
            self.dismiss_sql_dialog()?;
            self.rest.sidebar.dismiss(&mut self.focus);
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
        if matches!(event, InputEvent::Key(input) if input.modifiers.ctrl && input.modifiers.shift && !input.modifiers.alt && matches!(input.key, Key::Character('f' | 'F')))
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
        if let InputEvent::Key(input) = event {
            if input.modifiers.alt
                && let Key::Character(number @ '1'..='7') = input.key
            {
                if number != '5' {
                    self.clear_diff()?;
                    self.git.deactivate(&mut self.documents, &mut self.focus)?;
                }
                self.tabs.selected = (number as u8 - b'1') as usize;
                if self.tabs.selected != 6 {
                    self.rest.sidebar.dismiss(&mut self.focus);
                }
                if self.tabs.selected == 5 {
                    self.database.initialize()?;
                }
                self.focus_panel();
                return Ok(());
            }
            if input.key == Key::Function(6) {
                if self
                    .viewer_id()
                    .is_some_and(|id| self.focus.focused == Some(id))
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
                && self.focus.focused != Some(self.lsp_settings.id)
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
            && view.dispatch(
                event,
                &mut self.documents,
                &mut self.focus,
                self.geometry.viewer,
            )?
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
                Some(if self.tabs.selected == 4 {
                    self.git.focus_id()
                } else if self.tabs.selected == 3 {
                    self.lsp_settings.id
                } else if self.tabs.selected == 0 {
                    self.files.id
                } else if self.tabs.selected == 5 {
                    self.database.tree.id
                } else if self.tabs.selected == 6 {
                    self.rest.sidebar.id
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
            if self.tabs.selected != 6 {
                self.rest.sidebar.dismiss(&mut self.focus);
            }
            if self.tabs.selected == 5 {
                self.database.initialize()?;
            }
            if self.tabs.selected != 4 {
                self.clear_diff()?;
                self.git.deactivate(&mut self.documents, &mut self.focus)?;
            }
            return Ok(());
        }
        let in_panel = match event {
            InputEvent::Pointer(pointer) => self
                .geometry
                .panel
                .contains(Position::new(pointer.x, pointer.y)),
            _ => {
                if self.tabs.selected == 4 {
                    self.git.contains_focus(&self.focus)
                } else {
                    self.focus.focused
                        == Some(if self.tabs.selected == 3 {
                            self.lsp_settings.id
                        } else if self.tabs.selected == 0 {
                            self.files.id
                        } else if self.tabs.selected == 5 {
                            self.database.tree.id
                        } else if self.tabs.selected == 6 {
                            self.rest.sidebar.id
                        } else {
                            self.project_panel.id
                        })
                }
            }
        };
        if in_panel {
            if self.tabs.selected == 0 {
                self.files.dispatch(event, &mut self.focus)?;
            } else if self.tabs.selected == 1 {
                self.project_panel
                    .dispatch(event, &self.project, &mut self.focus);
            } else if self.tabs.selected == 3 {
                self.lsp_settings.dispatch(event, &mut self.focus);
            } else if self.tabs.selected == 4 {
                self.git
                    .dispatch(event, &mut self.documents, &mut self.focus)?;
            } else if self.tabs.selected == 5 {
                self.database.dispatch(event, &mut self.focus)?;
            } else if self.tabs.selected == 6 {
                if self.rest.persistence_enabled {
                    self.rest
                        .sidebar
                        .dispatch(event, &mut self.rest.model, &mut self.focus)?;
                } else if let Some(warning) = &self.rest.warning {
                    self.notice = warning.clone();
                }
            }
        } else if let Some(diff) = &mut self.diff {
            diff.dispatch(event, &mut self.documents, &mut self.focus)?;
        } else if let Some(view) = &mut self.editor {
            view.dispatch(
                event,
                &mut self.documents,
                &mut self.focus,
                self.geometry.viewer,
            )?;
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
            0 => self.files.id,
            1 => self.project_panel.id,
            3 => self.lsp_settings.id,
            4 => self.git.focus_id(),
            5 => self.database.tree.id,
            6 => self.rest.sidebar.id,
            _ => self.tabs_id,
        });
        if let Some(view) = &mut self.editor {
            view.intelligence_changed();
        }
    }
    pub fn tick(&mut self) -> Result<()> {
        if !self.closing {
            self.git
                .tick(&self.documents, &self.focus, self.tabs.selected == 4)?;
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
                view.tick(&self.documents, &self.focus, self.geometry.viewer)?;
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
