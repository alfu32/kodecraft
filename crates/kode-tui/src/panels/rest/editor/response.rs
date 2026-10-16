//! Bounded live captures and worker-prepared, immutable response displays.
use crate::{
    editor::host::{EditorView, HostDescriptor, HostRole, QueuedEditorHost},
    layout::{Axis, Splitter},
};
use kode_core::{
    DocumentId, DocumentOrigin, JobTag, Result, document::Document, editor::selection::CaretState,
};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    http::{CookieJar, ExecutionKey, Normalization, Response, ResponseHeader},
    resolve::PreparedRequest,
};
use ratatui::layout::Rect;
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};
#[derive(Clone)]
pub struct Live {
    pub key: ExecutionKey,
    pub request: Arc<PreparedRequest>,
    pub normalizations: Arc<[Normalization]>,
    pub headers: Arc<[ResponseHeader]>,
    pub status: Option<u16>,
    pub chunks: Arc<Vec<Arc<[u8]>>>,
    pub retained: usize,
    pub received: u64,
    pub truncated: bool,
    pub started: Instant,
}
impl Live {
    pub fn append(&mut self, bytes: Arc<[u8]>, truncated: bool) {
        const LIMIT: usize = 10 * 1024 * 1024;
        let keep = if self.chunks.len() < 4096 {
            bytes.len().min(LIMIT.saturating_sub(self.retained))
        } else {
            0
        };
        self.truncated |= truncated || keep < bytes.len();
        self.received = self.received.saturating_add(bytes.len() as u64);
        if keep > 0 {
            self.retained += keep;
            Arc::make_mut(&mut self.chunks).push(if keep == bytes.len() {
                bytes
            } else {
                Arc::from(&bytes[..keep])
            });
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Tab {
    #[default]
    Body,
    Headers,
    Cookies,
    Raw,
    Transaction,
}
impl Tab {
    pub const ALL: [Self; 5] = [
        Self::Body,
        Self::Headers,
        Self::Cookies,
        Self::Raw,
        Self::Transaction,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Body => "Body",
            Self::Headers => "Headers",
            Self::Cookies => "Cookies",
            Self::Raw => "Raw",
            Self::Transaction => "Transaction",
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Proof {
    pub key: String,
    pub revision: u64,
    pub tab: Tab,
    pub pretty: bool,
}
pub enum Source {
    Final(Arc<Response>),
    Live(Live),
}
impl Source {
    pub fn key(&self) -> &ExecutionKey {
        match self {
            Self::Final(response) => &response.key,
            Self::Live(live) => &live.key,
        }
    }
}
pub struct Input {
    pub source: Source,
    pub root: PathBuf,
    pub tab: Tab,
    pub pretty: bool,
    pub cookies: Arc<CookieJar>,
}
pub struct Prepared {
    pub document: Document,
    pub descriptor: HostDescriptor,
    pub status: String,
}
pub struct Panel {
    pub tab: Tab,
    pub pretty: bool,
    pub splitter: Splitter,
    pub area: Rect,
    pub body: Rect,
    pub tabs: Vec<(Tab, Rect)>,
    pub pretty_button: Rect,
    pub clear_button: Rect,
    pub editor: Option<EditorView<QueuedEditorHost>>,
    pub pending: Option<JobTag>,
    pub requested: Option<Proof>,
    pub ready: Option<(Proof, Prepared)>,
    pub installed: Option<Proof>,
    pub status: String,
    pub views: HashMap<(String, Tab, bool), (CaretState, usize)>,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            tab: Tab::Body,
            pretty: true,
            splitter: Splitter::new(Axis::Vertical, 16),
            area: Rect::default(),
            body: Rect::default(),
            tabs: Vec::new(),
            pretty_button: Rect::default(),
            clear_button: Rect::default(),
            editor: None,
            pending: None,
            requested: None,
            ready: None,
            installed: None,
            status: String::new(),
            views: HashMap::new(),
        }
    }
}
impl Panel {
    pub fn cache_view(&mut self) {
        if let (Some(proof), Some(view)) = (&self.installed, &self.editor) {
            self.views.insert(
                (proof.key.clone(), proof.tab, proof.pretty),
                (view.input.state.caret.clone(), view.input.state.scroll_rows),
            );
        }
    }
}
pub fn prepare(input: Input, cancel: &CancellationToken) -> Result<Prepared> {
    if cancel.is_cancelled() {
        return Err(kode_core::DomainError::Cancelled);
    }
    let (
        body,
        headers,
        status,
        content_type,
        request,
        normalizations,
        received,
        truncated,
        duration,
        error,
        cookies,
        original,
    ) = match &input.source {
        Source::Final(response) => (
            response.body.clone(),
            response.headers.clone(),
            response.status_code,
            response.content_type.clone(),
            response.normalized_request.clone(),
            response.normalizations.clone(),
            response.received_bytes,
            response.truncated,
            response.duration,
            response.error.map(|error| error.message()),
            response.cookies.clone(),
            Some(response.request.clone()),
        ),
        Source::Live(live) => {
            let mut bytes = Vec::with_capacity(live.retained);
            for chunk in live.chunks.iter() {
                if cancel.is_cancelled() {
                    return Err(kode_core::DomainError::Cancelled);
                }
                bytes.extend_from_slice(chunk);
            }
            let content_type = live
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("content-type"))
                .map(|header| header.text().into_owned());
            (
                Arc::from(bytes),
                live.headers.clone(),
                live.status,
                content_type,
                live.request.clone(),
                live.normalizations.clone(),
                live.received,
                live.truncated,
                live.started.elapsed(),
                None,
                Arc::from([]),
                None,
            )
        }
    };
    let mut status = format!(
        "{} | {} ms | {} bytes{}{}{}",
        status.map_or_else(|| "No status".into(), |status| status.to_string()),
        duration.as_millis(),
        received,
        if matches!(input.source, Source::Live(_)) {
            " | streaming"
        } else {
            ""
        },
        if truncated {
            " | preview truncated"
        } else {
            ""
        },
        error.map_or_else(String::new, |error| format!(" | {error}"))
    );
    if std::str::from_utf8(&body).is_err() {
        status.push_str(" | text preview replaces invalid UTF-8");
    }
    if let Source::Final(response) = &input.source
        && let Some(close) = &response.close
    {
        status.push_str(&format!(" | close {} {}", close.code, close.reason));
    }
    let header_text = || {
        headers
            .iter()
            .map(|header| format!("{}: {}\r\n", header.name, header.text()))
            .collect::<String>()
    };
    let raw_body = || String::from_utf8_lossy(&body);
    let mut language = None;
    let text = match input.tab {
        Tab::Body => {
            language = kode_rest::format::language_for_content_type(content_type.as_deref())
                .map(str::to_string);
            kode_rest::format::pretty_print(
                &raw_body(),
                content_type.as_deref(),
                input.pretty,
                cancel,
            )
            .into_owned()
        }
        Tab::Headers => {
            language = Some("http".into());
            header_text()
        }
        Tab::Cookies => {
            let mut text = String::from("Response Set-Cookie values\n");
            for cookie in cookies.iter() {
                text.push_str(cookie);
                text.push('\n');
            }
            text.push_str("\nWorkspace cookie jar\n");
            for cookie in input.cookies.snapshot() {
                text.push_str(&format!(
                    "{}={} | {}{} | secure={} httpOnly={}\n",
                    cookie.name,
                    cookie.value,
                    cookie.domain,
                    cookie.path,
                    cookie.secure,
                    cookie.http_only
                ));
            }
            text
        }
        Tab::Raw => {
            language = Some("http".into());
            format!(
                "HTTP status {}\r\n{}\r\n{}",
                status.split(" | ").next().unwrap_or(""),
                header_text(),
                raw_body()
            )
        }
        Tab::Transaction => {
            language = Some("http".into());
            let mut text = String::from(
                "Logical request/response transaction. TLS packets and redirect hops are not captured.\n\n",
            );
            if let Some(original) = original {
                text.push_str("Configured request\n");
                text.push_str(&original.preview(10 * 1024 * 1024)?.text);
                text.push_str("\n\n");
            }
            text.push_str("Normalized initial request\n");
            text.push_str(&request.preview(10 * 1024 * 1024)?.text);
            for notice in normalizations.iter() {
                text.push_str(&format!("\n{}: {}", notice.header, notice.reason));
            }
            text.push_str(&format!(
                "\n\nResponse\n{status}\n{}\n{}",
                header_text(),
                raw_body()
            ));
            text
        }
    };
    if cancel.is_cancelled() {
        return Err(kode_core::DomainError::Cancelled);
    }
    let mut descriptor = HostDescriptor::new(
        HostRole::Response,
        "rest-response".into(),
        input.source.key().item_id.clone(),
        format!("Response {}", input.tab.name()),
        input.root,
    );
    descriptor.language = language;
    descriptor.grammar = descriptor.language.clone().unwrap_or_default();
    descriptor.mime = content_type;
    let document = Document::new(
        DocumentId::fresh()?,
        DocumentOrigin::Virtual {
            owner: descriptor.owner.clone(),
            key: descriptor.key.clone(),
            label: descriptor.label.clone(),
        },
        descriptor.language.clone(),
        &text,
    );
    Ok(Prepared {
        document,
        descriptor,
        status,
    })
}
