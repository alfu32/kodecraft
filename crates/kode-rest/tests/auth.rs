use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use http::{HeaderMap, HeaderValue};
use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    auth::{ExecutionAuth, SCHEMES, sign},
    http::{ExecutionKey, Executor, Failure},
    model::{NodePath, Workspace, editing},
    resolve::{PreparedRequest, RequestResolver},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
#[path = "support/websocket.rs"]
mod socket_server;
#[path = "support/http.rs"]
mod support;

fn auth(kind: &str, values: &[(&str, &str)]) -> ExecutionAuth {
    ExecutionAuth {
        runtime_token: false,
        kind: kind.into(),
        fields: values
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
    }
}
fn time(year: i32, month: time::Month, day: u8, hour: u8, minute: u8, second: u8) -> SystemTime {
    time::Date::from_calendar_date(year, month, day)
        .unwrap()
        .with_hms(hour, minute, second)
        .unwrap()
        .assume_utc()
        .into()
}
fn signed(
    auth: &ExecutionAuth,
    method: &str,
    url: &str,
    headers: &mut HeaderMap,
    body: &[u8],
    now: SystemTime,
    nonce: &str,
) -> String {
    sign(
        auth,
        method,
        &url.parse().unwrap(),
        headers,
        body,
        now,
        nonce,
    )
    .unwrap();
    headers["authorization"].to_str().unwrap().into()
}
#[test]
fn published_oauth1_rfc5849_duplicate_query_and_form_vector() {
    // RFC 5849 sections 3.4.1.1/3.4.2; public synthetic example keys.
    let a = auth(
        "oauth1",
        &[
            ("consumerKey", "9djdj82h48djs9d2"),
            ("consumerSecret", "j49sk3j29djd"),
            ("token", "kkk9d7dh3k39sjv7"),
            ("tokenSecret", "dh893hdasih9"),
        ],
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );
    let value = signed(
        &a,
        "POST",
        "http://example.com/request?b5=%3D%253D&a3=a&c%40=&a2=r%20b",
        &mut headers,
        b"c2&a3=2+q",
        UNIX_EPOCH + Duration::from_secs(137131201),
        "7d8f3e4a",
    );
    assert!(
        value.contains("oauth_signature=\"r6%2FTJjbCOr97%2F%2BUU0NsvSne7s5g%3D\""),
        "{value}"
    );
    assert!(headers["authorization"].is_sensitive());
}
#[test]
fn published_aws_get_vanilla_vector_and_payload_tamper() {
    // aws-sigv4 1.6.0 aws-signing-test-suite/v4/get-vanilla.
    let a = auth(
        "awsv4",
        &[
            ("accessKey", "AKIDEXAMPLE"),
            ("secretKey", "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY"),
            ("region", "us-east-1"),
            ("service", "service"),
        ],
    );
    let now = time(2015, time::Month::August, 30, 12, 36, 0);
    let expected = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31";
    assert_eq!(
        signed(
            &a,
            "GET",
            "https://example.amazonaws.com/",
            &mut HeaderMap::new(),
            b"",
            now,
            "unused"
        ),
        expected
    );
    assert_ne!(
        signed(
            &a,
            "GET",
            "https://example.amazonaws.com/",
            &mut HeaderMap::new(),
            b"tampered",
            now,
            "unused"
        ),
        expected
    );
}
#[test]
fn published_akamai_vectors_cover_query_headers_methods_and_body_limits() {
    let data: Value = serde_json::from_str(include_str!(
        "../../../fixtures/rest/auth/edgegrid/testdata.json"
    ))
    .unwrap();
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../fixtures/rest/auth/edgegrid/testcases.json"
    ))
    .unwrap();
    let fields: BTreeMap<_, _> = [
        ("clientToken", data["client_token"].as_str().unwrap().into()),
        (
            "clientSecret",
            data["client_secret"].as_str().unwrap().into(),
        ),
        ("accessToken", data["access_token"].as_str().unwrap().into()),
        ("maxBody", data["max_body"].to_string()),
        (
            "headersToSign",
            data["headers_to_sign"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(","),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect();
    let a = ExecutionAuth {
        runtime_token: false,
        kind: "edgegrid".into(),
        fields,
    };
    let mut checked = 0;
    for case in cases {
        // Upstream's one leading-whitespace vector is rejected by its transport;
        // it has no expected signature. Our resolver rejects the same HTTP input.
        if case.get("failsWithMessage").is_some() {
            continue;
        }
        let request = &case["request"];
        let mut headers = HeaderMap::new();
        for object in request["headers"].as_array().into_iter().flatten() {
            for (k, v) in object.as_object().unwrap() {
                headers.insert(
                    http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    HeaderValue::from_str(v.as_str().unwrap()).unwrap(),
                );
            }
        }
        let url = format!(
            "{}{}",
            data["base_url"].as_str().unwrap().trim_end_matches('/'),
            request["path"].as_str().unwrap()
        );
        let value = signed(
            &a,
            request["method"].as_str().unwrap(),
            &url,
            &mut headers,
            request["data"].as_str().unwrap_or("").as_bytes(),
            time(2014, time::Month::March, 21, 19, 34, 21),
            data["nonce"].as_str().unwrap(),
        );
        assert_eq!(
            value,
            case["expectedAuthorization"].as_str().unwrap(),
            "{}",
            case["testName"]
        );
        checked += 1;
    }
    assert_eq!(checked, 14);
}
#[test]
fn hawk_payload_and_uri_are_bound_to_signature_and_secret_debug_is_redacted() {
    use std::str::FromStr;
    let a = auth(
        "hawk",
        &[
            ("authId", "test-client"),
            ("authKey", "synthetic-secret"),
            ("algorithm", "sha256"),
        ],
    );
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    let value = signed(
        &a,
        "POST",
        "https://example.test:8443/path?q=1",
        &mut headers,
        b"unicode \xe7\x95\x8c",
        SystemTime::now(),
        "fresh-nonce",
    );
    let header = hawk::Header::from_str(value.strip_prefix("Hawk ").unwrap()).unwrap();
    let hash =
        hawk::PayloadHasher::hash("text/plain", hawk::SHA256, b"unicode \xe7\x95\x8c").unwrap();
    let key = hawk::Key::new("synthetic-secret", hawk::SHA256).unwrap();
    assert!(
        hawk::RequestBuilder::new("POST", "example.test", 8443, "/path?q=1")
            .hash(&hash[..])
            .request()
            .validate_header(&header, &key, Duration::from_secs(60))
    );
    assert!(
        !hawk::RequestBuilder::new("POST", "example.test", 8443, "/wrong")
            .hash(&hash[..])
            .request()
            .validate_header(&header, &key, Duration::from_secs(60))
    );
    assert_eq!(header.hash.unwrap(), hash);
    assert!(!format!("{a:?}").contains("synthetic-secret"));
}
fn plan(kind: &str, url: &str, fields: &[(&str, &str)]) -> PreparedRequest {
    let entries: Vec<_> = fields
        .iter()
        .map(|(k, v)| json!({"key":k,"value":v,"type":"string"}))
        .collect();
    let mut auth = json!({"type":kind});
    auth[kind] = json!(entries);
    let collection = json!({"info":{},"auth":auth,"item":[{"name":"call","request":{"method":"POST","url":url,"body":{"mode":"raw","raw":"actual body"}}}]});
    RequestResolver::new(
        &collection,
        NodePath(vec![0]),
        Path::new("/synthetic"),
        None,
    )
    .unwrap()
    .plan()
    .unwrap()
    .materialize(1024, &CancellationToken::new())
    .unwrap()
}
fn key() -> ExecutionKey {
    ExecutionKey {
        tag: JobTag {
            workspace: WorkspaceGeneration(1),
            request: RequestId::fresh().unwrap(),
            document: None,
        },
        item_id: "auth-fixture".into(),
    }
}
#[test]
fn all_screenshot_choices_preserve_inheritance_fields_and_unknown_data() {
    let mut model = Workspace::default();
    model.replace_collection(json!({"info":{},"x-unknown":{"keep":42},"item":[{"name":"call","request":{"url":"https://example.test/"}}]})).unwrap();
    for scheme in SCHEMES {
        assert!(editing::auth(&mut model, &NodePath(vec![]), Some(scheme.kind)).unwrap());
        let resolver = RequestResolver::new(
            model.collection(),
            NodePath(vec![0]),
            Path::new("/synthetic"),
            None,
        )
        .unwrap();
        assert_eq!(
            resolver.effective_auth().unwrap().unwrap().kind,
            scheme.kind
        );
        if scheme.kind != "noauth" {
            assert_eq!(
                model.collection()["auth"][scheme.kind]
                    .as_array()
                    .unwrap()
                    .len(),
                scheme.fields.len() + kode_rest::oauth::configuration_fields(scheme.kind).len()
            );
        }
        assert_eq!(model.collection()["x-unknown"]["keep"], 42);
    }
    assert_eq!(SCHEMES.len(), 11);
    editing::auth(&mut model, &NodePath(vec![0]), Some("noauth")).unwrap();
    assert!(
        RequestResolver::new(
            model.collection(),
            NodePath(vec![0]),
            Path::new("/synthetic"),
            None
        )
        .unwrap()
        .plan()
        .unwrap()
        .auth
        .is_none()
    );
    for name in [
        "password",
        "consumerSecret",
        "tokenSecret",
        "accessToken",
        "sessionToken",
        "authKey",
        "clientSecret",
        "value",
    ] {
        assert!(kode_rest::auth::secret_field(name));
    }
}
async fn read(stream: &mut TcpStream) -> (String, Vec<u8>) {
    let mut bytes = vec![];
    let mut buffer = [0u8; 1024];
    let end = loop {
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(at) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let len = head
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < end + len {
        let n = stream.read(&mut buffer).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]);
    }
    (head, bytes[end..end + len].to_vec())
}
fn authorization(head: &str) -> &str {
    head.lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .map(|(_, v)| v.trim())
        })
        .unwrap()
}
#[tokio::test]
async fn digest_real_post_401_exchange_keeps_body_and_verifies_password_proof() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/digest?x=1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = read(&mut stream).await;
        assert!(!head.to_ascii_lowercase().contains("authorization:"));
        assert_eq!(body, b"actual body");
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"fixture\", nonce=\"server-nonce\", algorithm=MD5, qop=\"auth\"\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        let (head, body) = read(&mut stream).await;
        assert_eq!(body, b"actual body");
        let value = authorization(&head);
        assert!(value.starts_with("Digest "));
        assert!(!value.contains("synthetic-password"));
        let fields: BTreeMap<_, _> = value[7..]
            .split(',')
            .map(|s| {
                let (k, v) = s.trim().split_once('=').unwrap();
                (k, v.trim_matches('"'))
            })
            .collect();
        use md5::{Digest, Md5};
        let hash = |s: &str| format!("{:x}", Md5::digest(s.as_bytes()));
        let ha1 = hash("user:fixture:synthetic-password");
        let ha2 = hash("POST:/digest?x=1");
        let expected = hash(&format!(
            "{ha1}:server-nonce:{}:{}:auth:{ha2}",
            fields["nc"], fields["cnonce"]
        ));
        assert_eq!(fields["response"], expected);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
    });
    let result = Executor::default()
        .execute(
            key(),
            plan(
                "digest",
                &url,
                &[("username", "user"), ("password", "synthetic-password")],
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.is_success(), "{result:?}");
    assert_eq!(&*result.body, b"ok");
    server.await.unwrap();
}
#[tokio::test]
async fn ntlm_real_same_connection_exchange_verifies_ntlmv2_proof() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/ntlm", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (head, body) = read(&mut stream).await;
        assert_eq!(body, b"actual body");
        let first = STANDARD
            .decode(authorization(&head).strip_prefix("NTLM ").unwrap())
            .unwrap();
        assert!(matches!(
            ntlmclient::Message::try_from(&first[..]).unwrap(),
            ntlmclient::Message::Negotiate(_)
        ));
        let challenge = ntlmclient::Message::Challenge(ntlmclient::ChallengeMessage {
            target_name: "DOMAIN".into(),
            flags: ntlmclient::Flags::NEGOTIATE_UNICODE
                | ntlmclient::Flags::NEGOTIATE_NTLM
                | ntlmclient::Flags::NEGOTIATE_TARGET_INFO,
            challenge: [1, 2, 3, 4, 5, 6, 7, 8],
            context: (0, 0),
            target_information: vec![],
            os_version: Default::default(),
        });
        let header = format!(
            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM {}\r\nContent-Length: 0\r\n\r\n",
            STANDARD.encode(challenge.to_bytes().unwrap())
        );
        stream.write_all(header.as_bytes()).await.unwrap();
        let (head, body) = read(&mut stream).await;
        assert_eq!(body, b"actual body");
        let bytes = STANDARD
            .decode(authorization(&head).strip_prefix("NTLM ").unwrap())
            .unwrap();
        let ntlmclient::Message::Authenticate(message) =
            ntlmclient::Message::try_from(&bytes[..]).unwrap()
        else {
            panic!("expected authenticate")
        };
        assert_eq!(message.user_name, "User");
        assert_eq!(message.domain_name, "DOMAIN");
        assert!(message.ntlm_response.len() > 32);
        let credentials = ntlmclient::Credentials {
            username: "User".into(),
            password: "synthetic-password".into(),
            domain: "DOMAIN".into(),
        };
        let response_key = ntlmclient::ntlm_v2_password_func(&credentials);
        let mut mac = Hmac::<md5::Md5>::new_from_slice(&response_key).unwrap();
        mac.update(&[1, 2, 3, 4, 5, 6, 7, 8]);
        mac.update(&message.ntlm_response[16..]);
        mac.verify_slice(&message.ntlm_response[..16]).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
    });
    let result = Executor::default()
        .execute(
            key(),
            plan(
                "ntlm",
                &url,
                &[
                    ("username", "User"),
                    ("password", "synthetic-password"),
                    ("domain", "DOMAIN"),
                    ("workstation", "fixture"),
                ],
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(result.is_success(), "{result:?}");
    server.await.unwrap();
}
#[tokio::test]
async fn malformed_authentication_challenge_is_bounded_and_keeps_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read(&mut stream).await;
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: NTLM not-base64!\r\nContent-Length: 0\r\n\r\n").await.unwrap();
    });
    let result = Executor::default()
        .execute(
            key(),
            plan("ntlm", &url, &[("username", "User")]),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.error, Some(Failure::Authentication));
    assert_eq!(result.status_code, Some(401));
    server.await.unwrap();
}

