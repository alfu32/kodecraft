use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};
#[derive(Clone, Debug)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Captured {
    pub fn values(&self, key: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}
pub struct Server {
    pub base: String,
    pub received: Arc<Mutex<Vec<Captured>>>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    pub async fn start(tls: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!(
            "{}://127.0.0.1:{}",
            if tls { "https" } else { "http" },
            listener.local_addr().unwrap().port()
        );
        let received = Arc::new(Mutex::new(vec![]));
        let state = received.clone();
        let acceptor = tls.then(|| {
            let certificate = CertificateDer::from_pem_slice(include_bytes!(
                "../../../../fixtures/rest/http/localhost-cert.pem"
            ))
            .unwrap();
            let key = PrivateKeyDer::from_pem_slice(include_bytes!(
                "../../../../fixtures/rest/http/localhost-key.pem"
            ))
            .unwrap();
            let config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![certificate], key)
                .unwrap();
            TlsAcceptor::from(Arc::new(config))
        });
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream,_)) = accepted else { break; };
                        let state = state.clone(); let acceptor = acceptor.clone();
                        jobs.spawn(async move {
                            if let Some(acceptor) = acceptor {
                                if let Ok(stream) = acceptor.accept(stream).await { handle(stream, state).await; }
                            } else { handle(stream, state).await; }
                        });
                    },
                    _ = jobs.join_next(), if !jobs.is_empty() => {},
                }
            }
        });
        Self {
            base,
            received,
            task,
        }
    }
    pub fn captures(&self) -> Vec<Captured> {
        self.received.lock().unwrap().clone()
    }
}
async fn handle<T: AsyncRead + AsyncWrite + Unpin>(
    mut stream: T,
    received: Arc<Mutex<Vec<Captured>>>,
) {
    let mut input = vec![];
    let mut buffer = [0; 4096];
    let end = loop {
        if input.len() > 256 * 1024 {
            return;
        }
        let Ok(count) = stream.read(&mut buffer).await else {
            return;
        };
        if count == 0 {
            return;
        }
        input.extend_from_slice(&buffer[..count]);
        if let Some(pos) = input.windows(4).position(|b| b == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&input[..end]);
    let mut lines = head.split("\r\n");
    let mut line = lines.next().unwrap().split_whitespace();
    let method = line.next().unwrap().to_string();
    let path = line.next().unwrap().to_string();
    let headers: Vec<_> = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(k, v)| (k.to_string(), v.trim().to_string()))
        })
        .collect();
    let length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("Content-Length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return;
    }
    while input.len() < end + length {
        let Ok(n) = stream.read(&mut buffer).await else {
            return;
        };
        if n == 0 {
            return;
        }
        input.extend_from_slice(&buffer[..n]);
    }
    let captured = Captured {
        method,
        path: path.clone(),
        headers,
        body: input[end..end + length].to_vec(),
    };
    received.lock().unwrap().push(captured.clone());
    let (status, headers, body): (&str, String, Vec<u8>) = match path.split('?').next().unwrap() {
        "/sse" => ("200 OK", "Content-Type: text/event-stream; charset=utf-8\r\n".into(), b"\xef\xbb\xbfid: 7\r\nevent: change\r\ndata: one\r\ndata: two\r\nretry: 5\r\n\r\ndata: \xc3\xa9\n\n".to_vec()),
        "/sse-many" => ("200 OK", "Content-Type: text/event-stream\r\n".into(), b"data: sample\n\n".repeat(100)),
        "/sse-huge" => ("200 OK", "Content-Type: text/event-stream\r\n".into(), vec![b'x';70*1024]),
        "/sse-huge-event" => ("200 OK", "Content-Type: text/event-stream\r\n".into(),format!("data: {}\n","x".repeat(4096)).repeat(70).into_bytes()),
        "/sse-invalid" => ("200 OK", "Content-Type: text/event-stream\r\n".into(), b"data: \xff\n\n".to_vec()),
        "/sse-infinite" => {
            if stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.is_err() { return; }
            loop { if stream.write_all(b"data: tick\n\n").await.is_err() {return;} let _=stream.flush().await; tokio::time::sleep(Duration::from_millis(1)).await; }
        },
        "/huge-header" => ("200 OK", format!("X-Huge: {}\r\n", "x".repeat(270*1024)), b"ok".to_vec()),
        "/set" => ("200 OK", "Set-Cookie: session=abc; Path=/; HttpOnly\r\nX-Duplicate: one\r\nX-Duplicate: two\r\nContent-Type: application/json\r\n".into(), br#"{"ok":true}"#.to_vec()),
        "/echo" => ("200 OK", String::new(), captured.values("Cookie").join("; ").into_bytes()),
        "/reflect" => ("200 OK", String::new(), captured.body),
        "/large" => ("200 OK", String::new(), b"0123456789".repeat(100)),
        "/status" => ("418 I'm a teapot", String::new(), b"teapot".to_vec()),
        "/binary" => ("200 OK", String::new(), vec![0,255,0xc3,0xa9]),
        "/redirect" => ("302 Found", "Location: /set\r\nSet-Cookie: redirect=yes; Path=/\r\n".into(), vec![]),
        "/redirect307" => ("307 Temporary Redirect", "Location: /reflect\r\n".into(), vec![]),
        "/redirect-to" => { let target = url::form_urlencoded::parse(path.split_once('?').map_or("", |(_,q)|q).as_bytes()).find(|(k,_)| k == "url").unwrap().1; ("302 Found", format!("Location: {target}\r\n"), vec![]) },
        "/loop" => ("302 Found", "Location: /loop\r\n".into(), vec![]),
        "/hang" => { tokio::time::sleep(Duration::from_secs(10)).await; return; },
        "/slow" => { let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nabc").await; let _ = stream.flush().await; tokio::time::sleep(Duration::from_secs(10)).await; return; },
        "/broken" => { let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nabc").await; let _ = stream.shutdown().await; return; },
        "/chunked" => { let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nX-Binary: \x80\r\nConnection: close\r\n\r\n1\r\n\xc3\r\n1\r\n\xa9\r\n0\r\n\r\n").await; let _ = stream.shutdown().await; return; },
        _ => ("200 OK", String::new(), b"ok".to_vec()),
    };
    let header = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}
