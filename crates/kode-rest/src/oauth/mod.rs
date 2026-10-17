//! Cancellable OAuth acquisition. Runtime tokens never implement serialization.
mod callback;
mod client;
mod oauth1;
mod oauth2_flow;
mod settings;
use crate::auth::ExecutionAuth;
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
pub use settings::{BoundTokens, configuration_fields};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    AuthorizationCode,
    ClientCredentials,
    DeviceCode,
    OAuth1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuthentication {
    Basic,
    Body,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub grant: Grant,
    pub authorization_url: String,
    pub token_url: String,
    pub request_token_url: String,
    pub device_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    /// Port zero chooses a fresh loopback port. Non-loopback URIs use explicit
    /// pasted redirects, so a remote GUI browser need not reach this process.
    pub redirect_url: String,
    pub client_authentication: ClientAuthentication,
    pub timeout: Duration,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            grant: Grant::AuthorizationCode,
            authorization_url: String::new(),
            token_url: String::new(),
            request_token_url: String::new(),
            device_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scopes: vec![],
            redirect_url: "https://localhost:0/callback".into(),
            client_authentication: ClientAuthentication::Basic,
            timeout: Duration::from_secs(300),
        }
    }
}
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("grant", &self.grant)
            .finish_non_exhaustive()
    }
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.client_id.is_empty() {
            return Err(invalid("OAuth requires a client ID"));
        }
        if self.timeout.is_zero() || self.timeout > Duration::from_secs(1800) {
            return Err(invalid(
                "OAuth timeout must be positive and at most 30 minutes",
            ));
        }
        if self.client_id.len() > 64 * 1024
            || self.client_secret.len() > 64 * 1024
            || self.scopes.len() > 128
            || self.scopes.iter().any(|s| s.len() > 4096)
        {
            return Err(invalid("OAuth settings exceed their size limits"));
        }
        endpoint(&self.token_url)?;
        match self.grant {
            Grant::AuthorizationCode => {
                endpoint(&self.authorization_url)?;
                callback::validate_uri(&self.redirect_url)?;
            }
            Grant::OAuth1 => {
                endpoint(&self.authorization_url)?;
                endpoint(&self.request_token_url)?;
                callback::validate_uri(&self.redirect_url)?;
            }
            Grant::DeviceCode => {
                endpoint(&self.device_url)?;
            }
            Grant::ClientCredentials => {}
        }
        Ok(())
    }
}
#[derive(Clone)]
pub struct TokenSet {
    pub(crate) kind: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_secret: Option<String>,
    pub(crate) prefix: String,
    pub(crate) expires_at: Option<Instant>,
    pub scopes: Vec<String>,
}
impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("kind", &self.kind)
            .field("expired", &self.expired())
            .finish_non_exhaustive()
    }
}
impl TokenSet {
    /// Accept an access token supplied by the user after browser authorization.
    /// The token is kept only in the runtime owner and never serialized.
    pub fn pasted_oauth2(value: String) -> Result<Self> {
        let token = value.trim();
        if token.is_empty()
            || token.len() > 64 * 1024
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(invalid(
                "Paste one non-empty OAuth 2.0 access token without spaces or control characters",
            ));
        }
        Ok(Self {
            kind: "oauth2".into(),
            access_token: token.into(),
            refresh_token: None,
            token_secret: None,
            prefix: "Bearer".into(),
            expires_at: None,
            scopes: vec![],
        })
    }
    pub fn expired(&self) -> bool {
        self.expires_at.is_some_and(|at| Instant::now() >= at)
    }
    pub fn expires_in(&self) -> Option<Duration> {
        self.expires_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
    }
    /// The caller verifies the captured request owner before applying this
    /// ephemeral override. It must never be merged into collection/export data.
    pub fn apply(&self, auth: &mut ExecutionAuth) -> Result<()> {
        if self.expired() {
            return Err(invalid(
                "OAuth token expired; refresh or acquire a token explicitly",
            ));
        }
        if auth.kind != self.kind {
            return Err(invalid(
                "OAuth token does not match the selected authentication type",
            ));
        }
        auth.runtime_token = true;
        if self.kind == "oauth2" {
            auth.fields
                .insert("accessToken".into(), self.access_token.clone());
            auth.fields
                .insert("headerPrefix".into(), self.prefix.clone());
        } else {
            auth.fields
                .insert("token".into(), self.access_token.clone());
            auth.fields.insert(
                "tokenSecret".into(),
                self.token_secret.clone().unwrap_or_default(),
            );
        }
        Ok(())
    }
    pub fn execution_auth(&self) -> Result<ExecutionAuth> {
        let mut auth = ExecutionAuth {
            runtime_token: false,
            kind: self.kind.clone(),
            fields: BTreeMap::new(),
        };
        self.apply(&mut auth)?;
        Ok(auth)
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Progress {
    pub message: String,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub callback_url: Option<String>,
}
impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthProgress")
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            message: "Preparing OAuth authorization".into(),
            authorization_url: None,
            user_code: None,
            callback_url: None,
        }
    }
}
pub struct Channels {
    pub redirects: mpsc::Receiver<String>,
    pub progress: watch::Sender<Progress>,
}
pub fn channels() -> (mpsc::Sender<String>, watch::Receiver<Progress>, Channels) {
    let (sender, redirects) = mpsc::channel(4);
    let (progress, receiver) = watch::channel(Progress::default());
    (
        sender,
        receiver,
        Channels {
            redirects,
            progress,
        },
    )
}
/// No external browser is launched implicitly. The UI displays/copies the URL.
pub async fn acquire(
    config: Config,
    mut channels: Channels,
    cancel: CancellationToken,
) -> Result<TokenSet> {
    config.validate()?;
    let _guard = cancel.clone().drop_guard();
    tokio::select! {biased;
        _=cancel.cancelled()=>Err(DomainError::Cancelled),
        _=tokio::time::sleep(config.timeout)=>Err(invalid("OAuth authorization timed out")),
        result=async {
            let http=client::Client::new()?;
            if config.grant==Grant::OAuth1 {oauth1::acquire(&config,&http,&mut channels).await} else {oauth2_flow::acquire(&config,&http,&mut channels).await}
        }=>result
    }
}
pub async fn refresh(
    config: Config,
    previous: TokenSet,
    cancel: CancellationToken,
) -> Result<TokenSet> {
    config.validate()?;
    let _guard = cancel.clone().drop_guard();
    tokio::select! {biased;
        _=cancel.cancelled()=>Err(DomainError::Cancelled),
        _=tokio::time::sleep(config.timeout)=>Err(invalid("OAuth refresh timed out")),
        result=async {oauth2_flow::refresh(&config,&client::Client::new()?,previous).await}=>result
    }
}
pub(crate) fn endpoint(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|_| invalid("Invalid OAuth endpoint URL"))?;
    if value.len() > 8192
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "OAuth endpoint needs an HTTP(S) URL without userinfo or a fragment",
        ));
    }
    Ok(url)
}
pub(crate) fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "OAuth authorization",
        message: message.into(),
    }
}