#[tokio::test]
async fn static_signers_reach_actual_transport_with_exact_body_and_runtime_headers() {
    use std::str::FromStr;
    let server = support::Server::start(false).await;
    type WireCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
    let choices: [WireCase<'_>; 5] = [
        (
            "oauth1",
            &[
                ("consumerKey", "consumer"),
                ("consumerSecret", "secret"),
                ("token", "token"),
                ("tokenSecret", "token-secret"),
            ],
            "OAuth ",
        ),
        (
            "oauth2",
            &[("accessToken", "synthetic-token")],
            "Bearer synthetic-token",
        ),
        (
            "hawk",
            &[("authId", "fixture"), ("authKey", "synthetic-key")],
            "Hawk ",
        ),
        (
            "awsv4",
            &[
                ("accessKey", "AKIDEXAMPLE"),
                ("secretKey", "synthetic-secret"),
                ("region", "us-east-1"),
                ("service", "s3"),
            ],
            "AWS4-HMAC-SHA256 ",
        ),
        (
            "edgegrid",
            &[
                ("clientToken", "client"),
                ("clientSecret", "synthetic-secret"),
                ("accessToken", "access"),
            ],
            "EG1-HMAC-SHA256 ",
        ),
    ];
    for (kind, fields, prefix) in choices {
        let result = Executor::default()
            .execute(
                key(),
                plan(kind, &format!("{}/reflect?q=one", server.base), fields),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(result.is_success(), "{kind}: {result:?}");
        assert_eq!(&*result.body, b"actual body");
        let request = server.captures().pop().unwrap();
        let authorization = request.values("authorization")[0];
        assert!(authorization.starts_with(prefix));
        assert!(
            result
                .normalized_request
                .request
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("authorization") && h.value == authorization)
        );
        if kind == "hawk" {
            let header =
                hawk::Header::from_str(authorization.strip_prefix("Hawk ").unwrap()).unwrap();
            let hash =
                hawk::PayloadHasher::hash("text/plain", hawk::SHA256, &request.body).unwrap();
            let parsed = url::Url::parse(&server.base).unwrap();
            assert!(
                hawk::RequestBuilder::new(
                    &request.method,
                    "127.0.0.1",
                    parsed.port().unwrap(),
                    &request.path
                )
                .hash(&hash[..])
                .request()
                .validate_header(
                    &header,
                    &hawk::Key::new("synthetic-key", hawk::SHA256).unwrap(),
                    Duration::from_secs(60)
                )
            );
        }
        if kind == "awsv4" {
            use sha2::{Digest, Sha256};
            assert_eq!(
                request.values("x-amz-content-sha256"),
                [format!("{:x}", Sha256::digest(&request.body)).as_str()]
            );
        }
    }
}

