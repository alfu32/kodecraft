use super::state::{AppState, AppWork};
use crate::panels::rest::{
    oauth::{Action, Pending, Prepare},
    state::Work,
};
use kode_core::{JobTag, Result};
use kode_rest::{
    oauth::{BoundTokens, Config, Grant, TokenSet},
    resolve::RequestResolver,
};
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
                self.rest.oauth.messages.remove(&key);
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

    pub(super) fn rest_oauth_access_token(&mut self, flow: Option<JobTag>, text: String) {
        let Some(form) = self.rest.form.as_ref() else {
            return;
        };
        let key = form.key.clone();
        if !self.rest.run.live_keys.contains(&key) {
            return;
        }
        let config = if let Some(flow) = flow {
            let Some(running) = self.rest.oauth.running.as_ref() else {
                return;
            };
            if running.tag != flow || running.key != key || running.config.grant == Grant::OAuth1 {
                return;
            }
            running.config.clone()
        } else {
            let path = form.path.clone();
            let captured = (|| -> Result<Config> {
                let (model, root) = self.rest_model_capture().model()?;
                let resolver = RequestResolver::new(
                    model.collection(),
                    path,
                    &root,
                    model.active_environment(),
                )?;
                Config::from_resolver(&resolver)
            })();
            match captured {
                Ok(config) if config.grant != Grant::OAuth1 => config,
                Ok(_) => return,
                Err(error) => {
                    self.rest.oauth.message(&key, error.to_string());
                    self.sync_rest_oauth();
                    return;
                }
            }
        };
        let tokens = match TokenSet::pasted_oauth2(text) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.rest.oauth.message(&key, error.to_string());
                self.sync_rest_oauth();
                return;
            }
        };
        self.rest.oauth.cancel();
        match self
            .rest
            .oauth
            .store(key.clone(), BoundTokens { config, tokens })
        {
            Ok(()) => {
                self.rest.oauth.messages.remove(&key);
            }
            Err(error) => self.rest.oauth.message(&key, error.to_string()),
        }
        self.sync_rest_oauth();
    }
}
