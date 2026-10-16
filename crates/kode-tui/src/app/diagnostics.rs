use super::state::{AppState, AppWork};
use crate::{
    editor::host::ClipboardJob,
    input::terminal::InputEvent,
    widgets::diagnostic::{Action, Diagnostic},
};
use kode_core::Result;
impl AppState {
    pub(super) fn show_diagnostic(&mut self, title: String, text: String) -> Result<()> {
        let dialog = Diagnostic::new(title, text, self.focus.focused)?;
        self.focus.focused = Some(dialog.panel.id);
        self.focus.captured = None;
        self.diagnostic = Some(dialog);
        Ok(())
    }
    pub(super) fn dispatch_diagnostic(&mut self, event: &InputEvent) -> Result<bool> {
        let Some(dialog) = &mut self.diagnostic else {
            return Ok(false);
        };
        // A completed parent operation may replace its modal while details stay open.
        if self.focus.focused != Some(dialog.panel.id) {
            dialog.previous = self.focus.focused;
        }
        match dialog.dispatch(event, &mut self.focus) {
            Some(Action::Dismiss) => {
                let dialog = self.diagnostic.take().expect("active diagnostic");
                self.focus.focused = dialog.previous;
            }
            Some(Action::Copy) => {
                let job = ClipboardJob::Publish(self.clipboard.remember(&dialog.text)?);
                let tag = self.tag()?;
                self.work.push(AppWork::DatabaseClipboard {
                    tag,
                    job,
                    proof: None,
                });
            }
            None => {}
        }
        Ok(true)
    }
}
