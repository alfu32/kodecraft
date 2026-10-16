use super::state::{AppState, AppWork};
use crate::{
    editor::{
        display::display_text,
        host::{EditorView, QueuedEditorHost},
    },
    input::terminal::{InputEvent, Key, PointerAction},
    layout::{Axis, split},
    panels::rest::{
        editor::{
            jobs,
            response::{self, Proof, Source, Tab},
        },
        state::Work,
    },
    theme::Theme,
    widgets::controls::draw_text,
};
use crossterm::event::MouseButton;
use kode_core::Result;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};
impl AppState {
    fn rest_response_source(&self) -> Option<Source> {
        if self.rest_runner_visible() {
            return None;
        }
        let key = &self.rest.form.as_ref().filter(|form| !form.closed)?.key;
        if self.rest.run.failures.contains_key(key) {
            return None;
        }
        if let Some(live) = self
            .rest
            .run
            .live
            .as_ref()
            .filter(|live| live.key.item_id == *key)
        {
            return Some(Source::Live(live.clone()));
        }
        self.rest
            .run
            .responses
            .iter()
            .rev()
            .find(|response| response.key.item_id == *key)
            .cloned()
            .map(Source::Final)
    }
    pub(super) fn rest_response_visible(&self) -> bool {
        self.rest_response_source().is_some()
    }
    pub(super) fn rest_response_request_area(&self, area: Rect) -> Rect {
        if (self.rest_response_visible() || self.rest_runner_visible()) && area.height >= 12 {
            split(
                area,
                Axis::Vertical,
                area.height
                    .saturating_sub(
                        (self.rest.model.ui().response_panel_height.clamp(5, 60) as u16)
                            .min(area.height.saturating_sub(8)),
                    )
                    .saturating_sub(1),
            )
            .first
        } else {
            area
        }
    }
    pub(super) fn drain_rest_response(&mut self) -> Result<()> {
        let Some(source) = self.rest_response_source() else {
            return Ok(());
        };
        let proof = Proof {
            key: source.key().item_id.clone(),
            revision: self.rest.run.display_revision,
            tab: self.rest.response.tab,
            pretty: self.rest.response.pretty,
        };
        // One display job at a time. Publish intermediate snapshots during a live
        // stream, then prepare its newest captured version on the following frame.
        if self.rest.response.pending.is_none()
            && self.rest.response.ready.is_none()
            && self.rest.response.requested.as_ref() != Some(&proof)
        {
            let tag = self.tag()?;
            let input = response::Input {
                source,
                root: self.project.root.clone(),
                tab: proof.tab,
                pretty: proof.pretty,
                cookies: self.rest.run.client.cookies(),
            };
            self.rest.response.pending = Some(tag);
            self.rest.response.requested = Some(proof.clone());
            self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
                jobs::Job::Response {
                    tag,
                    proof,
                    input: Box::new(input),
                },
            )))));
        }
        if self.rest_response_modal() {
            return Ok(());
        }
        if let Some((proof, prepared)) = self.rest.response.ready.take() {
            if self
                .rest
                .form
                .as_ref()
                .is_none_or(|form| form.key != proof.key || form.closed)
                || proof.tab != self.rest.response.tab
                || proof.pretty != self.rest.response.pretty
            {
                return Ok(());
            }
            self.rest.response.cache_view();
            let id = self.documents.insert(
                prepared.document,
                &prepared.descriptor,
                self.generation,
                None,
            )?;
            let mut view = EditorView::new(
                id,
                &self.documents,
                QueuedEditorHost::new(prepared.descriptor),
                self.generation,
                self.clipboard.clone(),
                self.clock.clone(),
            )?;
            if let Some((caret, scroll)) =
                self.rest
                    .response
                    .views
                    .get(&(proof.key.clone(), proof.tab, proof.pretty))
            {
                view.input.state.caret = caret.clone();
                view.input.state.scroll_rows = *scroll;
            }
            if let Some(mut old) = self.rest.response.editor.take() {
                let focused = self.focus.focused == Some(old.input.id);
                old.detach(&mut self.focus);
                let owner = old.owner();
                self.work.extend(
                    old.take_work()
                        .into_iter()
                        .map(|work| AppWork::Editor { owner, work }),
                );
                self.documents.remove(old.document_id())?;
                if focused {
                    self.focus.focused = Some(view.input.id);
                }
            }
            view.synchronize(&self.documents)?;
            let owner = view.owner();
            self.work.extend(
                view.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
            self.rest.response.editor = Some(view);
            self.rest.response.installed = Some(proof);
            self.rest.response.status = prepared.status;
        }
        if let Some(view) = &mut self.rest.response.editor {
            let owner = view.owner();
            self.work.extend(
                view.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
            let _ = view.host.take_events();
        }
        Ok(())
    }
    pub(super) fn rest_response_modal(&self) -> bool {
        self.rest.sidebar.has_modal()
            || self.rest.form.as_ref().is_some_and(|form| form.has_modal())
            || self.files.has_modal()
            || self.database.has_modal()
            || self.sql.dialog.is_some()
            || self.sql.menu.is_some()
            || self.search.is_some()
            || self.picker.is_some()
            || self.path_picker.is_some()
            || self.rescan.is_some()
            || self.git.has_modal()
    }
    pub(super) fn dispatch_rest_response(&mut self, event: &InputEvent) -> Result<bool> {
        if !self.rest_response_visible() || self.rest_response_modal() {
            return Ok(false);
        }
        let area = self.geometry.viewer;
        if self.rest.response.splitter.handle(event, area) {
            self.rest.response.splitter.size = self
                .rest
                .response
                .splitter
                .size
                .clamp(7, area.height.saturating_sub(6).max(7));
            let height =
                area.height
                    .saturating_sub(self.rest.response.splitter.size + 1) as i32;
            if height != self.rest.model.ui().response_panel_height {
                self.rest.model.set_response_panel_height(height);
            }
            return Ok(true);
        }
        let panel = &mut self.rest.response;
        let focused = panel
            .editor
            .as_ref()
            .is_some_and(|view| self.focus.focused == Some(view.input.id));
        let mut selected = None;
        if let InputEvent::Key(input) = event
            && focused
            && input.key == Key::Tab
            && (input.modifiers.ctrl || input.modifiers.alt)
        {
            let index = Tab::ALL
                .iter()
                .position(|tab| *tab == panel.tab)
                .unwrap_or(0);
            selected = Some(Tab::ALL[(index + if input.modifiers.shift { 4 } else { 1 }) % 5]);
        }
        if let InputEvent::Pointer(pointer) = event
            && pointer.action == PointerAction::Down(MouseButton::Left)
        {
            let position = Position::new(pointer.x, pointer.y);
            if let Some((tab, _)) = panel.tabs.iter().find(|(_, area)| area.contains(position)) {
                selected = Some(*tab);
            }
            if panel.pretty_button.contains(position) {
                panel.pretty = !panel.pretty;
                self.rest.model.set_pretty_print_responses(panel.pretty);
                panel.requested = None;
                panel.pending = None;
                panel.ready = None;
                return Ok(true);
            }
            if panel.clear_button.contains(position) {
                self.rest.run.client.clear_cookies();
                panel.requested = None;
                panel.pending = None;
                panel.ready = None;
                return Ok(true);
            }
            if panel.body.contains(position)
                && panel.installed.is_some()
                && let Some(view) = &panel.editor
            {
                self.focus.focused = Some(view.input.id);
            }
        }
        if let Some(tab) = selected {
            panel.tab = tab;
            panel.requested = None;
            panel.pending = None;
            panel.ready = None;
            return Ok(true);
        }
        if panel.installed.as_ref().is_some_and(|proof| {
            self.rest.form.as_ref().is_some_and(|form| {
                form.key == proof.key && proof.tab == panel.tab && proof.pretty == panel.pretty
            })
        }) && let Some(view) = &mut panel.editor
        {
            let pointer = matches!(event,InputEvent::Pointer(pointer) if panel.body.contains(Position::new(pointer.x,pointer.y)));
            if focused || pointer || view.input.is_dragging(&self.focus) {
                return view.dispatch(event, &mut self.documents, &mut self.focus, panel.body);
            }
        }
        Ok(false)
    }
    pub(super) fn render_rest_response(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Result<Option<Position>> {
        if !self.rest_response_visible() || area.height < 12 {
            self.rest.response.area = Rect::default();
            self.rest.response.body = Rect::default();
            return Ok(None);
        }
        let panes = split(
            area,
            Axis::Vertical,
            self.rest_response_request_area(area).height,
        );
        let panel = &mut self.rest.response;
        panel.splitter.size = panes.first.height;
        panel.area = panes.second;
        Clear.render(panel.area, buffer);
        panel.splitter.render(area, buffer, theme);
        draw_text(
            Rect::new(panel.area.x, panel.area.y, panel.area.width, 1),
            buffer,
            &display_text(
                if panel.installed.as_ref().is_some_and(|proof| {
                    self.rest
                        .form
                        .as_ref()
                        .is_some_and(|form| form.key == proof.key)
                }) {
                    &panel.status
                } else {
                    "Preparing response display…"
                },
            ),
            theme.style("content"),
        );
        panel.tabs.clear();
        panel.pretty_button = Rect::default();
        panel.clear_button = Rect::default();
        let mut x = panel.area.x;
        let mut y = panel.area.y.saturating_add(1);
        for (label, tab) in Tab::ALL
            .into_iter()
            .map(|tab| (tab.name(), Some(tab)))
            .chain([("Pretty", None), ("Clear cookies", None)])
        {
            let width = (label.len() as u16).min(panel.area.width);
            if x > panel.area.x && x + width > panel.area.right() {
                x = panel.area.x;
                y = y.saturating_add(1);
            }
            if y >= panel.area.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            let active = tab.map_or(panel.pretty && label == "Pretty", |tab| tab == panel.tab);
            draw_text(
                rect,
                buffer,
                label,
                theme.style(if active {
                    "file-entry:selected"
                } else {
                    "lsp-button"
                }),
            );
            if let Some(tab) = tab {
                panel.tabs.push((tab, rect));
            } else if label == "Pretty" {
                panel.pretty_button = rect;
            } else {
                panel.clear_button = rect;
            }
            x = x.saturating_add(width + 1);
        }
        let top = y.saturating_add(1).min(panel.area.bottom());
        panel.body = Rect::new(
            panel.area.x,
            top,
            panel.area.width,
            panel.area.bottom() - top,
        );
        if let Some(view) = &mut panel.editor {
            // While another request's preparation is pending, do not expose its
            // previous response under the new request's controls.
            if panel.installed.as_ref().is_some_and(|proof| {
                self.rest.form.as_ref().is_some_and(|form| {
                    form.key == proof.key && proof.tab == panel.tab && proof.pretty == panel.pretty
                })
            }) {
                view.synchronize(&self.documents)?;
                return view.render(&self.documents, &self.focus, panel.body, buffer, theme, &[]);
            }
        }
        Ok(None)
    }
}
