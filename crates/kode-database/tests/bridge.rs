#![cfg(unix)]
use kode_database::bridge::{
    client::{Client, Failure, Options, State},
    runtime::{Installation, Runtime, java_major},
};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec, SystemProcessFactory},
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
fn options() -> Options {
    Options {
        startup_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
        write_timeout: Duration::from_secs(1),
        shutdown_timeout: Duration::from_millis(300),
        ..Options::default()
    }
}
fn spec(mode: &str, pid: &Path) -> ProcessSpec {
    let mut spec = ProcessSpec::new("python3");
    spec.args = vec![
        format!(
            "{}/tests/fixtures/fake_helper.py",
            env!("CARGO_MANIFEST_DIR")
        ),
        mode.into(),
        pid.to_string_lossy().into(),
    ];
    spec
}
async fn start(mode: &str, pid: &Path) -> Client {
    Client::start(
        spec(mode, pid),
        options(),
        Arc::new(SystemProcessFactory),
        &CancellationToken::new(),
    )
    .await
    .unwrap()
}
async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(check(), "condition did not become true");
}
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .split(')')
            .nth(1)
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}
#[tokio::test]
async fn fragmented_hello_multiple_out_of_order_requests_independent_cancel_and_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let client = start("normal", &dir.path().join("pid")).await;
    let pid = client.process_id();
    let c = client.clone();
    let slow = tokio::spawn(async move {
        c.request(
            "test.echo",
            json!({"delay":0.15,"text":"é界😀"}),
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    });
    let quick = client
        .request(
            "test.echo",
            json!({"delay":0,"text":"fast"}),
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(quick["text"], "fast");
    assert!(!slow.is_finished());
    assert_eq!(slow.await.unwrap()["text"], "é界😀");
    let c = client.clone();
    let blocked = tokio::spawn(async move {
        c.request("test.block", json!({}), true, &CancellationToken::new())
            .await
    });
    eventually(|| client.pending_requests() == 1).await;
    assert_eq!(
        client
            .request(
                "query.cancel",
                json!({"executionId":"one"}),
                false,
                &CancellationToken::new()
            )
            .await
            .unwrap()["status"],
        "requested"
    );
    assert_eq!(blocked.await.unwrap().unwrap()["outcome"], "cancelled");
    client.shutdown().await.unwrap();
    assert_eq!(client.state(), State::Stopped);
    assert!(!alive(pid));
}
#[tokio::test]
async fn poisoned_lengths_truncation_incompatible_version_start_timeout_and_drop_reap() {
    let dir = tempfile::tempdir().unwrap();
    for mode in [
        "poison",
        "zero",
        "oversize",
        "truncated",
        "wrong-version",
        "timeout",
    ] {
        let path = dir.path().join(mode);
        let mut options = options();
        if mode == "timeout" {
            options.startup_timeout = Duration::from_millis(150);
        }
        let pids = Arc::new(std::sync::Mutex::new(vec![]));
        let result = Client::start(
            spec(mode, &path),
            options,
            Arc::new(RecordingFactory(pids.clone())),
            &CancellationToken::new(),
        )
        .await;
        let error = result.err().expect("invalid helper was accepted");
        assert_eq!(
            error.kind,
            if mode == "timeout" {
                Failure::Timeout
            } else {
                Failure::Protocol
            },
            "{mode}"
        );
        let pids = pids.lock().unwrap();
        assert_eq!(pids.len(), 1);
        assert!(!alive(pids[0]), "{mode} leaked");
    }
    let path = dir.path().join("abort");
    let spec = spec("descendant", &path);
    let startup = tokio::spawn(async move {
        Client::start(
            spec,
            options(),
            Arc::new(SystemProcessFactory),
            &CancellationToken::new(),
        )
        .await
    });
    eventually(|| {
        path.with_extension("child").exists()
            || PathBuf::from(format!("{}.child", path.display())).exists()
    })
    .await;
    let pid = std::fs::read_to_string(&path)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let child = std::fs::read_to_string(format!("{}.child", path.display()))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    startup.abort();
    let _ = startup.await;
    eventually(|| !alive(pid) && !alive(child)).await;
    let client = start("normal", &dir.path().join("drop")).await;
    let pid = client.process_id();
    drop(client);
    eventually(|| !alive(pid)).await;
}
#[tokio::test]
async fn crash_and_result_overflow_preserve_received_events_and_unknown_outcomes_without_secret_diagnostics()
 {
    let dir = tempfile::tempdir().unwrap();
    for method in ["test.crash", "test.flood"] {
        let client = start("normal", &dir.path().join(method)).await;
        let pid = client.process_id();
        let error = client
            .request(
                method,
                json!({"password":"synthetic-password-marker"}),
                true,
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(error.unknown_outcome);
        assert!(!format!("{error:?}").contains("synthetic-password-marker"));
        if method == "test.flood" {
            let events = client.take_events();
            assert_eq!(events.len(), 32);
            for (i, event) in events.iter().enumerate() {
                assert_eq!(event.params["sequence"], i);
            }
        }
        client.force_shutdown().await;
        assert!(!alive(pid));
        assert_eq!(client.state(), State::Failed);
    }
}
#[tokio::test]
async fn cancellation_and_timeout_do_not_claim_rollback_or_poison_later_response_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options();
    opts.request_timeout = Duration::from_millis(80);
    let client = Client::start(
        spec("normal", &dir.path().join("pid")),
        opts,
        Arc::new(SystemProcessFactory),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let error = client
        .request(
            "test.echo",
            json!({"delay":0.2}),
            true,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, Failure::Timeout);
    assert!(error.unknown_outcome);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let c = client.clone();
    let pending =
        tokio::spawn(async move { c.request("test.block", json!({}), true, &token).await });
    eventually(|| client.pending_requests() == 1).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let error = pending.await.unwrap().unwrap_err();
    assert_eq!(error.kind, Failure::Cancelled);
    assert!(error.unknown_outcome);
    tokio::time::sleep(Duration::from_millis(220)).await;
    assert_eq!(
        client
            .request("ping", json!({}), false, &CancellationToken::new())
            .await
            .unwrap()["status"],
        "ok"
    );
    assert_eq!(client.pending_requests(), 0);
    client.shutdown().await.unwrap();
}
struct CountingFactory(AtomicUsize);
struct RecordingFactory(Arc<std::sync::Mutex<Vec<u32>>>);
impl ProcessFactory for RecordingFactory {
    fn spawn(&self, spec: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        let child = SystemProcessFactory.spawn(spec)?;
        self.0.lock().unwrap().push(child.id().unwrap());
        Ok(child)
    }
}
#[tokio::test]
async fn structured_remote_error_is_available_to_feature_decoder_but_never_in_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let client = start("normal", &dir.path().join("pid")).await;
    let error = client
        .request("test.error", json!({}), true, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.remote_code, Some(-32603));
    assert!(error.unknown_outcome);
    assert_eq!(error.remote_data().unwrap()["sqlState"], "42000");
    assert!(!format!("{error:?} {error}").contains("synthetic-password-marker"));
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn ordinary_capacity_is_bounded_and_control_requests_keep_reserved_slots() {
    let dir = tempfile::tempdir().unwrap();
    let client = start("normal", &dir.path().join("pid")).await;
    let mut calls = vec![];
    for count in 1..=64 {
        let c = client.clone();
        calls.push(tokio::spawn(async move {
            c.request("test.block", json!({}), true, &CancellationToken::new())
                .await
        }));
        eventually(|| client.pending_requests() == count).await;
    }
    let error = client
        .request("test.block", json!({}), true, &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind, Failure::Busy);
    assert!(!error.unknown_outcome);
    assert_eq!(
        client
            .request(
                "query.cancel",
                json!({"executionId":"one"}),
                false,
                &CancellationToken::new()
            )
            .await
            .unwrap()["status"],
        "requested"
    );
    for call in calls {
        assert_eq!(call.await.unwrap().unwrap()["outcome"], "cancelled");
    }
    assert_eq!(client.pending_requests(), 0);
    client.shutdown().await.unwrap();
}
#[tokio::test]
async fn queued_cancel_prevents_later_submission_after_blocked_pipe_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pid");
    let client = start("slow-reader", &path).await;
    let c = client.clone();
    let large = tokio::spawn(async move {
        c.request(
            "test.echo",
            json!({"payload":"x".repeat(256*1024)}),
            false,
            &CancellationToken::new(),
        )
        .await
    });
    eventually(|| client.pending_requests() == 1).await;
    // The peer deliberately stops reading for 500 ms after its hello.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let c = client.clone();
    let queued =
        tokio::spawn(async move { c.request("test.side-effect", json!({}), true, &token).await });
    eventually(|| client.pending_requests() == 2).await;
    cancel.cancel();
    let error = queued.await.unwrap().unwrap_err();
    assert_eq!(error.kind, Failure::Cancelled);
    assert!(!error.unknown_outcome);
    large.await.unwrap().unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(format!("{}.requests", path.display())).unwrap(),
        "test.echo\n"
    );
}

