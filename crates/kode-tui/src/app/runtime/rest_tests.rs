use super::*;
use kode_platform::jobs::CancellationToken;
#[tokio::test]
async fn superseded_cancellable_jobs_release_their_context_proofs_without_a_completion() {
    let directory = tempfile::tempdir().unwrap();
    let prepared = crate::app::workspace::prepare_workspace(
        directory.path(),
        kode_core::WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(kode_platform::clipboard::ClipboardService::internal()),
        Arc::new(kode_platform::jobs::MonotonicClock::default()),
    )
    .unwrap();
    // The worker does not capture this value. Its only remaining strong owner is
    // the UI context, which cancellable delivery is allowed never to return.
    let collection = Arc::new(serde_json::json!({"info":{},"item":[]}));
    let proof = Arc::downgrade(&collection);
    let old = app.state.tag().unwrap();
    app.blocking(
        old,
        app.root_owner,
        "rest-retention",
        false,
        Context::Rest(Box::new(crate::panels::rest::state::Context::Editor(
            crate::panels::rest::editor::jobs::Context::Form {
                key: "old".into(),
                path: Default::default(),
                collection,
            },
        ))),
        Box::new(|cancel| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            if cancel.is_cancelled() {
                Err(DomainError::Cancelled)
            } else {
                Err(DomainError::Unsupported("fixture".into()))
            }
        }),
    )
    .unwrap();
    app.pump().unwrap();
    assert!(app.contexts.contains_key(&old));
    assert!(proof.upgrade().is_some());
    let next = app.state.tag().unwrap();
    app.blocking(
        next,
        app.root_owner,
        "rest-retention",
        false,
        Context::Rest(Box::new(crate::panels::rest::state::Context::Run {
            structure: 0,
        })),
        Box::new(|_| Err(DomainError::Cancelled)),
    )
    .unwrap();
    assert!(!app.contexts.contains_key(&old));
    assert!(proof.upgrade().is_none());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

async fn pump_until(app: &mut AppRuntime, ready: impl Fn(&AppRuntime) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        app.pump().unwrap();
        if ready(app) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "OAuth runtime did not reach expected state: {}",
            app.state.notice
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn oauth_switch_removal_and_shutdown_preserve_runtime_item_ownership() {
    use crate::panels::rest::{oauth::Action, sidebar::TreeChange};
    use kode_rest::model::NodePath;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let provider = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = vec![0; 16384];
        let _ = socket.read(&mut input).await.unwrap();
        let body =
            r#"{"access_token":"runtime-owned-secret","token_type":"Bearer","expires_in":60}"#;
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let prepared = crate::app::workspace::prepare_workspace(
        directory.path(),
        kode_core::WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(kode_platform::clipboard::ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    app.state.rest.model.replace_collection(serde_json::json!({"info":{},"auth":{"type":"oauth2","oauth2":[
        {"key":"clientId","value":"synthetic-client"},{"key":"authUrl","value":format!("{endpoint}/authorize")},{"key":"accessTokenUrl","value":format!("{endpoint}/token")}
    ]},"item":[{"name":"A","request":{"url":format!("{endpoint}/a")}},{"name":"B","request":{"url":format!("{endpoint}/b")}}]})).unwrap();
    app.state.begin_rest_form(NodePath(vec![0])).unwrap();
    pump_until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|f| f.path == NodePath(vec![0]))
    })
    .await;
    let key_a = app.state.rest.form.as_ref().unwrap().key.clone();
    app.state.rest_oauth_action(Action::Acquire).unwrap();
    pump_until(&mut app, |app| {
        app.state
            .rest
            .oauth
            .running
            .as_ref()
            .is_some_and(|r| r.progress.borrow().authorization_url.is_some())
    })
    .await;
    let running = app.state.rest.oauth.running.as_ref().unwrap();
    let sender = running.redirects.clone();
    let authorization = url::Url::parse(
        running
            .progress
            .borrow()
            .authorization_url
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    let params: std::collections::HashMap<_, _> =
        authorization.query_pairs().into_owned().collect();
    let callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    let port = callback.port().unwrap();
    app.state.begin_rest_form(NodePath(vec![1])).unwrap();
    pump_until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|f| f.path == NodePath(vec![1]))
    })
    .await;
    let key_b = app.state.rest.form.as_ref().unwrap().key.clone();
    sender
        .send(format!(
            "{callback}?code=synthetic&state={}",
            params["state"]
        ))
        .await
        .unwrap();
    pump_until(&mut app, |app| {
        app.state.rest.oauth.tokens.contains_key(&key_a)
    })
    .await;
    provider.await.unwrap();
    assert!(!app.state.rest.oauth.tokens.contains_key(&key_b));
    assert!(!app.state.rest.oauth.view(&key_b).has_token);
    assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok());
    assert!(
        !serde_json::to_string(&app.state.rest.model.project_state())
            .unwrap()
            .contains("runtime-owned-secret")
    );
    // Reusing path zero after removal cannot reuse A's stable runtime identity.
    app.state
        .rest
        .run
        .tree_changed(TreeChange::Removed(NodePath(vec![0])));
    app.state.rest.oauth.retain(&app.state.rest.run.live_keys);
    assert!(app.state.rest.oauth.tokens.is_empty());
    assert_eq!(
        app.state.rest.run.keys.get(&NodePath(vec![0])),
        Some(&key_b)
    );
    // Shutdown must also cancel a second flow while its local TLS listener waits.
    app.state.rest_oauth_action(Action::Acquire).unwrap();
    pump_until(&mut app, |app| {
        app.state
            .rest
            .oauth
            .running
            .as_ref()
            .is_some_and(|r| r.progress.borrow().authorization_url.is_some())
    })
    .await;
    let callback = app
        .state
        .rest
        .oauth
        .running
        .as_ref()
        .unwrap()
        .progress
        .borrow()
        .callback_url
        .clone()
        .unwrap();
    let port = url::Url::parse(&callback).unwrap().port().unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert!(TcpListener::bind(("127.0.0.1", port)).await.is_ok());
    let saved = std::fs::read_to_string(directory.path().join(".kode-rs.json")).unwrap();
    assert!(!saved.contains("runtime-owned-secret"));
}
