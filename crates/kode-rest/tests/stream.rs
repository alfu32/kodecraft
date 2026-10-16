#[path = "support/http.rs"]
mod http_server;
#[path = "support/websocket.rs"]
mod socket_server;
use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    http::{ExecutionKey, Failure, Response},
    model::NodePath,
    resolve::{BodyPlan, Header, PreparedRequest, Protocol},
    stream::{DataKind, Event, EventKind, Executor, InitialMessage, event_channel},
};
use std::{sync::Arc, time::Duration};
fn key() -> ExecutionKey {
    ExecutionKey {
        tag: JobTag {
            workspace: WorkspaceGeneration(9),
            request: RequestId::fresh().unwrap(),
            document: None,
        },
        item_id: "original".into(),
    }
}
fn request(url: String, protocol: Protocol) -> PreparedRequest {
    PreparedRequest {
        request: kode_rest::resolve::ResolvedRequest {
            path: NodePath(vec![0]),
            method: "GET".into(),
            url,
            headers: vec![],
            body: BodyPlan::Empty,
            timeout_ms: 1000,
            follow_redirects: true,
            ssl_validation: true,
            protocol,
        },
        body: Arc::from([]),
    }
}
async fn collect(executor: &Executor, request: PreparedRequest) -> (Response, Vec<Event>) {
    let (sender, mut receiver) = event_channel();
    let identity = key();
    let original = identity.clone();
    let events = async move {
        let mut events = vec![];
        while let Some(event) = receiver.recv().await {
            assert_eq!(event.key, original);
            events.push(event);
        }
        events
    };
    let (response, events) = tokio::join!(
        executor.execute(identity, request, CancellationToken::new(), Some(sender)),
        events
    );
    (response.unwrap(), events)
}
#[tokio::test]
async fn original_sse_incremental_case_preserves_raw_bom_boundaries_multiline_ids_and_retry() {
    let server = http_server::Server::start(false).await;
    let executor = Executor::new(1024).unwrap();
    let (response, events) = collect(
        &executor,
        request(format!("{}/sse", server.base), Protocol::Sse),
    )
    .await;
    assert!(response.is_success());
    assert!(response.body.starts_with(&[0xef, 0xbb, 0xbf]));
    assert_eq!(response.received_bytes, response.body.len() as u64);
    let parsed: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let EventKind::Sse {
                name,
                data,
                id,
                retry,
            } = &event.kind
            {
                Some((name.as_str(), data.as_str(), id.as_str(), *retry))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        parsed,
        [
            ("change", "one\ntwo", "7", Some(Duration::from_millis(5))),
            ("message", "é", "7", None)
        ]
    );
    assert!(events.iter().any(|e| matches!(
        e.kind,
        EventKind::Data {
            kind: DataKind::SseRaw,
            ..
        }
    )));
}
#[tokio::test]
async fn sse_caps_preview_counts_all_bytes_reports_bad_utf8_and_limits_unfinished_events() {
    let server = http_server::Server::start(false).await;
    let executor = Executor::new(32).unwrap();
    let (response, events) = collect(
        &executor,
        request(format!("{}/sse-many", server.base), Protocol::Sse),
    )
    .await;
    assert!(response.is_success());
    assert_eq!(response.body.len(), 32);
    assert_eq!(response.received_bytes, 1400);
    assert!(response.truncated);
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::Notice(_)))
    );
    for (path, failure) in [
        ("sse-huge", Failure::StreamLimit),
        ("sse-huge-event", Failure::StreamLimit),
        ("sse-invalid", Failure::StreamProtocol),
    ] {
        let response = executor
            .execute(
                key(),
                request(format!("{}/{path}", server.base), Protocol::Sse),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.error, Some(failure));
        assert!(response.received_bytes > 0);
        assert!(response.body.len() <= 32);
    }
    let response = executor
        .execute(
            key(),
            request(format!("{}/status", server.base), Protocol::Sse),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.status_code, Some(418));
    assert_eq!(response.error, None);
    assert_eq!(response.body_text(), "teapot");
}
#[tokio::test]
async fn sse_progress_precedes_completion_and_bounded_backpressure_is_independently_cancellable() {
    let server = http_server::Server::start(false).await;
    let executor = Executor::new(32).unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let (sender, mut receiver) = event_channel();
    let job = tokio::spawn(async move {
        executor
            .execute(
                key(),
                request(format!("{}/sse-infinite", server.base), Protocol::Sse),
                token,
                Some(sender),
            )
            .await
    });
    let first = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(first.kind, EventKind::Opened { .. }));
    assert!(!job.is_finished());
    tokio::time::timeout(Duration::from_secs(2), async {
        while receiver.len() < 64 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let response = tokio::time::timeout(Duration::from_millis(500), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert_eq!(response.body.len(), 32);
    assert!(response.truncated);
    assert!(receiver.len() <= 64);
}
#[tokio::test]
async fn sse_headers_body_timeout_tls_and_shared_http_event_interface_use_request_settings() {
    let server = http_server::Server::start(true).await;
    let executor = Executor::default();
    let mut prepared = request(format!("{}/sse", server.base), Protocol::Sse);
    prepared.request.method = "POST".into();
    prepared.body = Arc::from(&b"outgoing"[..]);
    prepared.request.body = BodyPlan::Bytes(prepared.body.clone());
    prepared.request.headers.push(Header {
        name: "X-Test".into(),
        value: "kept".into(),
    });
    let rejected = executor
        .execute(key(), prepared.clone(), CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(rejected.error, Some(Failure::Connect));
    prepared.request.ssl_validation = false;
    let (response, _) = collect(&executor, prepared).await;
    assert!(response.is_success());
    let capture = &server.captures()[0];
    assert_eq!(capture.method, "POST");
    assert_eq!(capture.path, "/sse");
    assert_eq!(capture.body, b"outgoing");
    assert_eq!(capture.values("X-Test"), ["kept"]);
    let plain = http_server::Server::start(false).await;
    let mut prepared = request(format!("{}/hang", plain.base), Protocol::Sse);
    prepared.request.timeout_ms = 50;
    let response = executor
        .execute(key(), prepared, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Timeout));
    let (response, events) = collect(
        &executor,
        request(format!("{}/ok", plain.base), Protocol::Http),
    )
    .await;
    assert!(response.is_success());
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::Data {
            kind: DataKind::Http,
            ..
        }
    )));
}
#[tokio::test]
async fn websocket_fragments_binary_ping_normal_close_and_header_cookie_auth_are_preserved() {
    let server = socket_server::Server::start(false).await;
    let executor = Executor::default();
    let mut prepared = request(format!("{}/messages", server.base), Protocol::WebSocket);
    executor.cookies().accept(
        &url::Url::parse(&server.base.replace("ws://", "http://")).unwrap(),
        ["session=abc; Path=/; HttpOnly"],
    );
    prepared.request.headers = vec![
        Header {
            name: "Authorization".into(),
            value: "Bearer synthetic".into(),
        },
        Header {
            name: "Host".into(),
            value: "wrong.invalid".into(),
        },
        Header {
            name: "X-Test".into(),
            value: "one".into(),
        },
        Header {
            name: "X-Test".into(),
            value: "two".into(),
        },
    ];
    let (response, events) = collect(&executor, prepared).await;
    assert!(response.is_success(), "{response:?}");
    assert_eq!(response.status_code, Some(101));
    assert_eq!(&*response.body, &[0xc3, 0xa9, 0, 255]);
    assert_eq!(response.received_bytes, 4);
    assert_eq!(response.close.as_ref().unwrap().code, 1000);
    assert_eq!(response.close.as_ref().unwrap().reason, "done");
    assert_eq!(
        response
            .headers
            .iter()
            .filter(|h| h.name == "x-duplicate")
            .count(),
        2
    );
    assert!(
        executor
            .cookies()
            .snapshot()
            .iter()
            .any(|cookie| cookie.name == "socket")
    );
    let headers = server.headers.lock().unwrap();
    let headers = &headers[0];
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer synthetic")
    );
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "cookie" && v == "session=abc")
    );
    assert_eq!(headers.iter().filter(|(k, _)| k == "x-test").count(), 2);
    assert!(!headers.iter().any(|(_, v)| v == "wrong.invalid"));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::Data {
            kind: DataKind::WebSocketText,
            message_end: true,
            ..
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event.kind,
        EventKind::Data {
            kind: DataKind::WebSocketBinary,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, EventKind::Closed(_)))
    );
}
#[tokio::test]
async fn websocket_abnormal_close_transport_drop_and_oversize_frame_keep_partial_data() {
    let server = socket_server::Server::start(false).await;
    let executor = Executor::new(2).unwrap();
    for (path, failure) in [
        ("abnormal", Failure::WebSocketAbnormalClose),
        ("drop", Failure::WebSocketAbnormalClose),
        ("large", Failure::StreamLimit),
        ("large-message", Failure::StreamLimit),
    ] {
        let response = executor
            .execute(
                key(),
                request(format!("{}/{path}", server.base), Protocol::WebSocket),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.error, Some(failure), "{path}");
        if path == "abnormal" {
            assert_eq!(response.close.as_ref().unwrap().code, 1008);
        }
        if path == "drop" {
            assert_eq!(response.body_text(), "pa");
            assert_eq!(response.received_bytes, 7);
            assert!(response.truncated);
        }
    }
}
#[tokio::test]
async fn websocket_initial_payload_requires_explicit_opt_in_for_text_or_binary() {
    let server = socket_server::Server::start(false).await;
    let executor = Executor::default();
    let mut prepared = request(format!("{}/initial", server.base), Protocol::WebSocket);
    prepared.body = Arc::from(&b"payload"[..]);
    prepared.request.body = BodyPlan::Bytes(prepared.body.clone());
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let job = {
        let executor = executor.clone();
        let prepared = prepared.clone();
        tokio::spawn(async move { executor.execute(key(), prepared, token, None).await })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.headers.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(!job.is_finished());
    assert!(server.incoming.lock().unwrap().is_empty());
    cancel.cancel();
    assert_eq!(job.await.unwrap().unwrap().error, Some(Failure::Cancelled));
    for mode in [InitialMessage::Text, InitialMessage::Binary] {
        let response = executor
            .execute_with_initial(
                key(),
                prepared.clone(),
                CancellationToken::new(),
                None,
                mode,
            )
            .await
            .unwrap();
        assert!(response.is_success());
        assert_eq!(response.body_text(), "payload");
        assert!(response.normalized_request.body.is_empty());
    }
    assert_eq!(server.incoming.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn wss_validation_bypass_secure_cookie_and_following_validated_request_are_independent() {
    let server = socket_server::Server::start(true).await;
    let executor = Executor::default();
    let mut prepared = request(format!("{}/messages", server.base), Protocol::WebSocket);
    executor.cookies().accept(
        &url::Url::parse(&server.base.replace("wss://", "https://")).unwrap(),
        ["secure=kept; Secure; Path=/; HttpOnly"],
    );
    let response = executor
        .execute(key(), prepared.clone(), CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Connect));
    prepared.request.ssl_validation = false;
    let response = executor
        .execute(key(), prepared.clone(), CancellationToken::new(), None)
        .await
        .unwrap();
    assert!(response.is_success());
    assert!(
        server.headers.lock().unwrap()[0]
            .iter()
            .any(|(k, v)| k == "cookie" && v == "secure=kept")
    );
    prepared.request.ssl_validation = true;
    let response = executor
        .execute(key(), prepared, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Connect));
}

#[tokio::test]
async fn websocket_redirect_setting_loop_limit_and_cross_origin_credentials_are_qualified() {
    let source = socket_server::Server::start(false).await;
    let destination = socket_server::Server::start(false).await;
    let executor = Executor::default();
    let response = executor
        .execute(
            key(),
            request(format!("{}/redirect", source.base), Protocol::WebSocket),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(response.is_success());
    assert_eq!(response.final_url, format!("{}/messages", source.base));
    let mut stopped = request(format!("{}/redirect", source.base), Protocol::WebSocket);
    stopped.request.follow_redirects = false;
    let response = executor
        .execute(key(), stopped, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(response.status_code, Some(302));
    assert_eq!(response.error, Some(Failure::StreamProtocol));
    let response = executor
        .execute(
            key(),
            request(format!("{}/loop", source.base), Protocol::WebSocket),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Redirect));
    let target = format!(
        "{}/messages",
        destination.base.replace("127.0.0.1", "localhost")
    );
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &target)
        .finish();
    let mut prepared = request(
        format!("{}/redirect?{query}", source.base),
        Protocol::WebSocket,
    );
    prepared.request.headers = vec![
        Header {
            name: "Authorization".into(),
            value: "Bearer private".into(),
        },
        Header {
            name: "Cookie".into(),
            value: "private=yes".into(),
        },
    ];
    let response = executor
        .execute(key(), prepared, CancellationToken::new(), None)
        .await
        .unwrap();
    assert!(response.is_success());
    let headers = destination.headers.lock().unwrap();
    assert!(
        headers[0]
            .iter()
            .all(|(name, _)| name != "authorization" && name != "cookie")
    );
}
#[tokio::test]
async fn websocket_infinite_stream_caps_output_keeps_identity_and_cancels_after_handshake_timeout()
{
    let server = socket_server::Server::start(false).await;
    let executor = Executor::new(8).unwrap();
    let mut prepared = request(format!("{}/infinite", server.base), Protocol::WebSocket);
    prepared.request.timeout_ms = 100;
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let identity = key();
    let original = identity.clone();
    let (sender, mut receiver) = event_channel();
    let job = tokio::spawn(async move {
        executor
            .execute(identity, prepared, token, Some(sender))
            .await
    });
    let observed = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.key, original);
    assert!(matches!(observed.kind, EventKind::Opened { .. }));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!job.is_finished());
    cancel.cancel();
    let response = tokio::time::timeout(Duration::from_millis(500), job)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.key, original);
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert_eq!(response.body.len(), 8);
    assert!(response.received_bytes > 8);
    assert!(response.truncated);
    assert_eq!(server.headers.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn sse_scheme_and_accept_selection_flow_from_the_real_resolver() {
    let server = http_server::Server::start(false).await;
    let executor = Executor::default();
    for url in [
        format!("{}/sse", server.base.replace("http://", "sse://")),
        format!("{}/sse", server.base),
    ] {
        let document = serde_json::json!({"info":{},"item":[{"request":{"url":url,"header":[{"key":"Accept","value":"text/event-stream"}]}}]});
        let prepared = kode_rest::resolve::RequestResolver::new(
            &document,
            NodePath(vec![0]),
            std::path::Path::new("."),
            None,
        )
        .unwrap()
        .plan()
        .unwrap()
        .materialize(1024, &CancellationToken::new())
        .unwrap();
        assert_eq!(prepared.request.protocol, Protocol::Sse);
        let (response, events) = collect(&executor, prepared).await;
        assert!(response.is_success());
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, EventKind::Sse { .. }))
        );
    }
}
#[tokio::test]
async fn stream_validation_closed_owner_and_explicit_message_validation_precede_network() {
    let server = socket_server::Server::start(false).await;
    let executor = Executor::default();
    let mut prepared = request(format!("{}/initial", server.base), Protocol::WebSocket);
    prepared.request.headers.push(Header {
        name: "Bad Name".into(),
        value: "private".into(),
    });
    assert!(
        executor
            .execute(key(), prepared, CancellationToken::new(), None)
            .await
            .is_err()
    );
    let mut prepared = request(format!("{}/initial", server.base), Protocol::WebSocket);
    prepared.body = Arc::from(&[255][..]);
    assert!(
        executor
            .execute_with_initial(
                key(),
                prepared.clone(),
                CancellationToken::new(),
                None,
                InitialMessage::Text
            )
            .await
            .is_err()
    );
    prepared.body = vec![0; 256 * 1024 + 1].into();
    assert!(
        executor
            .execute_with_initial(
                key(),
                prepared,
                CancellationToken::new(),
                None,
                InitialMessage::Binary
            )
            .await
            .is_err()
    );
    let (sender, receiver) = event_channel();
    drop(receiver);
    let response = executor
        .execute(
            key(),
            request(format!("{}/initial", server.base), Protocol::WebSocket),
            CancellationToken::new(),
            Some(sender),
        )
        .await
        .unwrap();
    assert_eq!(response.error, Some(Failure::Cancelled));
    assert!(server.headers.lock().unwrap().is_empty());
}
