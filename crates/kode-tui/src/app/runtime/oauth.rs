use super::*;
use crate::panels::rest::{oauth::Running, state::Reply};
use kode_platform::jobs::CancellationToken;
use kode_rest::oauth::{self, BoundTokens, Config};
impl AppRuntime {
    pub(super) fn oauth_prepared(
        &mut self,
        tag: JobTag,
        key: String,
        result: Result<Reply>,
    ) -> Result<()> {
        let state = &mut self.state.rest.oauth;
        if state
            .pending
            .as_ref()
            .is_none_or(|p| p.tag != tag || p.key != key)
        {
            return Ok(());
        }
        let refresh = state.pending.take().expect("matched pending flow").refresh;
        if self.state.closing || !self.state.rest.run.live_keys.contains(&key) {
            return Ok(());
        }
        let config = match result {
            Ok(Reply::OAuth(config)) => *config,
            Err(error) => {
                state.message(&key, error.to_string());
                return Ok(());
            }
            _ => return Ok(()),
        };
        let previous = if refresh {
            match state
                .tokens
                .get(&key)
                .filter(|bound| bound.config == config && bound.tokens.can_refresh())
            {
                Some(bound) => Some(bound.tokens.clone()),
                None => {
                    state.message(&key,"Refresh requires a retained refresh token and unchanged OAuth settings; acquire a token explicitly".into());
                    return Ok(());
                }
            }
        } else {
            None
        };
        let (redirects, progress, channels) = oauth::channels();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let captured = config.clone();
        let submitted=self.jobs.submit(self.root_owner,tag,Delivery::Cancellable,move |scheduler| async move {
            let _guard=token.clone().drop_guard();
            let future=async {if let Some(previous)=previous {oauth::refresh(captured,previous,token.clone()).await}else {oauth::acquire(captured,channels,token.clone()).await}};
            tokio::select! {biased;_=scheduler.cancelled()=>{token.cancel();Err(DomainError::Cancelled)},result=future=>result.map(Output::OAuth)}
        });
        if let Err(error) = submitted {
            state.message(&key, error.to_string());
            return Ok(());
        }
        state.running = Some(Running {
            tag,
            key: key.clone(),
            config: config.clone(),
            cancel,
            redirects,
            progress,
        });
        self.contexts.insert(
            tag,
            Context::OAuth {
                key,
                config: Box::new(config),
            },
        );
        self.state.sync_rest_oauth();
        self.notifications.repaint();
        Ok(())
    }
    pub(super) fn oauth_complete(
        &mut self,
        tag: JobTag,
        key: String,
        config: Config,
        result: Result<Output>,
    ) -> Result<()> {
        let state = &mut self.state.rest.oauth;
        if state
            .running
            .as_ref()
            .is_none_or(|r| r.tag != tag || r.key != key)
        {
            return Ok(());
        }
        state.running = None;
        if self.state.closing || !self.state.rest.run.live_keys.contains(&key) {
            return Ok(());
        }
        match result {
            Ok(Output::OAuth(tokens)) => {
                match state.store(key.clone(), BoundTokens { config, tokens }) {
                    Ok(()) => {
                        state.messages.remove(&key);
                    }
                    Err(error) => state.message(&key, error.to_string()),
                }
            }
            Err(error) => state.message(&key, error.to_string()),
            _ => {}
        }
        self.state.sync_rest_oauth();
        self.notifications.repaint();
        Ok(())
    }
    pub(super) fn pump_oauth(&mut self) {
        if let Some(running) = &mut self.state.rest.oauth.running
            && running.progress.has_changed().unwrap_or(false)
        {
            running.progress.borrow_and_update();
            self.notifications.repaint();
        }
        self.state.sync_rest_oauth();
    }
}
