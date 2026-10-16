//! One bounded progress stream for HTTP/SSE/WS; the returned tagged Response is
//! authoritative completion. The application owns the future and cancellation.
pub mod sse;
pub mod websocket;
use crate::{
    http::{self, CookieJar, ExecutionKey, Failure, Response, ResponseHeader},
    resolve::{PreparedRequest, Protocol},
};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc;
pub const EVENT_CAPACITY: usize = 64;
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_RETAINED_EVENTS: usize = 4096;
pub type EventSender = mpsc::Sender<Event>;
pub type EventReceiver = mpsc::Receiver<Event>;
pub fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel(EVENT_CAPACITY)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    Http,
    SseRaw,
    WebSocketText,
    WebSocketBinary,
}
#[derive(Clone)]
pub enum EventKind {
    Opened {
        status: Option<u16>,
        headers: Arc<[ResponseHeader]>,
        final_url: String,
        normalized_request: Arc<PreparedRequest>,
        normalizations: Arc<[http::Normalization]>,
    },
    Data {
        kind: DataKind,
        bytes: Arc<[u8]>,
        message_end: bool,
        truncated: bool,
    },
    Sse {
        name: String,
        data: String,
        id: String,
        retry: Option<std::time::Duration>,
    },
    Progress {
        received_bytes: u64,
        retained_bytes: usize,
        truncated: bool,
    },
    Closed(http::CloseInfo),
    Notice(&'static str),
}
#[derive(Clone)]
pub struct Event {
    pub key: ExecutionKey,
    pub kind: EventKind,
}
impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestStreamEvent")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InitialMessage {
    #[default]
    None,
    Text,
    Binary,
}
#[derive(Clone, Default, Debug)]
pub struct Executor {
    http: http::Executor,
}
impl Executor {
    pub fn new(preview_limit: usize) -> Result<Self> {
        Ok(Self {
            http: http::Executor::new(preview_limit)?,
        })
    }
    pub fn cookies(&self) -> Arc<CookieJar> {
        self.http.cookies()
    }
    pub fn clear_cookies(&self) {
        self.http.clear_cookies();
    }
    pub async fn execute(
        &self,
        key: ExecutionKey,
        request: PreparedRequest,
        cancel: CancellationToken,
        sender: Option<EventSender>,
    ) -> Result<Response> {
        self.execute_with_initial(key, request, cancel, sender, InitialMessage::None)
            .await
    }
    /// Sending an initial WebSocket message is an explicit extension; default
    /// execution matches the source's receive-only opening handshake.
    pub async fn execute_with_initial(
        &self,
        key: ExecutionKey,
        request: PreparedRequest,
        cancel: CancellationToken,
        sender: Option<EventSender>,
        initial: InitialMessage,
    ) -> Result<Response> {
        if sender.as_ref().is_some_and(EventSender::is_closed) {
            let mut response = Response::pending(key, request);
            response.error = Some(Failure::Cancelled);
            return Ok(response);
        }
        if sender
            .as_ref()
            .is_some_and(|sender| sender.max_capacity() > EVENT_CAPACITY)
        {
            return Err(invalid("Progress queue exceeds 64 events"));
        }
        let sink = Sink {
            key: key.clone(),
            sender,
            cancel: cancel.clone(),
        };
        if request.request.protocol == Protocol::Http {
            let response = self.http.execute(key, request, cancel).await?;
            // Ordinary HTTP source behavior publishes on completion. Streaming
            // transports publish incrementally through the same event variants.
            if sink.opened(&response).await.is_ok() {
                for chunk in response.body.chunks(16 * 1024) {
                    if sink
                        .send(EventKind::Data {
                            kind: DataKind::Http,
                            bytes: Arc::from(chunk),
                            message_end: false,
                            truncated: response.truncated,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            return Ok(response);
        }
        let started = Instant::now();
        let protocol = request.request.protocol;
        let mut output = Response::pending(key, request);
        if cancel.is_cancelled() {
            output.error = Some(Failure::Cancelled);
            return Ok(output);
        }
        let mut capture = Capture::new(self.http.preview_limit());
        let result = {
            let work: std::pin::Pin<
                Box<dyn std::future::Future<Output = std::result::Result<(), Failure>> + Send + '_>,
            > = match protocol {
                Protocol::Sse => {
                    let (client, request, notices) = self.http.build(&output.request)?;
                    let mut normalized = (*output.request).clone();
                    normalized.request.headers = http::headers_from_request(&request);
                    output.normalized_request = Arc::new(normalized);
                    output.normalizations = notices.into();
                    Box::pin(sse::run(client, request, &mut output, &mut capture, &sink))
                }
                Protocol::WebSocket => {
                    let prepared = websocket::prepare(&output.request, &self.cookies(), initial)?;
                    output.normalized_request = Arc::new(prepared.normalized.clone());
                    output.normalizations = prepared.notices.clone().into();
                    Box::pin(websocket::run(
                        prepared,
                        &mut output,
                        &mut capture,
                        &sink,
                        self.cookies(),
                    ))
                }
                Protocol::Http => unreachable!(),
            };
            tokio::select! { biased; _ = cancel.cancelled() => Err(Failure::Cancelled), result = work => result }
        };
        output.error = result.err();
        output.body = capture.bytes.into();
        output.received_bytes = capture.received;
        output.truncated = capture.truncated;
        output.duration = started.elapsed();
        Ok(output)
    }
}
#[derive(Clone)]
pub(super) struct Sink {
    key: ExecutionKey,
    sender: Option<EventSender>,
    cancel: CancellationToken,
}
impl Sink {
    pub(super) async fn send(&self, kind: EventKind) -> std::result::Result<(), Failure> {
        let Some(sender) = &self.sender else {
            return Ok(());
        };
        tokio::select! { biased; _ = self.cancel.cancelled() => Err(Failure::Cancelled), sent = sender.send(Event {key:self.key.clone(),kind}) => sent.map_err(|_|Failure::Cancelled) }
    }
    pub(super) async fn opened(&self, response: &Response) -> std::result::Result<(), Failure> {
        self.send(EventKind::Opened {
            status: response.status_code,
            headers: response.headers.clone(),
            final_url: response.final_url.clone(),
            normalized_request: response.normalized_request.clone(),
            normalizations: response.normalizations.clone(),
        })
        .await
    }
}
pub(super) struct Capture {
    bytes: Vec<u8>,
    received: u64,
    truncated: bool,
    limit: usize,
}
impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            received: 0,
            truncated: false,
            limit,
        }
    }
    pub(super) async fn record(
        &mut self,
        bytes: &[u8],
        kind: DataKind,
        message_end: bool,
        sink: &Sink,
    ) -> std::result::Result<(), Failure> {
        self.received = self.received.saturating_add(bytes.len() as u64);
        let keep = self.limit.saturating_sub(self.bytes.len()).min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..keep]);
        self.truncated |= keep < bytes.len();
        for (index, chunk) in bytes[..keep].chunks(16 * 1024).enumerate() {
            sink.send(EventKind::Data {
                kind,
                bytes: Arc::from(chunk),
                message_end: message_end && (index + 1) * 16 * 1024 >= keep,
                truncated: keep < bytes.len(),
            })
            .await?;
        }
        sink.send(EventKind::Progress {
            received_bytes: self.received,
            retained_bytes: self.bytes.len(),
            truncated: self.truncated,
        })
        .await
    }
}
pub(super) fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST stream",
        message: message.into(),
    }
}
