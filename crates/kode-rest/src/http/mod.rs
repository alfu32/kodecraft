//! Native async HTTP. The caller owns scheduling, workspace disposal and the
//! cancellation token; no request is retried or restored from persisted data.
pub mod cookies;
mod execute;
use crate::resolve::{Header, PreparedRequest};
pub use cookies::{CookieInfo, CookieJar};
pub use execute::Executor;
pub(crate) use execute::{apply_metadata, classify};
use kode_core::JobTag;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionKey {
    pub tag: JobTag,
    /// Persisted collection item's ID, captured before any edits or tree moves.
    pub item_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Cancelled,
    Timeout,
    Dns,
    Connect,
    Redirect,
    Body,
    Transport,
    ResponseHeadersTooLarge,
    StreamLimit,
    StreamProtocol,
    WebSocketAbnormalClose,
}
impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Request cancelled; a sent request may already have taken effect",
            Self::Timeout => "Request timed out; a sent request may already have taken effect",
            Self::Dns => "Hostname lookup failed",
            Self::Connect => "Connection or TLS negotiation failed",
            Self::Redirect => "Redirect failed or exceeded the limit",
            Self::Body => "Response body ended with a transport error",
            Self::Transport => "HTTP transport failed",
            Self::ResponseHeadersTooLarge => "Response headers exceed the configured limit",
            Self::StreamLimit => "Stream event, message or frame exceeds the configured limit",
            Self::StreamProtocol => "Stream protocol or encoding error",
            Self::WebSocketAbnormalClose => "WebSocket closed abnormally",
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct ResponseHeader {
    pub name: String,
    pub value: Arc<[u8]>,
}
impl std::fmt::Debug for ResponseHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResponseHeader(..)")
    }
}
impl ResponseHeader {
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.value)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalization {
    pub header: String,
    pub reason: &'static str,
}
#[derive(Clone)]
pub struct Response {
    pub key: ExecutionKey,
    pub request: Arc<PreparedRequest>,
    /// Logical initial request after normalization, including initial cookies.
    /// Redirect hops and TLS/wire frames are not represented as packet captures.
    pub normalized_request: Arc<PreparedRequest>,
    pub normalizations: Arc<[Normalization]>,
    pub started_at: SystemTime,
    pub duration: Duration,
    pub status_code: Option<u16>,
    pub status_text: String,
    pub headers: Arc<[ResponseHeader]>,
    pub cookies: Arc<[String]>,
    pub body: Arc<[u8]>,
    pub received_bytes: u64,
    pub truncated: bool,
    pub final_url: String,
    pub content_type: Option<String>,
    pub error: Option<Failure>,
    pub close: Option<CloseInfo>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct CloseInfo {
    pub code: u16,
    pub reason: String,
}
impl std::fmt::Debug for CloseInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloseInfo")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}
impl Response {
    pub(crate) fn pending(key: ExecutionKey, prepared: PreparedRequest) -> Self {
        let request = Arc::new(prepared);
        Self {
            key,
            final_url: request.request.url.clone(),
            normalized_request: request.clone(),
            request,
            started_at: SystemTime::now(),
            duration: Duration::ZERO,
            normalizations: Arc::from([]),
            status_code: None,
            status_text: String::new(),
            headers: Arc::from([]),
            cookies: Arc::from([]),
            body: Arc::from([]),
            received_bytes: 0,
            truncated: false,
            content_type: None,
            error: None,
            close: None,
        }
    }
    pub fn is_success(&self) -> bool {
        self.error.is_none()
            && self.status_code.is_some_and(|status| {
                (200..300).contains(&status)
                    || (status == 101
                        && self.request.request.protocol == crate::resolve::Protocol::WebSocket)
            })
    }
    pub fn body_text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}
impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestResponse")
            .field("key", &self.key)
            .field("status", &self.status_code)
            .field("received_bytes", &self.received_bytes)
            .field("truncated", &self.truncated)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
pub(crate) fn managed(name: &str) -> bool {
    [
        "host",
        "content-length",
        "upgrade",
        "connection",
        "expect",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
    ]
    .iter()
    .any(|managed| name.eq_ignore_ascii_case(managed))
}
pub(crate) fn headers_from_request(request: &reqwest::Request) -> Vec<Header> {
    request
        .headers()
        .iter()
        .map(|(name, value)| Header {
            name: name.to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect()
}
