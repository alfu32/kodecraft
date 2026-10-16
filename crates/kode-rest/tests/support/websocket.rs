use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
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
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        protocol::{
            CloseFrame,
            frame::{
                Frame,
                coding::{CloseCode, Data, OpCode},
            },
        },
    },
};
pub struct Server {
    pub base: String,
    pub headers: Headers,
    pub incoming: Arc<Mutex<Vec<Vec<u8>>>>,
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
            if tls { "wss" } else { "ws" },
            listener.local_addr().unwrap().port()
        );
        let headers = Arc::new(Mutex::new(vec![]));
        let incoming = Arc::new(Mutex::new(vec![]));
        let h = headers.clone();
        let i = incoming.clone();
        let acceptor = tls.then(|| {
            let certificate = CertificateDer::from_pem_slice(include_bytes!(
                "../../../../fixtures/rest/http/localhost-cert.pem"
            ))
            .unwrap();
            let key = PrivateKeyDer::from_pem_slice(include_bytes!(
                "../../../../fixtures/rest/http/localhost-key.pem"
            ))
            .unwrap();
            TlsAcceptor::from(Arc::new(
                ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(vec![certificate], key)
                    .unwrap(),
            ))
        });
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{let Ok((stream,_))=accepted else{break};let h=h.clone();let i=i.clone();let acceptor=acceptor.clone();jobs.spawn(async move{if let Some(acceptor)=acceptor{if let Ok(stream)=acceptor.accept(stream).await{serve(stream,h,i).await;}}else{serve(stream,h,i).await;}});},
                    _=jobs.join_next(),if !jobs.is_empty()=>{},
                }
            }
        });
        Self {
            base,
            headers,
            incoming,
            task,
        }
    }
}
type Headers = Arc<Mutex<Vec<Vec<(String, String)>>>>;
// Tungstenite's callback fixes the unboxed HTTP error response type.
#[allow(clippy::result_large_err)]
async fn serve<T: AsyncRead + AsyncWrite + Unpin>(
    stream: T,
    headers: Headers,
    incoming: Arc<Mutex<Vec<Vec<u8>>>>,
) {
    let path = Arc::new(Mutex::new(String::new()));
    let target = path.clone();
    let accepted = accept_hdr_async(stream, move |request: &Request, mut response: Response| {
        *target.lock().unwrap() = request.uri().path().to_string();
        headers.lock().unwrap().push(
            request
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
                .collect(),
        );
        if request.uri().path() == "/redirect" || request.uri().path() == "/loop" {
            let target = if request.uri().path() == "/loop" {
                "/loop".to_string()
            } else {
                url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                    .find(|(key, _)| key == "url")
                    .map(|(_, value)| value.into_owned())
                    .unwrap_or_else(|| "/messages".into())
            };
            return Err(::http::Response::builder()
                .status(302)
                .header("Location", target)
                .body(None)
                .unwrap());
        }
        response
            .headers_mut()
            .append("Set-Cookie", "socket=yes; Path=/".parse().unwrap());
        response
            .headers_mut()
            .append("X-Duplicate", "one".parse().unwrap());
        response
            .headers_mut()
            .append("X-Duplicate", "two".parse().unwrap());
        Ok(response)
    })
    .await;
    let Ok(mut socket) = accepted else { return };
    let path = path.lock().unwrap().clone();
    match path.as_str() {
        "/initial" => {
            if let Some(Ok(message)) = socket.next().await {
                incoming
                    .lock()
                    .unwrap()
                    .push(message.clone().into_data().to_vec());
                if socket.send(message).await.is_err() {
                    return;
                }
            }
        }
        "/messages" => {
            for message in [
                Message::Frame(Frame::message(vec![0xc3], OpCode::Data(Data::Text), false)),
                Message::Frame(Frame::message(
                    vec![0xa9],
                    OpCode::Data(Data::Continue),
                    true,
                )),
                Message::Binary(vec![0, 255].into()),
                Message::Ping(vec![1].into()),
            ] {
                if socket.send(message).await.is_err() {
                    return;
                }
            }
        }
        "/abnormal" => {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "denied".into(),
                })))
                .await;
            return;
        }
        "/drop" => {
            let _ = socket.send(Message::Text("partial".into())).await;
            return;
        }
        "/large" => {
            let _ = socket
                .send(Message::Binary(vec![0; 70 * 1024].into()))
                .await;
            return;
        }
        "/large-message" => {
            for index in 0..5 {
                let frame = Frame::message(
                    vec![b'a'; if index == 4 { 1 } else { 64 * 1024 }],
                    OpCode::Data(if index == 0 {
                        Data::Text
                    } else {
                        Data::Continue
                    }),
                    index == 4,
                );
                if socket.send(Message::Frame(frame)).await.is_err() {
                    return;
                }
            }
            return;
        }
        "/infinite" => loop {
            if socket.send(Message::Text("tick".into())).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        },
        _ => {}
    }
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "done".into(),
        })))
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}
