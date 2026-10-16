use super::{Channels, invalid};
use kode_core::{DomainError, Result};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;

pub(crate) fn validate_uri(value: &str) -> Result<Url> {
    let url = super::endpoint(value)?;
    if url.query_pairs().any(|(k, _)| {
        matches!(
            k.as_ref(),
            "code" | "state" | "error" | "oauth_token" | "oauth_verifier"
        )
    }) {
        return Err(invalid(
            "Configured callback URI must not contain OAuth response parameters",
        ));
    }
    Ok(url)
}
pub(crate) struct Callback {
    pub url: Url,
    listener: Option<TcpListener>,
    tls: Option<tokio_rustls::TlsAcceptor>,
}
impl Callback {
    pub async fn bind(value: &str) -> Result<Self> {
        let mut url = validate_uri(value)?;
        let ip = match url.host() {
            Some(url::Host::Domain("localhost")) => Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
            Some(url::Host::Ipv4(ip)) if ip.is_loopback() => Some(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) if ip.is_loopback() => Some(IpAddr::V6(ip)),
            _ => None,
        };
        let listener = if let Some(ip) = ip {
            let listener=TcpListener::bind(SocketAddr::new(ip,url.port().unwrap_or(0))).await.map_err(|_|invalid("Cannot listen on the configured OAuth loopback callback; choose a free port or use a remote callback URI and paste its redirect"))?;
            let port = listener
                .local_addr()
                .map_err(|_| invalid("Cannot inspect OAuth callback listener"))?
                .port();
            url.set_port(Some(port))
                .map_err(|_| invalid("Invalid OAuth callback port"))?;
            Some(listener)
        } else {
            None
        };
        let tls = if listener.is_some() && url.scheme() == "https" {
            let certified = rcgen::generate_simple_self_signed(vec![
                "localhost".into(),
                "127.0.0.1".into(),
                "::1".into(),
            ])
            .map_err(|_| invalid("Cannot create the ephemeral OAuth callback certificate"))?;
            let key = tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(
                certified.signing_key.serialize_der(),
            );
            let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
                tokio_rustls::rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|_| invalid("Cannot configure callback TLS"))?
            .with_no_client_auth()
            .with_single_cert(vec![certified.cert.der().clone()], key.into())
            .map_err(|_| invalid("Cannot configure the OAuth callback certificate"))?;
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
        } else {
            None
        };
        Ok(Self { url, listener, tls })
    }
    pub async fn wait<T>(
        &self,
        channels: &mut Channels,
        validate: impl Fn(&BTreeMap<String, String>) -> Result<T>,
    ) -> Result<T> {
        loop {
            let incoming = async {
                match &self.listener {
                    Some(listener) => listener
                        .accept()
                        .await
                        .map(|(stream, _)| Some(stream))
                        .map_err(|_| invalid("OAuth callback listener failed")),
                    None => std::future::pending::<Result<Option<TcpStream>>>().await,
                }
            };
            tokio::select! {
                pasted=channels.redirects.recv()=>{
                    let Some(pasted)=pasted else {return Err(DomainError::Cancelled)};
                    match parameters(&self.url,&pasted).and_then(|p|validate(&p)) {
                        Ok(value)=>return Ok(value),
                        Err(error)=>channels.progress.send_modify(|p|p.message=error.to_string()),
                    }
                }
                stream=incoming=>{
                    let stream=stream?.ok_or_else(||invalid("OAuth callback listener closed"))?;
                    let mut stream: Box<dyn CallbackStream + Send + Unpin> = if let Some(tls)=&self.tls {
                        match tokio::time::timeout(Duration::from_secs(2),tls.accept(stream)).await {
                            Ok(Ok(stream))=>Box::new(stream),
                            _=>continue,
                        }
                    } else {Box::new(stream)};
                    // A partial/local hostile connection cannot monopolize the
                    // flow; redirect input is checked again after this bounded read.
                    let read=tokio::time::timeout(Duration::from_secs(2),read(&mut stream,&self.url)).await;
                    let result=match read {Ok(Ok(url))=>parameters(&self.url,&url).and_then(|p|validate(&p)), _=>Err(invalid("Invalid or incomplete OAuth callback request"))};
                    let success=result.is_ok();
                    let body=if success {"Authorization received. You can return to Kode."} else {"Callback rejected. Return to the pending authorization and try again."};
                    let reply=format!("HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",if success {"200 OK"} else {"400 Bad Request"},body.len());
                    let _=tokio::time::timeout(Duration::from_secs(1),stream.write_all(reply.as_bytes())).await;
                    match result {Ok(value)=>return Ok(value),Err(error)=>channels.progress.send_modify(|p|p.message=error.to_string())}
                }
            }
        }
    }
}
trait CallbackStream: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> CallbackStream for T {}
async fn read(stream: &mut (impl AsyncRead + Unpin + ?Sized), base: &Url) -> Result<String> {
    let mut bytes = vec![];
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| invalid("Cannot read OAuth callback"))?;
        if n == 0 || n > (16 * 1024usize).saturating_sub(bytes.len()) {
            return Err(invalid("OAuth callback request is incomplete or too large"));
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid("OAuth callback must be UTF-8"))?;
    let mut first = text.lines().next().unwrap_or("").split_whitespace();
    if first.next() != Some("GET") {
        return Err(invalid("OAuth callback requires GET"));
    }
    let path = first
        .next()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
        .ok_or_else(|| invalid("Invalid OAuth callback target"))?;
    let host = text
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim())
        .collect::<Vec<_>>();
    let expected = &base[url::Position::BeforeHost..url::Position::AfterPort];
    if host.len() != 1 || !host[0].eq_ignore_ascii_case(expected) {
        return Err(invalid("OAuth callback host does not match this listener"));
    }
    let url = base
        .join(path)
        .map_err(|_| invalid("Invalid OAuth callback URL"))?;
    Ok(url.into())
}
pub(crate) fn parameters(expected: &Url, redirect: &str) -> Result<BTreeMap<String, String>> {
    if redirect.len() > 16 * 1024 {
        return Err(invalid("OAuth redirect is too long"));
    }
    let url = Url::parse(redirect.trim())
        .map_err(|_| invalid("Paste the complete OAuth redirect URL"))?;
    if url.scheme() != expected.scheme()
        || url.host() != expected.host()
        || url.port_or_known_default() != expected.port_or_known_default()
        || url.path() != expected.path()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid(
            "OAuth callback URI does not match the pending flow",
        ));
    }
    let mut fields = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if fields
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(invalid("Duplicate OAuth callback parameter"));
        }
    }
    for (key, value) in expected.query_pairs() {
        if fields.get(key.as_ref()).map(String::as_str) != Some(value.as_ref()) {
            return Err(invalid(
                "OAuth callback is missing its configured query parameters",
            ));
        }
    }
    Ok(fields)
}
