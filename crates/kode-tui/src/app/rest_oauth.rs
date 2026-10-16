use super::state::{AppState, AppWork};
use crate::panels::rest::{
    oauth::{Action, Pending, Prepare},
    state::Work,
};
use kode_core::{JobTag, Result};
impl AppState {
    pub(super) fn sync_rest_oauth(&mut self) {
        if let Some(form) = &mut self.rest.form {
            let view = self.rest.oauth.view(&form.key);
            form.set_oauth(view);
        }
    }
    pub(super) fn rest_oauth_action(&mut self, action: Action) -> Result<()> {
        let Some(form) = &self.rest.form else {
            return Ok(());
        };
        let key = form.key.clone();
        let path = form.path.clone();
        match action {
            Action::Acquire | Action::Refresh => {
                self.rest.oauth.cancel();
                let tag = self.tag()?;
                self.rest.oauth.pending = Some(Pending {
                    tag,
                    key: key.clone(),
                    refresh: action == Action::Refresh,
                });
                self.work
                    .push(AppWork::Rest(Box::new(Work::OAuth(Box::new(Prepare {
                        tag,
                        key,
                        path,
                        capture: self.rest_model_capture(),
                    })))));
            }
            Action::Cancel => self.rest.oauth.cancel(),
            Action::Clear => {
                if self
                    .rest
                    .oauth
                    .running
                    .as_ref()
                    .is_some_and(|r| r.key == key)
                    || self
                        .rest
                        .oauth
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.key == key)
                {
                    self.rest.oauth.cancel();
                }
                self.rest.oauth.tokens.remove(&key);
                self.rest
                    .oauth
                    .message(&key, "Runtime OAuth token cleared".into());
            }
            _ => {}
        }
        self.sync_rest_oauth();
        Ok(())
    }
    pub(super) fn rest_oauth_redirect(&mut self, flow: JobTag, text: String) {
        if let Some(running) = &self.rest.oauth.running
            && running.tag == flow
            && self
                .rest
                .form
                .as_ref()
                .is_some_and(|f| f.key == running.key)
        {
            let key = running.key.clone();
            if running.redirects.try_send(text).is_err() {
                self.rest.oauth.message(
                    &key,
                    "OAuth flow ended or redirect queue is full; try again".into(),
                );
            }
        }
    }
}
