#[path = "support/http.rs"]
mod support;
use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    http::{ExecutionKey, Executor, Failure},
    model::NodePath,
    resolve::{BodyPlan, Header, PreparedRequest, Protocol, RequestResolver, ResolvedRequest},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use support::Server;
fn key(name: &str) -> ExecutionKey {
    ExecutionKey {
        tag: JobTag {
            workspace: WorkspaceGeneration(7),
            request: RequestId::fresh().unwrap(),
            document: None,
        },
        item_id: name.into(),
    }
}
fn prepared(url: String) -> PreparedRequest {
    PreparedRequest {
        request: ResolvedRequest {
            path: NodePath(vec![0]),
            method: "GET".into(),
            url,
            headers: vec![],
            body: BodyPlan::Empty,
            timeout_ms: 2000,
            follow_redirects: true,
            ssl_validation: true,
            protocol: Protocol::Http,
        },
        body: Arc::from([]),
    }
}
async fn execute(executor: &Executor, prepared: PreparedRequest) -> kode_rest::http::Response {
    executor
        .execute(key("saved-request"), prepared, CancellationToken::new())
        .await
        .unwrap()
}
#[tokio::test]
async fn original_executes_captures_duplicate_headers_and_carries_cookies() {
    let server = Server::start(false).await;
    let executor = Executor::new(1024).unwrap();
    let first = execute(&executor, prepared(format!("{}/set", server.base))).await;
    assert_eq!(first.status_code, Some(200));
    assert_eq!(
        first
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("X-Duplicate"))
            .count(),
        2
    );
    assert_eq!(first.cookies.len(), 1);
    assert_eq!(first.content_type.as_deref(), Some("application/json"));
    assert_eq!(first.body_text(), r#"{"ok":true}"#);
    let second = execute(&executor, prepared(format!("{}/echo", server.base))).await;
    assert_eq!(second.body_text(), "session=abc");
    assert_eq!(executor.cookies().snapshot()[0].name, "session");
    assert!(executor.cookies().snapshot()[0].http_only);
    assert!(
        second
            .normalized_request
            .preview(1024)
            .unwrap()
            .text
            .contains("session=abc")
    );
    executor.clear_cookies();
    assert_eq!(
        execute(&executor, prepared(format!("{}/echo", server.base)))
            .await
            .body_text(),
        ""
    );
    assert!(Executor::default().cookies().snapshot().is_empty());
}
#[tokio::test]
async fn original_response_preview_truncation_counts_all_bytes_and_zero_limit_is_valid() {
    let server = Server::start(false).await;
    let response = execute(
        &Executor::new(32).unwrap(),
        prepared(format!("{}/large", server.base)),
    )
    .await;
    assert_eq!(response.received_bytes, 1000);
    assert_eq!(response.body.len(), 32);
    assert!(response.truncated);
    assert!(response.is_success());
    let zero = execute(
        &Executor::new(0).unwrap(),
        prepared(format!("{}/large", server.base)),
    )
    .await;
    assert_eq!(zero.received_bytes, 1000);
    assert!(zero.body.is_empty());
    assert!(zero.truncated);
    assert!(Executor::new(64 * 1024 * 1024 + 1).is_err());
}
#[tokio::test]
async fn original_managed_headers_are_normalized_and_duplicate_application_headers_reach_server() {
    let server = Server::start(false).await;
    let mut request = prepared(format!("{}/reflect", server.base));
    request.request.method = "POST".into();
    request.body = Arc::from(&b"hello"[..]);
    request.request.body = BodyPlan::Bytes(request.body.clone());
    for (name, value) in [
        ("Connection", "keep-alive"),
        ("Host", "example.invalid"),
        ("Content-Length", "999"),
        ("Expect", "100-continue"),
        ("Upgrade", "websocket"),
        ("Transfer-Encoding", "chunked"),
        ("X-Test", "one"),
        ("X-Test", "two"),
    ] {
        request.request.headers.push(Header {
            name: name.into(),
            value: value.into(),
        });
    }
    let response = execute(&Executor::default(), request.clone()).await;
    assert_eq!(response.body_text(), "hello");
    assert!(response.is_success());
    assert_eq!(response.normalizations.len(), 6);
    let capture = &server.captures()[0];
    assert_eq!(capture.method, "POST");
    assert_eq!(capture.values("Content-Length"), ["5"]);
    assert!(!capture.values("Host").contains(&"example.invalid"));
    assert!(capture.values("Expect").is_empty());
    assert!(capture.values("Upgrade").is_empty());
    assert!(capture.values("Transfer-Encoding").is_empty());
    assert_eq!(capture.values("X-Test"), ["one", "two"]);
    assert_eq!(request.request.headers[2].value, "999");
    let preview = response.normalized_request.preview(1024).unwrap();
    assert!(preview.text.contains("Content-Length: 5"));
    assert!(!preview.text.contains("example.invalid"));
    assert!(!preview.text.to_ascii_lowercase().contains("upgrade:"));
}
#[tokio::test]
async fn redirects_obey_setting_carry_intermediate_cookies_and_preserve_body_on_307() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let response = execute(&executor, prepared(format!("{}/redirect", server.base))).await;
    assert!(response.is_success());
    assert_eq!(response.final_url, format!("{}/set", server.base));
    assert!(
        executor
            .cookies()
            .snapshot()
            .iter()
            .any(|cookie| cookie.name == "redirect")
    );
    assert!(
        server.captures()[1]
            .values("Cookie")
            .iter()
            .any(|value| value.contains("redirect=yes"))
    );
    let mut stopped = prepared(format!("{}/redirect", server.base));
    stopped.request.follow_redirects = false;
    let response = execute(&executor, stopped).await;
    assert_eq!(response.status_code, Some(302));
    let mut post = prepared(format!("{}/redirect307", server.base));
    post.request.method = "POST".into();
    post.body = Arc::from(&b"redirect payload"[..]);
    post.request.body = BodyPlan::Bytes(post.body.clone());
    let response = execute(&executor, post).await;
    assert_eq!(response.body_text(), "redirect payload");
    let captures = server.captures();
    assert_eq!(captures.last().unwrap().method, "POST");
}
#[tokio::test]
async fn redirect_loops_are_errors_and_cross_origin_drops_auth_and_source_cookies() {
    let source = Server::start(false).await;
    let destination = Server::start(false).await;
    let executor = Executor::default();
    let response = execute(&executor, prepared(format!("{}/loop", source.base))).await;
    assert_eq!(response.error, Some(Failure::Redirect));
    assert!(source.captures().len() <= 12);
    execute(&executor, prepared(format!("{}/set", source.base))).await;
    let destination_url = format!("{}/ok", destination.base.replace("127.0.0.1", "localhost"));
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &destination_url)
        .finish();
    let mut request = prepared(format!("{}/redirect-to?{query}", source.base));
    request.request.headers.push(Header {
        name: "Authorization".into(),
        value: "Bearer synthetic-secret".into(),
    });
    let response = execute(&executor, request).await;
    assert!(response.is_success());
    let captured = &destination.captures()[0];
    assert!(captured.values("Authorization").is_empty());
    assert!(captured.values("Cookie").is_empty());
}
#[tokio::test]
async fn explicit_cookie_overrides_jar_and_url_basic_auth_is_in_normalized_preview() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    execute(&executor, prepared(format!("{}/set", server.base))).await;
    let mut request = prepared(format!(
        "{}/echo",
        server.base.replace("http://", "http://user:pass@")
    ));
    request.request.headers.push(Header {
        name: "Cookie".into(),
        value: "manual=yes".into(),
    });
    let response = execute(&executor, request).await;
    assert_eq!(response.body_text(), "manual=yes");
    let captured = server.captures();
    assert_eq!(
        captured.last().unwrap().values("Authorization"),
        ["Basic dXNlcjpwYXNz"]
    );
    assert!(
        response
            .normalized_request
            .request
            .headers
            .iter()
            .any(
                |h| h.name.eq_ignore_ascii_case("Authorization") && h.value == "Basic dXNlcjpwYXNz"
            )
    );
}
#[tokio::test]
async fn non_success_status_raw_binary_and_split_utf8_are_response_data() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let response = execute(&executor, prepared(format!("{}/status", server.base))).await;
    assert_eq!(response.status_code, Some(418));
    assert!(!response.is_success());
    assert_eq!(response.error, None);
    assert_eq!(response.body_text(), "teapot");
    let response = execute(&executor, prepared(format!("{}/binary", server.base))).await;
    assert_eq!(&*response.body, &[0, 255, 0xc3, 0xa9]);
    let response = execute(&executor, prepared(format!("{}/chunked", server.base))).await;
    assert_eq!(response.body_text(), "é");
    assert_eq!(response.received_bytes, 2);
    assert_eq!(
        &*response
            .headers
            .iter()
            .find(|h| h.name == "x-binary")
            .unwrap()
            .value,
        &[0x80]
    );
}
#[tokio::test]
async fn body_failure_timeout_and_cancel_retain_partial_response_with_original_identity() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let broken = execute(&executor, prepared(format!("{}/broken", server.base))).await;
    assert!(broken.error.is_some());
    assert_eq!(broken.status_code, Some(200));
    assert_eq!(broken.body_text(), "abc");
    assert_eq!(broken.received_bytes, 3);
    let mut slow = prepared(format!("{}/slow", server.base));
    slow.request.timeout_ms = 150;
    let timeout = execute(&executor, slow.clone()).await;
    assert_eq!(timeout.error, Some(Failure::Timeout));
    assert_eq!(timeout.body_text(), "abc");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    slow.request.timeout_ms = 2000;
    let identity = key("original-item");
    let cancellation = async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        token.cancel();
    };
    let (response, ()) = tokio::join!(
        executor.execute(identity.clone(), slow, cancel),
        cancellation
    );
    let response = response.unwrap();
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert_eq!(response.key, identity);
    assert_eq!(response.body_text(), "abc");
}
#[tokio::test]
async fn timeout_before_headers_cancel_before_send_and_connection_failure_are_explicit_and_never_replayed()
 {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let mut request = prepared(format!("{}/hang", server.base));
    request.request.timeout_ms = 80;
    request.request.method = "POST".into();
    let response = execute(&executor, request.clone()).await;
    assert_eq!(response.error, Some(Failure::Timeout));
    assert_eq!(response.status_code, None);
    assert_eq!(server.captures().len(), 1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let response = executor
        .execute(key("cancelled"), request, cancel)
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert_eq!(server.captures().len(), 1);
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let response = execute(
        &executor,
        prepared(format!("http://{address}/secret-token")),
    )
    .await;
    assert_eq!(response.error, Some(Failure::Connect));
    assert!(!format!("{response:?}").contains("secret-token"));
    assert!(!response.error.unwrap().message().contains("secret-token"));
}
#[tokio::test]
async fn invalid_transport_inputs_fail_before_network_and_do_not_echo_secrets() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    for (name, value) in [
        ("bad name", "synthetic-secret"),
        ("X-Test", "synthetic-secret\r\nInjected: yes"),
        ("Host", "synthetic-secret\n"),
    ] {
        let mut request = prepared(format!("{}/ok", server.base));
        request.request.headers.push(Header {
            name: name.into(),
            value: value.into(),
        });
        let error = executor
            .execute(key("invalid"), request, CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("synthetic-secret"));
    }
    for url in ["file:///tmp/private", "not-a-url"] {
        assert!(
            executor
                .execute(
                    key("invalid"),
                    prepared(url.into()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
    }
    let mut request = prepared(format!("{}/ok", server.base));
    request.request.method = "bad method".into();
    assert!(
        executor
            .execute(key("invalid"), request, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(server.captures().is_empty());
}

#[tokio::test]
async fn outgoing_and_incoming_header_budgets_are_explicit() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let mut request = prepared(format!("{}/ok", server.base));
    request.request.headers = (0..257)
        .map(|index| Header {
            name: format!("X-{index}"),
            value: "v".into(),
        })
        .collect();
    assert!(
        executor
            .execute(key("too-many"), request, CancellationToken::new())
            .await
            .is_err()
    );
    let mut request = prepared(format!("{}/ok", server.base));
    request.request.headers.push(Header {
        name: "X-Large".into(),
        value: "x".repeat(270 * 1024),
    });
    assert!(
        executor
            .execute(key("too-large"), request, CancellationToken::new())
            .await
            .is_err()
    );
    assert!(server.captures().is_empty());
    let response = execute(&executor, prepared(format!("{}/huge-header", server.base))).await;
    assert_eq!(response.error, Some(Failure::ResponseHeadersTooLarge));
    assert!(response.headers.is_empty());
    assert!(response.body.is_empty());
}
#[tokio::test]
async fn tls_validation_is_per_request_and_https_downgrade_is_stopped() {
    let secure = Server::start(true).await;
    let plain = Server::start(false).await;
    let executor = Executor::default();
    let response = execute(&executor, prepared(format!("{}/ok", secure.base))).await;
    assert_eq!(response.error, Some(Failure::Connect));
    let mut request = prepared(format!("{}/ok", secure.base));
    request.request.ssl_validation = false;
    let response = execute(&executor, request).await;
    assert!(response.is_success());
    let response = execute(&executor, prepared(format!("{}/ok", secure.base))).await;
    assert_eq!(response.error, Some(Failure::Connect));
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &format!("{}/ok", plain.base))
        .finish();
    let mut request = prepared(format!("{}/redirect-to?{query}", secure.base));
    request.request.ssl_validation = false;
    let response = execute(&executor, request).await;
    assert_eq!(response.status_code, Some(302));
    assert!(plain.captures().is_empty());
}
#[tokio::test]
async fn every_resolver_body_mode_reaches_loopback_as_the_captured_materialized_bytes() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("payload.bin"), [0, 1, 255]).unwrap();
    for body in [
        json!({"mode":"raw","raw":"é","options":{"raw":{"language":"text"}}}),
        json!({"mode":"urlencoded","urlencoded":[{"key":"a","value":"one two"},{"key":"a","value":"three"}]}),
        json!({"mode":"file","file":{"src":"payload.bin"}}),
        json!({"mode":"formdata","formdata":[{"key":"text","value":"hello","type":"text"},{"key":"file","src":"payload.bin","type":"file"}]}),
        json!({"mode":"graphql","graphql":{"query":"query { name }","variables":{"id":7}}}),
    ] {
        let collection = json!({"info":{},"item":[{"request":{"method":"POST","url":format!("{}/reflect",server.base),"body":body}}]});
        let prepared = RequestResolver::new(&collection, NodePath(vec![0]), dir.path(), None)
            .unwrap()
            .plan()
            .unwrap()
            .materialize(64 * 1024, &CancellationToken::new())
            .unwrap();
        let expected = prepared.body.clone();
        let response = execute(&executor, prepared).await;
        assert!(response.is_success());
        assert_eq!(response.body, expected);
    }
    assert_eq!(server.captures().len(), 5);
}
#[tokio::test(flavor = "current_thread")]
async fn network_wait_keeps_same_thread_available_for_edits_and_request_switches() {
    let server = Server::start(false).await;
    let executor = Executor::default();
    let cancel = CancellationToken::new();
    let request = prepared(format!("{}/hang", server.base));
    let original = key("original");
    let job = {
        let cancel = cancel.clone();
        let original = original.clone();
        tokio::spawn(async move { executor.execute(original, request, cancel).await })
    };
    let mut workspace = kode_rest::model::Workspace::default();
    let path = workspace
        .add_request(&NodePath::default(), "second")
        .unwrap();
    for index in 0..10 {
        tokio::time::sleep(Duration::from_millis(2)).await;
        workspace.rename(&path, &format!("typing {index}")).unwrap();
        workspace.select(&path);
    }
    assert!(!job.is_finished());
    cancel.cancel();
    let response = job.await.unwrap().unwrap();
    assert_eq!(response.key, original);
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert_eq!(workspace.node(&path).unwrap()["name"], "typing 9");
    assert_eq!(server.captures()[0].path, "/hang");
}
