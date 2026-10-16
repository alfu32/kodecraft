use super::state::AppState;
use crate::{
    input::terminal::InputEvent,
    layout::{Axis, split},
    panels::rest::runner::Intent,
    theme::Theme,
};
use kode_core::Result;
use ratatui::{buffer::Buffer, layout::Rect};
impl AppState {
    pub(super) fn rest_runner_visible(&self) -> bool {
        self.rest.runner.visible && self.rest.form.as_ref().is_some_and(|form| !form.closed)
    }
    pub(super) fn show_rest_runner(&mut self) -> Result<()> {
        if self.rest.form.is_none() {
            let path = self
                .rest
                .run
                .scope
                .clone()
                .filter(|path| self.rest.model.node(path).is_some())
                .unwrap_or_default();
            self.open_rest_node(path)?;
            self.rest.runner.focus_after_open = true;
        } else {
            self.focus.focused = Some(self.rest.runner.id);
        }
        self.rest.runner.visible = true;
        Ok(())
    }
    pub(super) fn drain_rest_runner(&mut self) -> Result<()> {
        for intent in self.rest.runner.take_intents() {
            match intent {
                Intent::Dismiss => {
                    self.rest.runner.visible = false;
                    self.focus.focused = self.viewer_id();
                }
                Intent::Stop => {
                    self.rest.run.pending = None;
                    self.rest.run.cancel();
                    self.rest.runner.message =
                        "Stopping; completed responses remain available".into();
                }
                Intent::Rerun => {
                    if let Some(scope) = self.rest.run.scope.clone() {
                        self.run_rest_selection(scope)?;
                    } else {
                        self.rest.runner.message =
                            "Run a request or collection from the sidebar first".into();
                    }
                }
                Intent::Open(key) => {
                    if let Some(path) = self.rest.path_for_key(&key) {
                        self.open_rest_node(path)?;
                    } else {
                        self.rest.runner.message =
                            "This request was removed; no current request was opened".into();
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn dispatch_rest_runner(&mut self, event: &InputEvent) -> Result<bool> {
        if !self.rest_runner_visible() || self.rest_response_modal() {
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
        Ok(self
            .rest
            .runner
            .dispatch(event, &self.rest.run, &mut self.focus))
    }
    pub(super) fn render_rest_runner(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if !self.rest_runner_visible() || area.height < 12 {
            self.rest.runner.area = Rect::default();
            return;
        }
        let panes = split(
            area,
            Axis::Vertical,
            self.rest_response_request_area(area).height,
        );
        self.rest.response.splitter.size = panes.first.height;
        self.rest.response.splitter.render(area, buffer, theme);
        self.rest.runner.render(
            panes.second,
            buffer,
            theme,
            &self.rest.run,
            self.focus.focused == Some(self.rest.runner.id),
        );
    }
}