#[tokio::test]
#[ignore = "requires Java 21 and a built helper; scripts/test-jdbc-bridge.sh runs it"]
async fn packaged_java21_helper_lazy_reuse_explicit_restart_new_epoch_and_reaping() {
    let helper =
        std::env::var_os("KODE_TEST_JDBC_HELPER").expect("KODE_TEST_JDBC_HELPER is required");
    let factory = Arc::new(CountingFactory(AtomicUsize::new(0)));
    let runtime = Runtime::new(
        Installation {
            java: PathBuf::from("java"),
            helper_candidates: vec![helper.into()],
        },
        Options::default(),
        factory.clone(),
    );
    assert_eq!(factory.0.load(Ordering::SeqCst), 0);
    let token = CancellationToken::new();
    let (first, second) = tokio::join!(runtime.get(&token), runtime.get(&token));
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.process_id(), second.process_id());
    assert_eq!(factory.0.load(Ordering::SeqCst), 2);
    let pid = first.process_id();
    let epoch = first.info().unwrap().helper_epoch.clone();
    assert_eq!(
        first
            .request("ping", json!({}), false, &CancellationToken::new())
            .await
            .unwrap()["helperEpoch"],
        epoch
    );
    let error = first
        .request(
            "future.optional.method",
            json!({}),
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.remote_code, Some(-32601));
    first.cancel();
    first.force_shutdown().await;
    assert!(!alive(pid));
    assert!(runtime.get(&CancellationToken::new()).await.is_err());
    assert_eq!(factory.0.load(Ordering::SeqCst), 2);
    let restarted = runtime.restart(&CancellationToken::new()).await.unwrap();
    assert_ne!(restarted.info().unwrap().helper_epoch, epoch);
    let new_pid = restarted.process_id();
    runtime.shutdown().await.unwrap();
    assert!(!alive(new_pid));
    assert_eq!(restarted.state(), State::Stopped);
}

