use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    auth::ExecutionAuth,
    oauth::{self, ClientAuthentication, Config, Grant, TokenSet},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinHandle, JoinSet},
};

#[derive(Clone)]
struct Captured {
    path: String,
    headers: BTreeMap<String, String>,
    form: BTreeMap<String, String>,
}
struct Provider {
    base: String,
    requests: Arc<Mutex<Vec<Captured>>>,
    task: JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Provider {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let seen = requests.clone();
        let url = base.clone();
        let task = tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            loop {
                tokio::select! {
                    next=listener.accept()=>{let Ok((mut stream,_))=next else {break};let seen=seen.clone();let url=url.clone();jobs.spawn(async move {
                        let request=read(&mut stream).await;
                        let number={let mut seen=seen.lock().unwrap();seen.push(request.clone());seen.iter().filter(|r|r.path==request.path).count()};
                        if request.path=="/hang" {tokio::time::sleep(Duration::from_secs(60)).await;return;}
                        let(status,content,body)=match request.path.as_str() {
                        "/protected" if request.headers.get("authorization").is_some_and(|v|v=="Bearer runtime-access")=>("200 OK","text/plain","authorized".into()),
                        "/token"=>{
                                if let Some(verifier)=request.form.get("code_verifier") {assert!(verifier.len()>=43);}
                                let token=if request.form.get("grant_type").is_some_and(|v|v=="refresh_token") {"rotated-access"} else {"runtime-access"};
                                ("200 OK","application/json",json!({"access_token":token,"refresh_token":"runtime-refresh","token_type":"Bearer","expires_in":60,"scope":"read write"}).to_string())
                            }
                            "/expired"=>("200 OK","application/json",json!({"access_token":"expired-access","token_type":"Bearer","expires_in":0}).to_string()),
                            "/reject"=>("400 Bad Request","application/json",json!({"error":"invalid_client","error_description":"NEVER-LOG-THIS-SECRET"}).to_string()),
                            "/bad-json"=>("200 OK","application/json","NEVER-LOG-THIS-SECRET not json".into()),
                            "/huge"=>("200 OK","application/json","x".repeat(70*1024)),
                            "/redirect"=>("302 Found","application/json","{}".into()),
                            "/device"=>("200 OK","application/json",json!({"device_code":"runtime-device-secret","user_code":"ABCD-EFGH","verification_uri":format!("{url}/verify"),"verification_uri_complete":format!("{url}/verify?user_code=ABCD-EFGH"),"expires_in":30,"interval":1}).to_string()),
                            "/poll" if number==1=>("400 Bad Request","application/json",json!({"error":"authorization_pending"}).to_string()),
                            "/poll" if number==2=>("400 Bad Request","application/json",json!({"error":"slow_down"}).to_string()),
                            "/poll"=>("200 OK","application/json",json!({"access_token":"device-access","token_type":"Bearer","expires_in":60}).to_string()),
                            "/request-token"=>("200 OK","application/x-www-form-urlencoded","oauth_token=temporary&oauth_token_secret=temporary-secret&oauth_callback_confirmed=true".into()),
                            "/access-token"=>("200 OK","application/x-www-form-urlencoded","oauth_token=oauth1-access&oauth_token_secret=oauth1-secret".into()),
                            _=>("404 Not Found","text/plain","missing".into()),
                        };
                        let location=if request.path=="/redirect" {format!("Location: {url}/token\r\n")} else {String::new()};
                        let head=format!("HTTP/1.1 {status}\r\nContent-Type: {content}\r\nContent-Length: {}\r\nConnection: close\r\n{location}\r\n",body.len());
                        let _=stream.write_all(head.as_bytes()).await;let _=stream.write_all(body.as_bytes()).await;
                    });},
                    _=jobs.join_next(),if !jobs.is_empty()=>{},
                }
            }
        });
        Self {
            base,
            requests,
            task,
        }
    }
    fn config(&self, grant: Grant) -> Config {
        Config {
            grant,
            authorization_url: format!("{}/authorize", self.base),
            token_url: format!("{}/token", self.base),
            request_token_url: format!("{}/request-token", self.base),
            device_url: format!("{}/device", self.base),
            client_id: "fixture-client".into(),
            client_secret: "fixture-secret".into(),
            scopes: vec!["read".into(), "write".into()],
            timeout: Duration::from_secs(15),
            redirect_url: "http://127.0.0.1:0/callback".into(),
            ..Default::default()
        }
    }
    fn requests(&self) -> Vec<Captured> {
        self.requests.lock().unwrap().clone()
    }
}
async fn read(stream: &mut TcpStream) -> Captured {
    let mut input = vec![];
    let mut bytes = [0u8; 1024];
    let end = loop {
        let n = stream.read(&mut bytes).await.unwrap();
        assert!(n > 0);
        input.extend_from_slice(&bytes[..n]);
        if let Some(at) = input.windows(4).position(|b| b == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8(input[..end].to_vec()).unwrap();
    let path = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let headers: BTreeMap<String, String> = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().into()))
        .collect();
    let len = headers
        .get("content-length")
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(0);
    while input.len() < end + len {
        let n = stream.read(&mut bytes).await.unwrap();
        assert!(n > 0);
        input.extend_from_slice(&bytes[..n]);
    }
    let form = url::form_urlencoded::parse(&input[end..end + len])
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Captured {
        path,
        headers,
        form,
    }
}
async fn url(progress: &mut watch::Receiver<oauth::Progress>) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(url) = progress.borrow().authorization_url.clone() {
                return url;
            }
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
fn fields(url: &str) -> BTreeMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
fn redirect(url: &str, values: &[(&str, &str)]) -> String {
    let mut url = url::Url::parse(url).unwrap();
    for (k, v) in values {
        url.query_pairs_mut().append_pair(k, v);
    }
    url.into()
}
async fn credentials(server: &Provider) -> TokenSet {
    let (_, _, channels) = oauth::channels();
    oauth::acquire(
        server.config(Grant::ClientCredentials),
        channels,
        CancellationToken::new(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn pkce_loopback_accepts_matching_callback_and_exchanges_exact_verifier() {
    pkce_loopback("http://127.0.0.1:0/callback").await;
}
#[tokio::test]
async fn https_localhost_callback_uses_ephemeral_tls_and_releases_listener() {
    pkce_loopback("https://localhost:0/callback").await;
}
async fn pkce_loopback(callback: &str) {
    let provider = Provider::start().await;
    // Keep the manual sender alive while the listener owns this flow.
    let (sender, mut progress, channels) = oauth::channels();
    let mut config = provider.config(Grant::AuthorizationCode);
    config.redirect_url = callback.into();
    let task = tokio::spawn(oauth::acquire(config, channels, CancellationToken::new()));
    let authorization = url(&mut progress).await;
    let params = fields(&authorization);
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["scope"], "read write");
    let bad = redirect(
        &params["redirect_uri"],
        &[("code", "code"), ("state", "WRONG")],
    );
    let http = reqwest::Client::builder()
        // Only this isolated test client trusts the per-flow self-signed cert.
        .tls_danger_accept_invalid_certs(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = http.get(bad).send().await.unwrap();
    assert_eq!(response.status(), 400);
    assert!(provider.requests().is_empty());
    let good = redirect(
        &params["redirect_uri"],
        &[("code", "valid-code"), ("state", &params["state"])],
    );
    assert_eq!(http.get(good).send().await.unwrap().status(), 200);
    let tokens = task.await.unwrap().unwrap();
    drop(sender);
    assert!(!tokens.expired());
    assert!(tokens.can_refresh());
    let request = provider.requests().pop().unwrap();
    assert_eq!(request.form["code"], "valid-code");
    assert_eq!(request.form["redirect_uri"], params["redirect_uri"]);
    assert_eq!(
        URL_SAFE_NO_PAD.encode(Sha256::digest(request.form["code_verifier"].as_bytes())),
        params["code_challenge"]
    );
    let callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    let address = format!(
        "{}:{}",
        callback.host_str().unwrap(),
        callback.port().unwrap()
    );
    assert!(TcpStream::connect(address).await.is_err());
    assert!(!format!("{tokens:?}").contains("runtime-access"));
}
#[tokio::test]
async fn remote_pasted_redirect_rejects_wrong_uri_duplicates_and_state_then_succeeds() {
    let provider = Provider::start().await;
    let (sender, mut progress, channels) = oauth::channels();
    let mut config = provider.config(Grant::AuthorizationCode);
    config.redirect_url = "https://remote.example.test/callback?fixed=one".into();
    config.client_secret.clear();
    let task = tokio::spawn(oauth::acquire(config, channels, CancellationToken::new()));
    let params = fields(&url(&mut progress).await);
    for rejected in [
        redirect(
            "https://wrong.example.test/callback",
            &[("code", "x"), ("state", &params["state"])],
        ),
        redirect(
            &params["redirect_uri"],
            &[
                ("code", "x"),
                ("state", &params["state"]),
                ("state", &params["state"]),
            ],
        ),
        redirect(
            &params["redirect_uri"],
            &[("code", "x"), ("state", "wrong")],
        ),
    ] {
        sender.send(rejected).await.unwrap();
    }
    sender
        .send(redirect(
            &params["redirect_uri"],
            &[("code", "right"), ("state", &params["state"])],
        ))
        .await
        .unwrap();
    assert!(!task.await.unwrap().unwrap().expired());
    assert_eq!(provider.requests().len(), 1);
    let request = provider.requests().pop().unwrap();
    assert_eq!(request.form["client_id"], "fixture-client");
    assert!(!request.headers.contains_key("authorization"));
    assert!(!request.form.contains_key("client_secret"));
}
#[tokio::test]
async fn client_credentials_basic_and_body_auth_are_explicit_and_runtime_tokens_are_unpersisted() {
    let provider = Provider::start().await;
    for mode in [ClientAuthentication::Basic, ClientAuthentication::Body] {
        let mut config = provider.config(Grant::ClientCredentials);
        config.client_authentication = mode;
        assert!(!format!("{config:?}").contains("fixture-secret"));
        let (_, _, channels) = oauth::channels();
        let token = oauth::acquire(config, channels, CancellationToken::new())
            .await
            .unwrap();
        let auth = token.execution_auth().unwrap();
        assert_eq!(auth.value("accessToken"), "runtime-access");
        assert_eq!(token.scopes, ["read", "write"]);
        let request = provider.requests().pop().unwrap();
        assert_eq!(request.form["grant_type"], "client_credentials");
        if mode == ClientAuthentication::Basic {
            assert_eq!(
                request.headers["authorization"],
                format!(
                    "Basic {}",
                    STANDARD.encode(b"fixture-client:fixture-secret")
                )
            );
            assert!(!request.form.contains_key("client_secret"));
        } else {
            assert_eq!(request.form["client_secret"], "fixture-secret");
            assert!(!request.headers.contains_key("authorization"));
        }
    }
    // Runtime data has no serialization integration: acquiring does not modify
    // or insert fields into the caller's collection model.
    let model = kode_rest::model::Workspace::default();
    let before = model.export_collection();
    let _ = credentials(&provider).await;
    assert_eq!(model.export_collection(), before);
}
#[tokio::test]
async fn explicit_refresh_rotates_credentials_and_rejects_expired_runtime_tokens() {
    let provider = Provider::start().await;
    let token = credentials(&provider).await;
    let rotated = oauth::refresh(
        provider.config(Grant::ClientCredentials),
        token,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        rotated.execution_auth().unwrap().value("accessToken"),
        "rotated-access"
    );
    let request = provider.requests().pop().unwrap();
    assert_eq!(request.form["grant_type"], "refresh_token");
    assert_eq!(request.form["refresh_token"], "runtime-refresh");
    let mut config = provider.config(Grant::ClientCredentials);
    config.token_url = format!("{}/expired", provider.base);
    let (_, _, channels) = oauth::channels();
    let expired = oauth::acquire(config, channels, CancellationToken::new())
        .await
        .unwrap();
    assert!(expired.expired());
    assert!(expired.execution_auth().is_err());
}
#[tokio::test]
async fn oauth_endpoint_errors_bodies_and_redirects_never_leak_or_follow() {
    let provider = Provider::start().await;
    for path in ["reject", "bad-json", "huge", "redirect"] {
        let mut config = provider.config(Grant::ClientCredentials);
        config.token_url = format!("{}/{path}", provider.base);
        let (_, _, channels) = oauth::channels();
        let error = oauth::acquire(config, channels, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("NEVER-LOG-THIS-SECRET"));
    }
    assert_eq!(provider.requests().len(), 4);
}
#[tokio::test]
async fn cancelling_pending_callback_releases_its_listener_and_denial_is_terminal() {
    let provider = Provider::start().await;
    let (sender, mut progress, channels) = oauth::channels();
    let cancel = CancellationToken::new();
    let task = tokio::spawn(oauth::acquire(
        provider.config(Grant::AuthorizationCode),
        channels,
        cancel.clone(),
    ));
    let params = fields(&url(&mut progress).await);
    cancel.cancel();
    assert!(matches!(
        task.await.unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    drop(sender);
    let callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    assert!(
        TcpStream::connect((callback.host_str().unwrap(), callback.port().unwrap()))
            .await
            .is_err()
    );
    assert!(provider.requests().is_empty());
    let (sender, mut progress, channels) = oauth::channels();
    let task = tokio::spawn(oauth::acquire(
        provider.config(Grant::AuthorizationCode),
        channels,
        CancellationToken::new(),
    ));
    let params = fields(&url(&mut progress).await);
    sender
        .send(redirect(
            &params["redirect_uri"],
            &[("state", &params["state"]), ("error", "access_denied")],
        ))
        .await
        .unwrap();
    assert!(
        task.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("denied")
    );
}
#[tokio::test]
async fn device_pending_slow_down_and_success_respect_polling_intervals() {
    let provider = Provider::start().await;
    let (sender, mut progress, channels) = oauth::channels();
    let mut config = provider.config(Grant::DeviceCode);
    config.token_url = format!("{}/poll", provider.base);
    let start = Instant::now();
    let task = tokio::spawn(oauth::acquire(config, channels, CancellationToken::new()));
    let target = url(&mut progress).await;
    assert!(target.contains("/verify?user_code=ABCD-EFGH"));
    assert_eq!(progress.borrow().user_code.as_deref(), Some("ABCD-EFGH"));
    assert_eq!(
        task.await
            .unwrap()
            .unwrap()
            .execution_auth()
            .unwrap()
            .value("accessToken"),
        "device-access"
    );
    drop(sender);
    assert!(start.elapsed() >= Duration::from_secs(7));
    assert_eq!(
        provider
            .requests()
            .iter()
            .filter(|r| r.path == "/poll")
            .count(),
        3
    );
}
#[tokio::test]
async fn cancellation_and_timeout_cover_network_and_device_waits() {
    let provider = Provider::start().await;
    let mut config = provider.config(Grant::ClientCredentials);
    config.token_url = format!("{}/hang", provider.base);
    config.timeout = Duration::from_millis(30);
    let (_, _, channels) = oauth::channels();
    assert!(
        oauth::acquire(config, channels, CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    let (sender, mut progress, channels) = oauth::channels();
    let cancel = CancellationToken::new();
    let task = tokio::spawn(oauth::acquire(
        provider.config(Grant::DeviceCode),
        channels,
        cancel.clone(),
    ));
    url(&mut progress).await;
    cancel.cancel();
    assert!(matches!(
        task.await.unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    drop(sender);
}
#[tokio::test]
async fn oauth1_temporary_and_access_exchange_validate_callback_and_verifier() {
    let provider = Provider::start().await;
    let (sender, mut progress, channels) = oauth::channels();
    let mut config = provider.config(Grant::OAuth1);
    config.token_url = format!("{}/access-token", provider.base);
    let task = tokio::spawn(oauth::acquire(config, channels, CancellationToken::new()));
    let target = url(&mut progress).await;
    assert_eq!(fields(&target)["oauth_token"], "temporary");
    let callback = progress.borrow().callback_url.clone().unwrap();
    sender
        .send(redirect(
            &callback,
            &[("oauth_token", "wrong"), ("oauth_verifier", "bad")],
        ))
        .await
        .unwrap();
    sender
        .send(redirect(
            &callback,
            &[("oauth_token", "temporary"), ("oauth_verifier", "verified")],
        ))
        .await
        .unwrap();
    let token = task.await.unwrap().unwrap();
    let mut auth = ExecutionAuth {
        runtime_token: false,
        kind: "oauth1".into(),
        fields: BTreeMap::new(),
    };
    token.apply(&mut auth).unwrap();
    assert_eq!(auth.value("token"), "oauth1-access");
    assert_eq!(auth.value("tokenSecret"), "oauth1-secret");
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers["authorization"].contains("oauth_callback="));
    assert!(requests[1].headers["authorization"].contains("oauth_verifier=\"verified\""));
    assert!(requests[1].headers["authorization"].contains("oauth_token=\"temporary\""));
    assert!(!requests[1].headers["authorization"].contains("oauth_callback="));
}

#[tokio::test]
async fn acquired_token_executes_without_mutating_model_or_exposing_it_in_raw_preview() {
    use kode_core::{JobTag, RequestId, WorkspaceGeneration};
    use kode_rest::{
        http::{ExecutionKey, Executor},
        model::NodePath,
        resolve::RequestResolver,
    };
    let provider = Provider::start().await;
    let tokens = credentials(&provider).await;
    let collection = json!({"info":{},"auth":{"type":"oauth2","oauth2":[{"key":"accessToken","value":"{{not-defined}}"}]},"item":[{"request":{"url":format!("{}/protected",provider.base)}}]});
    let before = collection.clone();
    assert!(
        RequestResolver::new(
            &collection,
            NodePath(vec![0]),
            std::path::Path::new("/synthetic"),
            None
        )
        .unwrap()
        .plan()
        .is_err()
    );
    let prepared = RequestResolver::new(
        &collection,
        NodePath(vec![0]),
        std::path::Path::new("/synthetic"),
        None,
    )
    .unwrap()
    .with_runtime_tokens(tokens)
    .plan()
    .unwrap()
    .materialize(1024, &CancellationToken::new())
    .unwrap();
    let key = ExecutionKey {
        tag: JobTag {
            workspace: WorkspaceGeneration(1),
            request: RequestId::fresh().unwrap(),
            document: None,
        },
        item_id: "captured-item".into(),
    };
    let result = Executor::default()
        .execute(key, prepared, CancellationToken::new())
        .await
        .unwrap();
    assert!(result.is_success(), "{result:?}");
    assert_eq!(&*result.body, b"authorized");
    assert_eq!(collection, before);
    let preview = result.normalized_request.preview(1024).unwrap().text;
    assert!(preview.contains("[runtime OAuth credential]"));
    assert!(!preview.contains("runtime-access"));
    assert_eq!(
        provider.requests().pop().unwrap().headers["authorization"],
        "Bearer runtime-access"
    );
}

#[tokio::test]
async fn bound_tokens_resolve_inherited_settings_replace_postman_and_reject_changed_clients() {
    use kode_rest::{model::NodePath, oauth::BoundTokens, resolve::RequestResolver};
    let provider = Provider::start().await;
    let tokens = credentials(&provider).await;
    let mut collection = json!({"info":{},"auth":{"type":"oauth2","oauth2":[
        {"key":"clientId","value":"fixture-client"},{"key":"clientSecret","value":"fixture-secret"},
        {"key":"authUrl","value":format!("{}/authorize",provider.base)},
        {"key":"accessTokenUrl","value":format!("{}/token",provider.base)},
        {"key":"redirect_uri","value":"https://oauth.pstmn.io/v1/callback"}
    ]},"item":[{"request":{"url":format!("{}/protected",provider.base)}}]});
    let root = std::path::Path::new("/synthetic");
    let resolver = RequestResolver::new(&collection, NodePath(vec![0]), root, None).unwrap();
    let config = Config::from_resolver(&resolver).unwrap();
    assert_eq!(config.redirect_url, "https://localhost:0/callback");
    let bound = BoundTokens { config, tokens };
    assert!(bound.for_resolver(&resolver).is_ok());
    collection["auth"]["oauth2"][0]["value"] = json!("different-client");
    let changed = RequestResolver::new(&collection, NodePath(vec![0]), root, None).unwrap();
    assert!(
        bound
            .for_resolver(&changed)
            .unwrap_err()
            .to_string()
            .contains("settings changed")
    );
    let debug = format!("{bound:?}");
    assert!(!debug.contains("runtime-access") && !debug.contains("fixture-secret"));
}

#[tokio::test]
async fn cancelling_https_during_an_incomplete_tls_handshake_releases_the_socket() {
    let provider = Provider::start().await;
    let (_sender, mut progress, channels) = oauth::channels();
    let cancel = CancellationToken::new();
    let mut config = provider.config(Grant::AuthorizationCode);
    config.redirect_url = Config::default().redirect_url;
    let task = tokio::spawn(oauth::acquire(config, channels, cancel.clone()));
    let params = fields(&url(&mut progress).await);
    let callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    let address = ("127.0.0.1", callback.port().unwrap());
    let _partial = TcpStream::connect(address).await.unwrap();
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(TcpListener::bind(address).await.is_ok());
    assert!(provider.requests().is_empty());
}
