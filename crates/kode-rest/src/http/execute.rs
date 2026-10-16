use super::*;
use crate::resolve::Protocol;
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use reqwest::{
    Client, Method, Url,
    header::{COOKIE, HeaderName, HeaderValue},
};
use std::time::Instant;

const MAX_HEADERS: usize = 256;
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Cloneable per-workspace owner. Constructing it performs no I/O and starts no
/// task. The application scheduler polls execute and cancels it on owner disposal.
#[derive(Clone)]
pub struct Executor {
    cookies: Arc<CookieJar>,
    preview_limit: usize,
}
impl Default for Executor {
    fn default() -> Self {
        Self {
            cookies: Arc::new(CookieJar::default()),
            preview_limit: 10 * 1024 * 1024,
        }
    }
}
impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestHttpExecutor")
            .field("preview_limit", &self.preview_limit)
            .finish_non_exhaustive()
    }
}
impl Executor {
    pub fn new(preview_limit: usize) -> Result<Self> {
        if preview_limit > MAX_BODY_BYTES {
            return Err(invalid("Response preview exceeds 64 MiB"));
        }
        Ok(Self {
            preview_limit,
            ..Default::default()
        })
    }
    pub fn cookies(&self) -> Arc<CookieJar> {
        self.cookies.clone()
    }
    pub fn clear_cookies(&self) {
        self.cookies.clear();
    }
    pub(crate) fn preview_limit(&self) -> usize {
        self.preview_limit
    }
    pub(crate) fn build(
        &self,
        prepared: &PreparedRequest,
    ) -> Result<(Client, reqwest::Request, Vec<Normalization>)> {
        let plan = &prepared.request;
        if plan.protocol == Protocol::WebSocket {
            return Err(invalid(
                "Use the streaming executor for SSE or WebSocket requests",
            ));
        }
        if prepared.body.len() > MAX_BODY_BYTES {
            return Err(invalid("Outgoing body exceeds 64 MiB"));
        }
        if plan.headers.len() > MAX_HEADERS {
            return Err(invalid("Too many outgoing headers"));
        }
        let url = Url::parse(&plan.url).map_err(|_| invalid("Invalid HTTP URL"))?;
        if !["http", "https"].contains(&url.scheme()) || url.host_str().is_none() {
            return Err(invalid(
                "HTTP execution requires an http or https URL with a host",
            ));
        }
        let method = Method::from_bytes(plan.method.as_bytes())
            .map_err(|_| invalid("Invalid HTTP method"))?;
        let redirects = if plan.follow_redirects && plan.auth.is_none() {
            reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() > 10 {
                    attempt.error("Redirect limit reached")
                } else if attempt
                    .previous()
                    .last()
                    .is_some_and(|url| url.scheme() == "https")
                    && attempt.url().scheme() == "http"
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            })
        } else {
            reqwest::redirect::Policy::none()
        };
        let timeout = Duration::from_millis(plan.timeout_ms.max(1));
        let mut builder = Client::builder()
            .no_proxy()
            .cookie_provider(self.cookies.clone())
            .retry(reqwest::retry::never())
            .redirect(redirects)
            .referer(false)
            .connect_timeout(timeout)
            .http1_max_headers(MAX_HEADERS)
            .http2_max_header_list_size(MAX_HEADER_BYTES as u32)
            .danger_accept_invalid_certs(!plan.ssl_validation)
            .danger_accept_invalid_hostnames(!plan.ssl_validation);
        if plan.protocol == Protocol::Http {
            builder = builder.timeout(timeout);
        }
        if plan.auth.as_ref().is_some_and(|a| a.kind == "ntlm")
            || plan
                .headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("Connection"))
        {
            builder = builder.http1_only();
        }
        let client = builder
            .build()
            .map_err(|_| invalid("Cannot configure HTTP client"))?;
        let mut request = client
            .request(method, url.clone())
            .body(prepared.body.to_vec());
        let mut normalizations = vec![];
        for header in &plan.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|_| invalid("Invalid outgoing header name"))?;
            let value = HeaderValue::from_str(&header.value)
                .map_err(|_| invalid("Invalid outgoing header value"))?;
            if managed(name.as_str()) {
                normalizations.push(Normalization { header: header.name.clone(), reason: "Transport owns connection framing, authority and upgrade/expect handling" });
            } else {
                request = request.header(name, value);
            }
        }
        if !plan
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("Cookie"))
            && let Some(value) = self.cookies.header(&url)
        {
            request = request.header(COOKIE, value);
        }
        let mut request = request
            .build()
            .map_err(|_| invalid("Cannot construct HTTP request"))?;
        if let Some(auth) = &plan.auth {
            let method = request.method().to_string();
            let url = request.url().clone();
            crate::auth::sign(
                auth,
                &method,
                &url,
                request.headers_mut(),
                &prepared.body,
                SystemTime::now(),
                &uuid::Uuid::new_v4().to_string(),
            )?;
            if plan.follow_redirects {
                normalizations.push(Normalization { header: "Location".into(), reason: "Signed and challenge-authenticated redirects are returned for explicit review; credentials never follow them automatically" });
            }
        }
        if request.headers().len() > MAX_HEADERS
            || request
                .headers()
                .iter()
                .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
                .sum::<usize>()
                > MAX_HEADER_BYTES
        {
            return Err(invalid("Outgoing headers exceed the configured limit"));
        }
        Ok((client, request, normalizations))
    }
    /// Validation returns an error before network I/O; transport/cancel/timeout
    /// outcomes return a tagged response, retaining any headers and body received.
    pub async fn execute(
        &self,
        key: ExecutionKey,
        prepared: PreparedRequest,
        cancel: CancellationToken,
    ) -> Result<Response> {
        if prepared.request.protocol != Protocol::Http {
            return Err(invalid(
                "Use the streaming executor for SSE or WebSocket requests",
            ));
        }
        let started = Instant::now();
        let started_at = SystemTime::now();
        if cancel.is_cancelled() {
            return Ok(empty(key, prepared, started_at, Failure::Cancelled));
        }
        let (client, request, normalizations) = self.build(&prepared)?;
        let timeout = Duration::from_millis(prepared.request.timeout_ms.max(1));
        let mut normalized = prepared.clone();
        normalized.request.headers = headers_from_request(&request);
        if !prepared.body.is_empty() {
            normalized.request.body = crate::resolve::BodyPlan::Bytes(prepared.body.clone());
        }
        let mut output = Response {
            key,
            final_url: prepared.request.url.clone(),
            request: Arc::new(prepared),
            normalized_request: Arc::new(normalized),
            normalizations: normalizations.into(),
            started_at,
            duration: Duration::ZERO,
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
        };
        let mut bytes = Vec::with_capacity(self.preview_limit.min(64 * 1024));
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let failure = tokio::select! {
            biased;
            _ = cancel.cancelled() => Some(Failure::Cancelled),
            _ = &mut deadline => Some(Failure::Timeout),
            result = self.receive(client, request, &mut output, &mut bytes) => result,
        };
        output.error = failure;
        output.body = bytes.into();
        output.duration = started.elapsed();
        Ok(output)
    }
    async fn receive(
        &self,
        client: Client,
        request: reqwest::Request,
        output: &mut Response,
        bytes: &mut Vec<u8>,
    ) -> Option<Failure> {
        let mut response = match crate::auth::challenge::execute(client, request, output).await {
            Ok(response) => response,
            Err(error) => return Some(error),
        };
        if let Err(error) = apply_metadata(&response, output) {
            return Some(error);
        }
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    output.received_bytes =
                        output.received_bytes.saturating_add(chunk.len() as u64);
                    let keep = self
                        .preview_limit
                        .saturating_sub(bytes.len())
                        .min(chunk.len());
                    bytes.extend_from_slice(&chunk[..keep]);
                    output.truncated |= keep < chunk.len();
                }
                Ok(None) => return None,
                Err(error) => return Some(classify(&error)),
            }
        }
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "HTTP request",
        message: message.into(),
    }
}
pub(crate) fn classify(error: &reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Timeout
    } else if error.is_redirect() {
        Failure::Redirect
    } else if error.is_dns() {
        Failure::Dns
    } else if error.is_connect() {
        Failure::Connect
    } else if error.is_body() || error.is_decode() {
        Failure::Body
    } else {
        Failure::Transport
    }
}
fn empty(
    key: ExecutionKey,
    prepared: PreparedRequest,
    started_at: SystemTime,
    failure: Failure,
) -> Response {
    let request = Arc::new(prepared);
    Response {
        key,
        final_url: request.request.url.clone(),
        normalized_request: request.clone(),
        request,
        started_at,
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
        error: Some(failure),
        close: None,
    }
}
pub(crate) fn apply_metadata(
    response: &reqwest::Response,
    output: &mut Response,
) -> std::result::Result<(), Failure> {
    output.status_code = Some(response.status().as_u16());
    output.status_text = response.status().canonical_reason().unwrap_or("").into();
    output.final_url = response.url().to_string();
    if response.headers().len() > MAX_HEADERS
        || response
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
            .sum::<usize>()
            > MAX_HEADER_BYTES
    {
        return Err(Failure::ResponseHeadersTooLarge);
    }
    output.headers = response
        .headers()
        .iter()
        .map(|(name, value)| ResponseHeader {
            name: name.to_string(),
            value: Arc::from(value.as_bytes()),
        })
        .collect();
    output.cookies = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect();
    output.content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    Ok(())
}