struct FakeJava {
    mode: String,
    pid: PathBuf,
    probe_delay: bool,
}
impl ProcessFactory for FakeJava {
    fn spawn(&self, specification: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        if specification.args == ["-version"] {
            let mut spec = ProcessSpec::new("python3");
            spec.args = vec![
                "-c".into(),
                if self.probe_delay {
                    format!(
                        "import os,time;open({:?},'w').write(str(os.getpid()));time.sleep(60)",
                        self.pid.to_str().unwrap()
                    )
                } else {
                    format!(
                        "import sys;sys.stderr.write('openjdk version \\\"{}\\\"\\n')",
                        if self.mode == "old-java" {
                            "17.0.9"
                        } else {
                            "21.0.9"
                        }
                    )
                },
            ];
            SystemProcessFactory.spawn(&spec)
        } else {
            SystemProcessFactory.spawn(&spec(&self.mode, &self.pid))
        }
    }
}
#[tokio::test]
async fn runtime_old_java_missing_jar_shutdown_during_probe_and_dropped_startup_reap() {
    let dir = tempfile::tempdir().unwrap();
    let helper = dir.path().join("helper.jar");
    std::fs::write(&helper, []).unwrap();
    for mode in ["old-java", "missing-jar"] {
        let runtime = Runtime::new(
            Installation {
                java: "synthetic-java".into(),
                helper_candidates: if mode == "missing-jar" {
                    vec![]
                } else {
                    vec![helper.clone()]
                },
            },
            options(),
            Arc::new(FakeJava {
                mode: mode.into(),
                pid: dir.path().join(mode),
                probe_delay: false,
            }),
        );
        let error = runtime.get(&CancellationToken::new()).await.err().unwrap();
        assert_eq!(error.kind, Failure::Unavailable);
        assert!(error.message.contains(if mode == "old-java" {
            "Java 21"
        } else {
            "helper is missing"
        }));
        runtime.shutdown().await.unwrap();
    }
    for probe in [true, false] {
        let pid_path = dir.path().join(if probe { "probe" } else { "helper" });
        let runtime = Runtime::new(
            Installation {
                java: "synthetic-java".into(),
                helper_candidates: vec![helper.clone()],
            },
            options(),
            Arc::new(FakeJava {
                mode: "timeout".into(),
                pid: pid_path.clone(),
                probe_delay: probe,
            }),
        );
        let rt = runtime.clone();
        let startup = tokio::spawn(async move { rt.get(&CancellationToken::new()).await });
        eventually(|| pid_path.exists()).await;
        let pid = std::fs::read_to_string(&pid_path)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        if probe {
            runtime.shutdown().await.unwrap();
            assert!(startup.await.unwrap().is_err());
        } else {
            startup.abort();
            let _ = startup.await;
            runtime.shutdown().await.unwrap();
        }
        assert!(!alive(pid));
    }
}
impl ProcessFactory for CountingFactory {
    fn spawn(&self, spec: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        self.0.fetch_add(1, Ordering::SeqCst);
        SystemProcessFactory.spawn(spec)
    }
}
#[tokio::test]
async fn dropping_workspace_runtime_stops_helper_even_while_a_caller_retains_client() {
    let dir = tempfile::tempdir().unwrap();
    let jar = dir.path().join("synthetic.jar");
    std::fs::write(&jar, []).unwrap();
    let runtime = Runtime::new(
        Installation {
            java: "synthetic-java".into(),
            helper_candidates: vec![jar],
        },
        options(),
        Arc::new(FakeJava {
            mode: "normal".into(),
            pid: dir.path().join("pid"),
            probe_delay: false,
        }),
    );
    let retained = runtime.get(&CancellationToken::new()).await.unwrap();
    let pid = retained.process_id();
    drop(runtime);
    eventually(|| !alive(pid)).await;
    retained.force_shutdown().await;
    assert_ne!(retained.state(), State::Running);
}
#[tokio::test]
async fn missing_java_is_lazy_actionable_and_version_parser_does_not_accept_launcher_noise() {
    let factory = Arc::new(CountingFactory(AtomicUsize::new(0)));
    let runtime = Runtime::new(
        Installation {
            java: PathBuf::from("/nonexistent/synthetic-java"),
            helper_candidates: vec![],
        },
        options(),
        factory.clone(),
    );
    assert_eq!(factory.0.load(Ordering::SeqCst), 0);
    let error = match runtime.get(&CancellationToken::new()).await {
        Err(error) => error,
        Ok(_) => panic!("missing Java accepted"),
    };
    assert_eq!(error.kind, Failure::Unavailable);
    assert!(error.message.contains("KODE_JAVA"));
    assert_eq!(factory.0.load(Ordering::SeqCst), 1);
    runtime.shutdown().await.unwrap();
    assert_eq!(
        java_major("launcher secret 99\nopenjdk version \"21.0.9\" 2025"),
        Some(21)
    );
    assert_eq!(java_major("java version \"1.8.0_71\""), Some(8));
    assert_eq!(java_major("Picked up OPTIONS: 99"), None);
}