#[tokio::test]
async fn authenticated_redirect_is_visible_without_forwarding_credentials() {
    let source = support::Server::start(false).await;
    let target = support::Server::start(false).await;
    let url = format!("{}/redirect-to?url={}/echo", source.base, target.base);
    let result = Executor::default()
        .execute(
            key(),
            plan("oauth2", &url, &[("accessToken", "must-not-forward")]),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.status_code, Some(302));
    assert!(target.captures().is_empty());
    assert!(
        result
            .normalizations
            .iter()
            .any(|n| n.reason.contains("explicit review"))
    );
}

#[tokio::test]
async fn cancellation_interrupts_authentication_exchange() {
    let server = support::Server::start(false).await;
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let worker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        signal.cancel();
    });
    let result = Executor::default()
        .execute(
            key(),
            plan(
                "digest",
                &format!("{}/hang", server.base),
                &[("username", "fixture")],
            ),
            cancel,
        )
        .await
        .unwrap();
    assert_eq!(result.error, Some(Failure::Cancelled));
    worker.await.unwrap();
}

#[test]
fn invalid_credentials_algorithms_and_variables_fail_before_network() {
    for a in [
        auth("oauth2", &[]),
        auth("awsv4", &[("accessKey", "key")]),
        auth(
            "hawk",
            &[
                ("authId", "id"),
                ("authKey", "key"),
                ("algorithm", "unknown"),
            ],
        ),
        auth(
            "oauth1",
            &[
                ("consumerKey", "key"),
                ("consumerSecret", "secret"),
                ("signatureMethod", "not-a-method"),
            ],
        ),
        auth(
            "edgegrid",
            &[
                ("clientToken", "token"),
                ("accessToken", "token"),
                ("clientSecret", "secret"),
                ("maxBody", "invalid"),
            ],
        ),
        auth("ntlm", &[("username", "line\r\nbreak")]),
    ] {
        assert!(a.validate().is_err());
    }
    let c = json!({"info":{},"auth":{"type":"oauth2","oauth2":[{"key":"accessToken","value":"{{missing}}"}]},"item":[{"request":{"url":"http://127.0.0.1:9/"}}]});
    assert!(
        RequestResolver::new(&c, NodePath(vec![0]), Path::new("/synthetic"), None)
            .unwrap()
            .plan()
            .is_err()
    );
    let c = json!({"info":{},"auth":{"type":"oauth2","oauth2":[{"key":"accessToken","value":"token"}]},"item":[{"request":{"url":"https://username:password@example.test/"}}]});
    assert!(
        RequestResolver::new(&c, NodePath(vec![0]), Path::new("/synthetic"), None)
            .unwrap()
            .plan()
            .unwrap_err()
            .to_string()
            .contains("userinfo")
    );
}

