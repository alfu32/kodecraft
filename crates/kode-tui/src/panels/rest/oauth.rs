//! Runtime-only OAuth ownership; none of this enters RestState::capture.
use kode_core::{JobTag, Result};
use kode_platform::jobs::CancellationToken;
use kode_rest::oauth::{BoundTokens, Config, Progress};
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, watch};
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Acquire,
    Refresh,
    Cancel,
    Clear,
    CopyUrl,
    ViewUrl,
    PasteRedirect,
    PasteToken,
}
#[derive(Clone, Default, PartialEq, Eq)]
pub struct View {
    pub flow: Option<JobTag>,
    pub pending: bool,
    pub message: String,
    pub callback: Option<String>,
    pub url: Option<String>,
    pub code: Option<String>,
    pub has_token: bool,
    pub can_refresh: bool,
}
pub struct Pending {
    pub tag: JobTag,
    pub key: String,
    pub refresh: bool,
}
pub struct Running {
    pub tag: JobTag,
    pub key: String,
    pub config: Config,
    pub cancel: CancellationToken,
    pub redirects: mpsc::Sender<String>,
    pub progress: watch::Receiver<Progress>,
}
#[derive(Default)]
pub struct State {
    pub pending: Option<Pending>,
    pub running: Option<Running>,
    pub tokens: HashMap<String, BoundTokens>,
    pub messages: HashMap<String, String>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl State {
    pub fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.message(&pending.key, "OAuth authorization cancelled".into());
        }
        if let Some(running) = self.running.take() {
            running.cancel.cancel();
            self.message(&running.key, "OAuth authorization cancelled".into());
        }
    }
    pub fn message(&mut self, key: &str, text: String) {
        if self.messages.len() >= 64
            && !self.messages.contains_key(key)
            && let Some(old) = self.messages.keys().next().cloned()
        {
            self.messages.remove(&old);
        }
        self.messages.insert(key.into(), text);
    }
    pub fn retain(&mut self, keys: &HashSet<String>) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| !keys.contains(&p.key))
            || self
                .running
                .as_ref()
                .is_some_and(|r| !keys.contains(&r.key))
        {
            self.cancel();
        }
        self.tokens.retain(|key, _| keys.contains(key));
        self.messages.retain(|key, _| keys.contains(key));
    }
    pub fn store(&mut self, key: String, value: BoundTokens) -> Result<()> {
        if self.tokens.len() >= 32 && !self.tokens.contains_key(&key) {
            return Err(kode_core::DomainError::Unsupported("32 runtime OAuth tokens are retained; clear an unused token before acquiring another".into()));
        }
        self.tokens.insert(key, value);
        Ok(())
    }
    pub fn view(&self, key: &str) -> View {
        let mut view = View::default();
        if let Some(token) = self.tokens.get(key) {
            view.has_token = true;
            view.can_refresh = token.tokens.can_refresh();
            view.message = if token.tokens.expired() {
                "OAuth token expired; Refresh or Acquire explicitly".into()
            } else {
                match token.tokens.expires_in() {
                    Some(left) => {
                        format!("Runtime OAuth token ready; expires in {}s", left.as_secs())
                    }
                    None => "Runtime OAuth token ready; not persisted or exported".into(),
                }
            };
        }
        if let Some(message) = self.messages.get(key)
            && (!view.has_token || !message.is_empty())
        {
            view.message = message.clone();
        }
        if let Some(pending) = &self.pending
            && pending.key == key
        {
            view.flow = Some(pending.tag);
            view.pending = true;
            view.message = "Preparing captured OAuth settings".into();
        }
        if let Some(running) = &self.running
            && running.key == key
        {
            let progress = running.progress.borrow();
            view.flow = Some(running.tag);
            view.pending = true;
            view.message = progress.message.clone();
            view.callback = progress.callback_url.clone();
            view.url = progress.authorization_url.clone();
            view.code = progress.user_code.clone();
            if let Some(message) = self.messages.get(key) {
                view.message = message.clone();
            }
        }
        view
    }
}
pub struct Prepare {
    pub tag: JobTag,
    pub key: String,
    pub path: kode_rest::model::NodePath,
    pub capture: super::editor::jobs::ModelCapture,
}
impl Prepare {
    pub fn prepare(self) -> Result<Config> {
        let (model, root) = self.capture.model()?;
        let resolver = kode_rest::resolve::RequestResolver::new(
            model.collection(),
            self.path,
            &root,
            model.active_environment(),
        )?;
        Config::from_resolver(&resolver)
    }
}
