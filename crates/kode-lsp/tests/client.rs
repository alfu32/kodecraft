use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, TextRange, commands::EditKind,
    document::Document,
};
use kode_lsp::{
    client::{ClientOptions, ClientState, LspClient},
    documents::{PositionEncoding, file_uri},
};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessSpec, SystemProcessFactory},
};
use lsp_types::{CompletionResponse, GotoDefinitionResponse, Position};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
#[path = "support/process.rs"]
mod process;
mod support;
struct Fixture {
    directory: tempfile::TempDir,
    spec: ProcessSpec,
    options: ClientOptions,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut spec = ProcessSpec::new(support::python());
        spec.args = vec![
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/lsp/server.py")
                .display()
                .to_string(),
            mode.into(),
            directory.path().join("trace.jsonl").display().to_string(),
            directory.path().join("pid").display().to_string(),
        ];
        let mut options = ClientOptions::new(directory.path().into());
        options.configuration = json!({"fixture":{"option":42}});
        options.startup_timeout = Duration::from_secs(2);
        options.request_timeout = Duration::from_millis(200);
        options.write_timeout = Duration::from_millis(300);
        options.shutdown_timeout = Duration::from_millis(150);
        options.exit_timeout = Duration::from_millis(100);
        Self {
            directory,
            spec,
            options,
        }
    }
    async fn start(&self) -> kode_core::Result<LspClient> {
        LspClient::start(
            self.spec.clone(),
            self.options.clone(),
            Arc::new(SystemProcessFactory),
            &CancellationToken::new(),
        )
        .await
    }
    fn trace(&self) -> Vec<Value> {
        fs::read_to_string(self.directory.path().join("trace.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
async fn until(predicate: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(Instant::now() < end, "fixture deadline exceeded");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
fn assert_reaped(pid: u32) {
    assert!(!process::running(pid), "child {pid} was not reaped");
}
#[tokio::test]
async fn real_child_initializes_server_requests_fragments_document_order_typed_queries_and_graceful_exit()
 {
    let fixture = Fixture::new("fragment");
    let client = fixture.start().await.unwrap();
    let pid = client.process_id();
    assert_eq!(client.state(), ClientState::Running);
    assert_eq!(client.info().unwrap().encoding, PositionEncoding::Utf16);
    assert_eq!(
        client.info().unwrap().server_name.as_deref(),
        Some("synthetic-peer")
    );
    let path = fixture.directory.path().join("source 😀.kt");
    let mut document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(path.clone()),
        Some("kotlin".into()),
        "// 😀\r\nval café = 1\r\n",
    );
    let uri = file_uri(&path).unwrap();
    let original = document.snapshot();
    let stamp = client
        .sync_document(original.clone(), false, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(stamp.document, document.id());
    document
        .apply_edit(
            document.revision(),
            TextRange::caret(ByteOffset(original.len_bytes())),
            "// edited\n",
            EditKind::Typing,
        )
        .unwrap();
    let stamp = client
        .sync_document(document.snapshot(), true, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(client.document_stamp(&uri), Some(stamp));
    assert!(
        client
            .sync_document(original, false, &CancellationToken::new())
            .await
            .is_err()
    );
    assert!(
        matches!(client.definitions(&uri,Position::new(1,4),&CancellationToken::new()).await.unwrap(),Some(GotoDefinitionResponse::Link(links)) if links.len()==1)
    );
    assert_eq!(
        client
            .references(&uri, Position::new(1, 4), &CancellationToken::new())
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        matches!(client.completions(&uri,Position::new(1,4),&CancellationToken::new()).await.unwrap(),Some(CompletionResponse::List(list)) if list.items[0].label=="naïve")
    );
    client
        .close_document(document.origin().clone())
        .await
        .unwrap();
    assert!(client.document_stamp(&uri).is_none());
    client.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(client.state(), ClientState::Stopped);
    assert_reaped(pid);
    let trace = fixture.trace();
    let methods: Vec<_> = trace
        .iter()
        .filter_map(|value| value["method"].as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/definition",
            "textDocument/references",
            "textDocument/completion",
            "textDocument/didClose",
            "shutdown",
            "exit"
        ]
    );
    let initialize = &trace[0]["params"];
    assert!(
        initialize["capabilities"]["general"]["positionEncodings"]
            .as_array()
            .unwrap()
            .contains(&json!("utf-16"))
    );
    assert_eq!(
        initialize["workspaceFolders"][0]["uri"],
        file_uri(fixture.directory.path()).unwrap().as_str()
    );
    assert!(
        trace
            .iter()
            .any(|value| value["id"] == "configuration-😀" && value["result"] == json!([42, null]))
    );
    assert!(
        trace
            .iter()
            .any(|value| value["id"] == -7 && value["error"]["code"] == -32601)
    );
    assert_eq!(client.pending_requests(), 0);
}
#[tokio::test]
async fn out_of_order_errors_timeouts_cancellation_and_dropped_futures_keep_pending_ids_correct() {
    let fixture = Fixture::new("normal");
    let client = fixture.start().await.unwrap();
    let cancel = CancellationToken::new();
    let (one, two) = tokio::join!(
        client.request("fixture/order", json!({"value":"one 😀"}), &cancel),
        client.request("fixture/order", json!({"value":"two"}), &cancel)
    );
    assert_eq!(one.unwrap(), "one 😀");
    assert_eq!(two.unwrap(), "two");
    let error = client
        .request("fixture/error", Value::Null, &cancel)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("-32603")
            && error.to_string().contains("fixture request failed")
    );
    assert_eq!(client.state(), ClientState::Running);
    assert!(
        client
            .request("fixture/hang", Value::Null, &cancel)
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert_eq!(client.pending_requests(), 0);
    let token = CancellationToken::new();
    let work_client = client.clone();
    let work_token = token.clone();
    let work = tokio::spawn(async move {
        work_client
            .request("fixture/hang", Value::Null, &work_token)
            .await
    });
    until(|| client.pending_requests() == 1).await;
    token.cancel();
    assert!(matches!(work.await.unwrap(), Err(DomainError::Cancelled)));
    assert_eq!(client.pending_requests(), 0);
    let work_client = client.clone();
    let work = tokio::spawn(async move {
        work_client
            .request("fixture/hang", Value::Null, &CancellationToken::new())
            .await
    });
    until(|| client.pending_requests() == 1).await;
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    assert_eq!(client.pending_requests(), 0);
    let ids = client
        .request("fixture/cancelled", Value::Null, &cancel)
        .await
        .unwrap();
    assert!(ids.as_array().unwrap().len() >= 3);
    assert_eq!(
        client
            .request("fixture/echo", json!("still running"), &cancel)
            .await
            .unwrap(),
        "still running"
    );
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn startup_errors_timeout_cancel_crash_and_bad_protocol_always_reap_the_owned_process() {
    for mode in [
        "init-error",
        "bad-frame",
        "bad-encoding",
        "crash-start",
        "no-initialize",
    ] {
        let mut fixture = Fixture::new(mode);
        fixture.options.startup_timeout = Duration::from_millis(200);
        let result = fixture.start().await;
        assert!(result.is_err(), "{mode}");
        let pid: u32 = fs::read_to_string(fixture.directory.path().join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_reaped(pid);
    }
    let fixture = Fixture::new("no-initialize");
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let spec = fixture.spec.clone();
    let options = fixture.options.clone();
    let work = tokio::spawn(async move {
        LspClient::start(spec, options, Arc::new(SystemProcessFactory), &token).await
    });
    until(|| {
        fs::read_to_string(fixture.directory.path().join("pid"))
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some()
    })
    .await;
    cancel.cancel();
    assert!(matches!(work.await.unwrap(), Err(DomainError::Cancelled)));
    let pid: u32 = fs::read_to_string(fixture.directory.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert_reaped(pid);
    let mut missing = fixture.spec.clone();
    missing.program = fixture.directory.path().join("not-installed");
    assert!(
        LspClient::start(
            missing,
            fixture.options.clone(),
            Arc::new(SystemProcessFactory),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn crash_stderr_flood_notifications_capabilities_and_forced_shutdown_remain_bounded() {
    let fixture = Fixture::new("stderr-flood");
    let client = fixture.start().await.unwrap();
    client
        .request(
            "fixture/notifications",
            Value::Null,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    until(|| client.stderr().contains("stderr end marker")).await;
    assert!(client.stderr().len() <= kode_platform::process::STDERR_CAPACITY);
    let events = client.take_events();
    assert!(events.len() <= 32);
    assert!(
        events
            .iter()
            .any(|event| event.method == "textDocument/publishDiagnostics")
    );
    let pid = client.process_id();
    assert!(
        client
            .request("fixture/crash", Value::Null, &CancellationToken::new())
            .await
            .is_err()
    );
    until(|| client.state() == ClientState::Failed).await;
    client.shutdown().await.unwrap();
    assert_reaped(pid);
    assert!(client.last_error().unwrap().contains("stderr"));
    let fixture = Fixture::new("no-capabilities");
    let client = fixture.start().await.unwrap();
    let uri = file_uri(fixture.directory.path()).unwrap();
    assert!(matches!(
        client
            .definitions(&uri, Position::new(0, 0), &CancellationToken::new())
            .await,
        Err(DomainError::Unsupported(_))
    ));
    client.shutdown().await.unwrap();
    assert!(
        !fixture
            .trace()
            .iter()
            .any(|message| message["method"] == "textDocument/definition")
    );
    let fixture = Fixture::new("ignore-shutdown");
    let client = fixture.start().await.unwrap();
    let pid = client.process_id();
    let start = Instant::now();
    client.shutdown().await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_reaped(pid);
}
#[tokio::test]
async fn blocked_pipe_dropped_start_future_and_dropped_client_cannot_leave_a_server_running() {
    let fixture = Fixture::new("blocked-stdin");
    let client = fixture.start().await.unwrap();
    let pid = client.process_id();
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(fixture.directory.path().join("big.kt")),
        Some("kotlin".into()),
        &"x".repeat(1024 * 1024),
    );
    assert!(
        client
            .sync_document(document.snapshot(), false, &CancellationToken::new())
            .await
            .is_err()
    );
    client.shutdown().await.unwrap();
    assert_reaped(pid);
    let fixture = Fixture::new("no-initialize");
    let spec = fixture.spec.clone();
    let options = fixture.options.clone();
    let work = tokio::spawn(async move {
        LspClient::start(
            spec,
            options,
            Arc::new(SystemProcessFactory),
            &CancellationToken::new(),
        )
        .await
    });
    until(|| {
        fs::read_to_string(fixture.directory.path().join("pid"))
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some()
    })
    .await;
    work.abort();
    assert!(work.await.is_err());
    let pid: u32 = fs::read_to_string(fixture.directory.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    until(|| !process::running(pid)).await;
    assert_reaped(pid);
    let fixture = Fixture::new("normal");
    let client = fixture.start().await.unwrap();
    let pid = client.process_id();
    drop(client);
    until(|| !process::running(pid)).await;
    assert_reaped(pid);
}

#[tokio::test]
async fn pending_request_capacity_is_bounded_and_slots_are_released_after_cancellation() {
    let mut fixture = Fixture::new("normal");
    fixture.options.request_timeout = Duration::from_secs(5);
    let client = fixture.start().await.unwrap();
    let cancel = CancellationToken::new();
    let mut tasks = vec![];
    for index in 0..63 {
        let client = client.clone();
        let token = cancel.clone();
        tasks.push(tokio::spawn(async move {
            client.request("fixture/hang", Value::Null, &token).await
        }));
        until(|| {
            fixture
                .trace()
                .iter()
                .filter(|message| message["method"] == "fixture/hang")
                .count()
                > index
        })
        .await;
    }
    until(|| client.pending_requests() == 63).await;
    assert!(matches!(
        client
            .request("fixture/echo", Value::Null, &CancellationToken::new())
            .await,
        Err(DomainError::QueueFull)
    ));
    cancel.cancel();
    for task in tasks {
        assert!(matches!(task.await.unwrap(), Err(DomainError::Cancelled)));
    }
    assert_eq!(client.pending_requests(), 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match client
            .request(
                "fixture/echo",
                json!("capacity restored"),
                &CancellationToken::new(),
            )
            .await
        {
            Err(DomainError::QueueFull) => {
                assert!(Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
            result => {
                assert_eq!(result.unwrap(), "capacity restored");
                break;
            }
        }
    }
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn registry_serializes_duplicate_starts_preserves_language_precedence_and_restarts_real_children()
 {
    use kode_lsp::{catalog::Catalog, discovery::ResolvedCommand, service::LspService};
    let fixture = Fixture::new("normal");
    let service = LspService::with_factory(
        fixture.options.clone(),
        Arc::new(SystemProcessFactory),
        None,
    );
    let entry = |id: &str| {
        Catalog::parse(
            &serde_json::to_vec(&json!({"servers":[{"id":id,"name":id,"languages":["kotlin"]}]}))
                .unwrap(),
        )
        .unwrap()
        .servers
        .remove(0)
    };
    let mut args = vec![fixture.spec.program.to_str().unwrap().to_owned()];
    args.extend(fixture.spec.args.clone());
    let command = ResolvedCommand { args, cwd: None };
    let cancel = CancellationToken::new();
    let (one, duplicate) = tokio::join!(
        service.start(entry("one"), command.clone(), &cancel),
        service.start(entry("one"), command.clone(), &cancel)
    );
    let one = one.unwrap();
    assert_eq!(one.process_id(), duplicate.unwrap().process_id());
    let first_pid = one.process_id();
    let two = service
        .start(entry("two"), command.clone(), &cancel)
        .await
        .unwrap();
    let second_pid = two.process_id();
    assert_eq!(
        service.for_language("KOTLIN").unwrap().process_id(),
        first_pid
    );
    assert!(service.running("one") && service.running("two"));
    let revision = service.revision();
    let restarted = service
        .restart(entry("one"), command, &cancel)
        .await
        .unwrap();
    assert_reaped(first_pid);
    assert_ne!(restarted.process_id(), first_pid);
    assert!(service.revision() > revision);
    assert_eq!(
        service.for_language("kotlin").unwrap().process_id(),
        second_pid
    );
    let restarted_pid = restarted.process_id();
    service.shutdown().await.unwrap();
    assert_reaped(second_pid);
    assert_reaped(restarted_pid);
    assert!(!service.running("one") && service.for_language("kotlin").is_none());
}

#[tokio::test]
async fn registry_records_failed_start_and_shutdown_cancels_an_inflight_start_before_reaping() {
    use kode_lsp::{catalog::Catalog, discovery::ResolvedCommand, service::LspService};
    let fixture = Fixture::new("init-error");
    let service = LspService::with_factory(
        fixture.options.clone(),
        Arc::new(SystemProcessFactory),
        None,
    );
    let entry = Catalog::parse(
        br#"{"servers":[{"id":"fixture","name":"fixture","languages":["kotlin"]}]}"#,
    )
    .unwrap()
    .servers
    .remove(0);
    let command = |fixture: &Fixture| {
        let mut args = vec![fixture.spec.program.to_str().unwrap().into()];
        args.extend(fixture.spec.args.clone());
        ResolvedCommand { args, cwd: None }
    };
    assert!(
        service
            .start(entry.clone(), command(&fixture), &CancellationToken::new())
            .await
            .is_err()
    );
    assert!(!service.running("fixture"));
    assert!(
        service
            .last_error("fixture")
            .unwrap()
            .contains("fixture startup denied")
    );
    service.shutdown().await.unwrap();
    let mut fixture = Fixture::new("no-initialize");
    fixture.options.startup_timeout = Duration::from_secs(10);
    let service = Arc::new(LspService::with_factory(
        fixture.options.clone(),
        Arc::new(SystemProcessFactory),
        None,
    ));
    let work_service = service.clone();
    let command = command(&fixture);
    let work = tokio::spawn(async move {
        work_service
            .start(entry, command, &CancellationToken::new())
            .await
    });
    until(|| {
        fs::read_to_string(fixture.directory.path().join("pid"))
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .is_some()
    })
    .await;
    let pid: u32 = fs::read_to_string(fixture.directory.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    service.shutdown().await.unwrap();
    assert!(matches!(work.await.unwrap(), Err(DomainError::Cancelled)));
    assert_reaped(pid);
}
