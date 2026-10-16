use crate::model::NodePath;
use std::{path::PathBuf, sync::Arc};
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}
impl std::fmt::Debug for Header {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedHeader(..)")
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct HeaderValue {
    pub name: String,
    pub value: String,
    pub source: String,
    pub editable: bool,
    pub disabled: bool,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Auth {
    pub kind: String,
    pub definition: serde_json::Value,
    pub source: String,
}
impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestAuth(..)")
    }
}
impl std::fmt::Debug for HeaderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestHeaderValue")
            .field("editable", &self.editable)
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Http,
    Sse,
    WebSocket,
}
#[derive(Clone, PartialEq, Eq)]
pub enum PartContent {
    Text(String),
    File(PathBuf),
}
#[derive(Clone, PartialEq, Eq)]
pub struct MultipartPart {
    pub name: String,
    pub content: PartContent,
    pub content_type: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
pub enum BodyPlan {
    Empty,
    Bytes(Arc<[u8]>),
    File(PathBuf),
    Multipart {
        boundary: String,
        parts: Vec<MultipartPart>,
    },
}
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedRequest {
    pub path: NodePath,
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: BodyPlan,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub ssl_validation: bool,
    pub protocol: Protocol,
}
#[derive(Clone)]
pub struct PreparedRequest {
    pub request: ResolvedRequest,
    pub body: Arc<[u8]>,
}
impl std::fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRequest")
            .field("request", &self.request)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}
impl std::fmt::Debug for ResolvedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRequest")
            .field("path", &self.path)
            .field("headers", &self.headers.len())
            .field("protocol", &self.protocol)
            .finish_non_exhaustive()
    }
}
