use super::*;
use crate::{
    http::{CloseInfo, Normalization, ResponseHeader},
    resolve::{BodyPlan, Header},
};
use futures_util::{SinkExt, StreamExt};
use rustls_platform_verifier::BuilderVerifierExt;
use std::time::Duration;
use tokio_rustls::rustls::{
    self, ClientConfig,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{self, Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

pub(super) struct Prepared {
    request: tungstenite::handshake::client::Request,
    connector: Option<Connector>,
    cookie_url: url::Url,
    initial: Option<Message>,
    pub normalized: PreparedRequest,
    pub notices: Vec<Normalization>,
}
pub(super) fn prepare(
    prepared: &PreparedRequest,
    cookies: &CookieJar,
    initial: InitialMessage,
) -> Result<Prepared> {
    if prepared.request.headers.len() > 256
        || prepared
            .request
            .headers
            .iter()
            .map(|h| h.name.len() + h.value.len())
            .sum::<usize>()
            > 256 * 1024
    {
        return Err(invalid(
            "Outgoing WebSocket headers exceed the configured limit",
        ));
    }
    let url =
        url::Url::parse(&prepared.request.url).map_err(|_| invalid("Invalid WebSocket URL"))?;
    if !["ws", "wss"].contains(&url.scheme()) || url.host_str().is_none() {
        return Err(invalid("WebSocket requires a ws or wss URL with a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(
            "Use request authentication instead of WebSocket URL userinfo",
        ));
    }
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| invalid("Cannot construct WebSocket handshake"))?;
    let mut notices = vec![];
    for header in &prepared.request.headers {
        let name = ::http::HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| invalid("Invalid WebSocket header name"))?;
        let value = ::http::HeaderValue::from_str(&header.value)
            .map_err(|_| invalid("Invalid WebSocket header value"))?;
        if http::managed(name.as_str())
            || [
                "sec-websocket-key",
                "sec-websocket-version",
                "sec-websocket-extensions",
            ]
            .iter()
            .any(|managed| name.as_str().eq_ignore_ascii_case(managed))
        {
            notices.push(Normalization {
                header: header.name.clone(),
                reason: "WebSocket handshake owns framing, upgrade and extension negotiation",
            });
        } else {
            request.headers_mut().append(name, value);
        }
    }
    let mut cookie_url = url.clone();
    cookie_url
        .set_scheme(if url.scheme() == "wss" {
            "https"
        } else {
            "http"
        })
        .map_err(|_| invalid("Invalid WebSocket cookie origin"))?;
    if !request.headers().contains_key(::http::header::COOKIE)
        && let Some(cookie) = cookies.header(&cookie_url)
    {
        request.headers_mut().insert(::http::header::COOKIE, cookie);
    }
    if request.headers().len() > 256
        || request
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>()
            > 256 * 1024
    {
        return Err(invalid(
            "WebSocket handshake headers exceed the configured limit",
        ));
    }
    let connector = if url.scheme() == "wss" {
        Some(Connector::Rustls(Arc::new(tls_config(
            prepared.request.ssl_validation,
        )?)))
    } else {
        None
    };
    let initial = match initial {
        InitialMessage::None => None,
        InitialMessage::Text | InitialMessage::Binary if prepared.body.len() > MAX_EVENT_BYTES => {
            return Err(invalid("Initial WebSocket message exceeds 256 KiB"));
        }
        InitialMessage::Text => Some(Message::Text(
            std::str::from_utf8(&prepared.body)
                .map_err(|_| invalid("Initial text WebSocket message must be UTF-8"))?
                .to_string()
                .into(),
        )),
        InitialMessage::Binary => Some(Message::Binary(prepared.body.to_vec().into())),
    };
    let mut normalized = prepared.clone();
    normalized.request.method = "GET".into();
    normalized.body = Arc::from([]);
    normalized.request.body = BodyPlan::Empty;
    normalized.request.headers = request
        .headers()
        .iter()
        .map(|(name, value)| Header {
            name: name.to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect();
    if prepared.request.method != "GET" {
        notices.push(Normalization {
            header: "method".into(),
            reason: "WebSocket opening handshake uses GET",
        });
    }
    if !prepared.body.is_empty() {
        notices.push(Normalization{header:"body".into(),reason:"WebSocket handshake has no HTTP request body; initial messages require explicit opt-in"});
    }
    Ok(Prepared {
        request,
        connector,
        cookie_url,
        initial,
        normalized,
        notices,
    })
}
pub(super) async fn run(
    mut prepared: Prepared,
    output: &mut Response,
    capture: &mut Capture,
    sink: &Sink,
    cookies: Arc<CookieJar>,
) -> std::result::Result<(), Failure> {
    let config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(2 * MAX_EVENT_BYTES)
        .max_message_size(Some(MAX_EVENT_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let timeout = Duration::from_millis(output.request.request.timeout_ms.max(1));
    let began = std::time::Instant::now();
    let mut redirects = 0;
    let mut plan = (*output.request).clone();
    let (mut socket, response) = loop {
        let connected = tokio::time::timeout(
            timeout.saturating_sub(began.elapsed()),
            connect_async_tls_with_config(
                prepared.request.clone(),
                Some(config),
                false,
                prepared.connector.clone(),
            ),
        )
        .await
        .map_err(|_| Failure::Timeout)?;
        match connected {
            Ok(connection) => break connection,
            Err(tungstenite::Error::Http(response)) => {
                metadata(response.status(), response.headers(), output);
                cookies.accept(
                    &prepared.cookie_url,
                    response
                        .headers()
                        .get_all(::http::header::SET_COOKIE)
                        .iter()
                        .filter_map(|header| header.to_str().ok()),
                );
                if plan.request.follow_redirects
                    && [301, 302, 303, 307, 308].contains(&response.status().as_u16())
                    && let Some(location) = response
                        .headers()
                        .get(::http::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                {
                    redirects += 1;
                    if redirects > 10 {
                        return Err(Failure::Redirect);
                    }
                    let current =
                        url::Url::parse(&plan.request.url).map_err(|_| Failure::StreamProtocol)?;
                    let mut next = current
                        .join(location)
                        .map_err(|_| Failure::StreamProtocol)?;
                    let scheme = match next.scheme() {
                        "http" => "ws",
                        "https" => "wss",
                        scheme => scheme,
                    }
                    .to_string();
                    if !["ws", "wss"].contains(&scheme.as_str()) {
                        return Err(Failure::StreamProtocol);
                    }
                    next.set_scheme(&scheme)
                        .map_err(|_| Failure::StreamProtocol)?;
                    if !(current.scheme() == "wss" && next.scheme() == "ws") {
                        if current.origin() != next.origin() {
                            plan.request.headers.retain(|header| {
                                !["authorization", "cookie", "proxy-authorization"]
                                    .iter()
                                    .any(|name| header.name.eq_ignore_ascii_case(name))
                            });
                        }
                        plan.request.url = next.to_string();
                        let mut redirect = prepare(&plan, &cookies, InitialMessage::None)
                            .map_err(|_| Failure::StreamProtocol)?;
                        redirect.initial = prepared.initial.take();
                        prepared = redirect;
                        output.final_url = next.to_string();
                        continue;
                    }
                }
                if let Some(body) = response.body() {
                    capture.record(body, DataKind::Http, false, sink).await?;
                }
                return Err(Failure::StreamProtocol);
            }
            Err(error) => return Err(classify_ws(&error)),
        }
    };
    metadata(response.status(), response.headers(), output);
    cookies.accept(
        &prepared.cookie_url,
        response
            .headers()
            .get_all(::http::header::SET_COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok()),
    );
    output.content_type = Some("application/octet-stream".into());
    sink.opened(output).await?;
    if let Some(initial) = prepared.initial {
        socket
            .send(initial)
            .await
            .map_err(|error| classify_ws(&error))?;
    }
    while let Some(message) = socket.next().await {
        match message.map_err(|error| classify_ws(&error))? {
            Message::Text(text) => {
                capture
                    .record(text.as_bytes(), DataKind::WebSocketText, true, sink)
                    .await?
            }
            Message::Binary(bytes) => {
                capture
                    .record(&bytes, DataKind::WebSocketBinary, true, sink)
                    .await?
            }
            Message::Ping(_) | Message::Pong(_) => {
                socket.flush().await.map_err(|error| classify_ws(&error))?;
            }
            Message::Close(frame) => {
                let close = frame.map_or(
                    CloseInfo {
                        code: 1005,
                        reason: String::new(),
                    },
                    |frame| CloseInfo {
                        code: frame.code.into(),
                        reason: frame.reason.to_string(),
                    },
                );
                let abnormal = ![1000, 1001, 1005].contains(&close.code);
                output.close = Some(close.clone());
                // Tungstenite queued the close acknowledgement. Bound flushing
                // so an unresponsive peer cannot delay completion indefinitely.
                let _ = tokio::time::timeout(Duration::from_millis(100), socket.flush()).await;
                sink.send(EventKind::Closed(close)).await?;
                return if abnormal {
                    Err(Failure::WebSocketAbnormalClose)
                } else {
                    Ok(())
                };
            }
            Message::Frame(_) => {}
        }
    }
    Err(Failure::WebSocketAbnormalClose)
}
fn metadata(status: ::http::StatusCode, headers: &::http::HeaderMap, output: &mut Response) {
    output.status_code = Some(status.as_u16());
    output.status_text = status.canonical_reason().unwrap_or("").into();
    output.headers = headers
        .iter()
        .map(|(name, value)| ResponseHeader {
            name: name.to_string(),
            value: Arc::from(value.as_bytes()),
        })
        .collect();
    output.cookies = headers
        .get_all(::http::header::SET_COOKIE)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .collect();
}
fn classify_ws(error: &tungstenite::Error) -> Failure {
    match error {
        tungstenite::Error::Io(_) | tungstenite::Error::Tls(_) => Failure::Connect,
        tungstenite::Error::Capacity(_)
        | tungstenite::Error::WriteBufferFull(_)
        | tungstenite::Error::AttackAttempt => Failure::StreamLimit,
        tungstenite::Error::ConnectionClosed => Failure::WebSocketAbnormalClose,
        tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => Failure::WebSocketAbnormalClose,
        _ => Failure::StreamProtocol,
    }
}
#[derive(Debug)]
struct SkipCertificateValidation(Arc<CryptoProvider>);
impl ServerCertVerifier for SkipCertificateValidation {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
fn tls_config(validate: bool) -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|_| invalid("Cannot configure WebSocket TLS"))?;
    if validate {
        Ok(builder
            .with_platform_verifier()
            .map_err(|_| invalid("Cannot initialize platform TLS verification"))?
            .with_no_client_auth())
    } else {
        Ok(builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(SkipCertificateValidation(provider)))
            .with_no_client_auth())
    }
}