#[tokio::test]
async fn digest_sse_uses_shared_challenge_path_before_incremental_delivery() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("sse://{}/events", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read(&mut stream).await;
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"ignored\", Digest realm=\"fixture\", nonce=\"nonce\", qop=\"auth\"\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        let (head, body) = read(&mut stream).await;
        assert!(authorization(&head).starts_with("Digest "));
        assert_eq!(body, b"actual body");
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 10\r\n\r\ndata: ok\n\n").await.unwrap();
    });
    let result = kode_rest::stream::Executor::default()
        .execute(
            key(),
            plan(
                "digest",
                &url,
                &[("username", "user"), ("password", "password")],
            ),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(result.is_success(), "{result:?}");
    assert_eq!(&*result.body, b"data: ok\n\n");
    server.await.unwrap();
}

#[tokio::test]
async fn websocket_runtime_signers_use_get_empty_body_and_challenge_modes_fail_explicitly() {
    use std::str::FromStr;
    let server = socket_server::Server::start(false).await;
    for a in [
        auth(
            "oauth2",
            &[("accessToken", "token"), ("headerPrefix", "Bearer")],
        ),
        auth(
            "hawk",
            &[
                ("authId", "fixture"),
                ("authKey", "key"),
                ("algorithm", "sha256"),
            ],
        ),
    ] {
        let request = plan(
            &a.kind,
            &format!("{}/messages", server.base),
            &a.fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect::<Vec<_>>(),
        );
        let result = kode_rest::stream::Executor::default()
            .execute(key(), request, CancellationToken::new(), None)
            .await
            .unwrap();
        assert!(result.is_success(), "{result:?}");
        assert_eq!(result.normalized_request.request.method, "GET");
        assert!(result.normalized_request.body.is_empty());
        let captured = server.headers.lock().unwrap();
        let headers = captured.last().unwrap();
        let value = &headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .unwrap()
            .1;
        if a.kind == "oauth2" {
            assert_eq!(value, "Bearer token");
        } else {
            let header = hawk::Header::from_str(value.strip_prefix("Hawk ").unwrap()).unwrap();
            let hash = hawk::PayloadHasher::hash("text/plain", hawk::SHA256, b"").unwrap();
            let url = url::Url::parse(&server.base).unwrap();
            assert!(
                hawk::RequestBuilder::new("GET", "127.0.0.1", url.port().unwrap(), "/messages")
                    .hash(&hash[..])
                    .request()
                    .validate_header(
                        &header,
                        &hawk::Key::new("key", hawk::SHA256).unwrap(),
                        Duration::from_secs(60)
                    )
            );
        }
    }
    assert!(server.incoming.lock().unwrap().is_empty());
    let before = server.headers.lock().unwrap().len();
    let result = kode_rest::stream::Executor::default()
        .execute(
            key(),
            plan(
                "ntlm",
                &format!("{}/messages", server.base),
                &[("username", "user")],
            ),
            CancellationToken::new(),
            None,
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("Digest/NTLM"));
    assert_eq!(server.headers.lock().unwrap().len(), before);
}
