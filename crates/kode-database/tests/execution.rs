#![cfg(unix)]
use kode_database::{
    bridge::{
        client::{Failure, Options as BridgeOptions},
        runtime::{Installation, Runtime},
        wire::{Cell, Outcome},
    },
    connections::Connections,
    credentials::CredentialStore,
    drivers::Installer,
    execution::{Executor, Options, Phase, Snapshot},
    model::DataSourceDefinition,
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
use tokio::sync::watch;
struct FakeJava {
    pid: PathBuf,
    spawns: AtomicUsize,
}
impl ProcessFactory for FakeJava {
    fn spawn(&self, input: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        let mut spec = ProcessSpec::new("python3");
        spec.args = if input.args == ["-version"] {
            vec![
                "-c".into(),
                "import sys;sys.stderr.write('openjdk version \"21.0.9\"\\n')".into(),
            ]
        } else {
            vec![
                format!(
                    "{}/tests/fixtures/fake_helper.py",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "normal".into(),
                self.pid.to_string_lossy().into(),
            ]
        };
        SystemProcessFactory.spawn(&spec)
    }
}
fn definition() -> DataSourceDefinition {
    serde_json::from_value(json!({"id":"query-source","name":"Query fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":format!("jdbc:h2:mem:query_{}",std::process::id()),"username":"sa","autoCommit":false})).unwrap()
}
fn fake(root: &Path) -> (Connections, Executor, Arc<FakeJava>) {
    let jar = root.join("synthetic.jar");
    std::fs::write(&jar, []).unwrap();
    let factory = Arc::new(FakeJava {
        pid: root.join("pid"),
        spawns: AtomicUsize::new(0),
    });
    let runtime = Runtime::new(
        Installation {
            java: "synthetic-java".into(),
            helper_candidates: vec![jar],
        },
        BridgeOptions::default(),
        factory.clone(),
    );
    let connections = Connections::new(
        Installer::new(runtime),
        CredentialStore::new(root.join("unused-credentials")),
    );
    (connections.clone(), Executor::new(connections), factory)
}
async fn execute(executor: &Executor, sql: &str) -> Snapshot {
    let (send, _) = watch::channel(Snapshot::default());
    executor
        .execute(
            "query-source",
            sql.into(),
            Options::default(),
            send,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
}
async fn rows(receive: &mut watch::Receiver<Snapshot>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if receive
                .borrow()
                .preview
                .result_sets
                .first()
                .is_some_and(|set| set.received_rows > 0)
            {
                break;
            }
            receive.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn executor_construction_invalid_requests_and_disconnected_reads_are_inert() {
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, factory) = fake(root.path());
    let (send, _) = watch::channel(Snapshot::default());
    assert_eq!(
        executor
            .execute(
                "query-source",
                "select 1".into(),
                Options::default(),
                send,
                &CancellationToken::new()
            )
            .await
            .unwrap_err()
            .kind,
        Failure::Unavailable
    );
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("unused-credentials").exists());
    assert_eq!(
        executor
            .cancel("not-active", &CancellationToken::new())
            .await
            .unwrap(),
        kode_database::bridge::wire::CancelStatus::AlreadyFinished
    );
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn independent_notification_consumer_drains_all_rows_before_final_and_routes_concurrent_executions()
 {
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, _) = fake(root.path());
    connections
        .open(definition(), None, &CancellationToken::new())
        .await
        .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let executor = executor.clone();
        tasks.spawn(async move { execute(&executor, "fixture:normal").await });
    }
    while let Some(result) = tasks.join_next().await {
        let result = result.unwrap();
        assert_eq!(result.phase, Phase::Finished);
        assert_eq!(
            result.completion.as_ref().unwrap().outcome,
            Outcome::Completed
        );
        assert_eq!(result.completion.as_ref().unwrap().event_count, Some(4));
        assert_eq!(result.preview.result_sets[0].received_rows, 3);
        let rows = &result.preview.result_sets[0].chunks[0];
        assert_eq!(rows[1][0], Cell::Null);
        assert!(matches!(&rows[2][0],Cell::Text{text,..}if text.is_empty()));
        assert!(!format!("{result:?}").contains("synthetic-private-row"));
    }
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn user_cancel_keeps_received_rows_waits_for_original_final_and_never_means_rollback() {
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, factory) = fake(root.path());
    connections
        .open(definition(), None, &CancellationToken::new())
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let (send, mut receive) = watch::channel(Snapshot::default());
    let owned = executor.clone();
    let work = tokio::spawn(async move {
        owned
            .execute(
                "query-source",
                "fixture:block".into(),
                Options::default(),
                send,
                &token,
            )
            .await
    });
    rows(&mut receive).await;
    cancel.cancel();
    let result = work.await.unwrap().unwrap();
    assert_eq!(
        result.completion.as_ref().unwrap().outcome,
        Outcome::Cancelled
    );
    assert_eq!(result.preview.result_sets[0].received_rows, 3);
    assert!(result.preview.incomplete);
    assert_eq!(
        result.cancel_status,
        Some(kode_database::bridge::wire::CancelStatus::Requested)
    );
    assert!(!result.unknown_outcome);
    let trace = std::fs::read_to_string(factory.pid.with_extension("requests")).unwrap();
    assert_eq!(
        trace
            .lines()
            .filter(|line| *line == "query.execute")
            .count(),
        1
    );
    assert_eq!(
        trace.lines().filter(|line| *line == "query.cancel").count(),
        1
    );
    assert!(!trace.lines().any(|line| line == "connection.rollback"));
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn unsupported_cancel_is_visible_retryable_and_a_later_completed_result_stays_completed() {
    use kode_database::bridge::wire::CancelStatus;
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, _) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let (send, mut receive) = watch::channel(Snapshot::default());
    let owned = executor.clone();
    let work = tokio::spawn(async move {
        owned
            .execute(
                "query-source",
                "fixture:unsupported".into(),
                Options::default(),
                send,
                &CancellationToken::new(),
            )
            .await
    });
    rows(&mut receive).await;
    let id = receive.borrow().execution_id.clone().unwrap();
    assert_eq!(
        executor.cancel(&id, &token).await.unwrap(),
        CancelStatus::Unsupported
    );
    assert_eq!(
        receive.borrow().cancel_status,
        Some(CancelStatus::Unsupported)
    );
    assert_eq!(receive.borrow().phase, Phase::Cancelling);
    assert!(!work.is_finished());
    assert_eq!(
        executor.cancel(&id, &token).await.unwrap(),
        CancelStatus::Unsupported
    );
    connections
        .runtime()
        .get(&token)
        .await
        .unwrap()
        .request("test.release-queries", json!({}), false, &token)
        .await
        .unwrap();
    let result = work.await.unwrap().unwrap();
    assert_eq!(
        result.completion.as_ref().unwrap().outcome,
        Outcome::Completed
    );
    assert_eq!(result.cancel_status, Some(CancelStatus::Unsupported));
    assert!(!result.preview.incomplete);
    assert_eq!(
        executor.cancel(&id, &token).await.unwrap(),
        CancelStatus::AlreadyFinished
    );
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn dropping_the_ui_wait_routes_cancellation_and_shutdown_joins_owned_work() {
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, factory) = fake(root.path());
    connections
        .open(definition(), None, &CancellationToken::new())
        .await
        .unwrap();
    let (send, mut receive) = watch::channel(Snapshot::default());
    let owned = executor.clone();
    let work = tokio::spawn(async move {
        owned
            .execute(
                "query-source",
                "fixture:block".into(),
                Options::default(),
                send,
                &CancellationToken::new(),
            )
            .await
    });
    rows(&mut receive).await;
    work.abort();
    let _ = work.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if std::fs::read_to_string(factory.pid.with_extension("requests"))
                .unwrap_or_default()
                .lines()
                .any(|line| line == "query.cancel")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), executor.shutdown())
        .await
        .unwrap();
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn helper_crash_preserves_partial_rows_marks_unknown_and_does_not_replay() {
    let root = tempfile::tempdir().unwrap();
    let (connections, executor, factory) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let client = connections.runtime().get(&token).await.unwrap();
    let (send, mut receive) = watch::channel(Snapshot::default());
    let owned = executor.clone();
    let work = tokio::spawn(async move {
        owned
            .execute(
                "query-source",
                "fixture:crash-pending".into(),
                Options::default(),
                send,
                &CancellationToken::new(),
            )
            .await
    });
    rows(&mut receive).await;
    assert!(
        client
            .request("test.crash", json!({}), false, &token)
            .await
            .is_err()
    );
    let result = work.await.unwrap().unwrap();
    assert!(result.error.is_some());
    assert!(result.unknown_outcome);
    assert_eq!(result.preview.result_sets[0].received_rows, 3);
    assert!(result.preview.result_sets[0].incomplete);
    assert!(connections.session("query-source").is_err());
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 2);
    let trace = std::fs::read_to_string(factory.pid.with_extension("requests")).unwrap();
    assert_eq!(
        trace
            .lines()
            .filter(|line| *line == "query.execute")
            .count(),
        1
    );
    executor.shutdown().await;
    let _ = connections.shutdown().await;
}
#[tokio::test]
async fn invalid_event_sequence_fails_visibly_and_a_known_final_survives_immediate_helper_exit() {
    for (sql, valid) in [
        ("fixture:bad-sequence", false),
        ("fixture:final-exit", true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (connections, executor, _) = fake(root.path());
        connections
            .open(definition(), None, &CancellationToken::new())
            .await
            .unwrap();
        let result = execute(&executor, sql).await;
        if valid {
            assert_eq!(
                result.completion.as_ref().unwrap().outcome,
                Outcome::Completed
            );
            assert_eq!(result.preview.result_sets[0].received_rows, 3);
            assert!(!result.unknown_outcome);
        } else {
            assert_eq!(result.error.as_ref().unwrap().kind, Failure::Protocol);
            assert!(result.unknown_outcome);
            assert!(result.preview.incomplete);
            assert_eq!(result.preview.result_sets[0].received_rows, 0);
        }
        executor.shutdown().await;
        let _ = connections.shutdown().await;
    }
}
#[tokio::test]
#[ignore = "requires built Java helper; scripts/test-jdbc-bridge.sh supplies it"]
async fn actual_java_h2_streaming_transactions_types_limits_errors_and_context_are_native() {
    let root = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(
        Installation {
            java: "java".into(),
            helper_candidates: vec![PathBuf::from(
                std::env::var_os("KODE_TEST_JDBC_HELPER").expect("helper JAR"),
            )],
        },
        BridgeOptions::default(),
        Arc::new(SystemProcessFactory),
    );
    let connections = Connections::new(
        Installer::new(runtime),
        CredentialStore::new(root.path().join("unused-credentials")),
    );
    let executor = Executor::new(connections.clone());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    assert_eq!(
        execute(
            &executor,
            "create table item(id int primary key, name varchar(40))"
        )
        .await
        .completion
        .unwrap()
        .outcome,
        Outcome::Completed
    );
    assert_eq!(
        execute(&executor, "insert into item values(1, 'Alice'), (2, null)")
            .await
            .preview
            .update_counts,
        [2]
    );
    assert_eq!(
        execute(&executor, "update item set name='changed' where id=1")
            .await
            .preview
            .update_counts,
        [1]
    );
    connections.rollback("query-source", &token).await.unwrap();
    let count = execute(&executor, "select count(*) from item").await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="0")
    );
    execute(&executor, "insert into item values(3, 'committed')").await;
    connections.commit("query-source", &token).await.unwrap();
    connections.rollback("query-source", &token).await.unwrap();
    let count = execute(&executor, "select count(*) from item").await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="1")
    );
    let values=execute(&executor,"select null, '', X'00FF', cast(12345678901234567890.000000000000000001 as decimal(38,18)), timestamp '2024-01-02 03:04:05.123456789'").await;
    let row = &values.preview.result_sets[0].chunks[0][0];
    assert_eq!(row[0], Cell::Null);
    assert!(matches!(&row[1],Cell::Text{text,..}if text.is_empty()));
    assert!(matches!(&row[2],Cell::Binary{base64,..}if base64=="AP8="));
    assert!(
        matches!(&row[3],Cell::Text{text,..}if text=="12345678901234567890.000000000000000001")
    );
    assert!(matches!(&row[4],Cell::Text{text,..}if text=="2024-01-02 03:04:05.123456789"));
    let (send, _) = watch::channel(Snapshot::default());
    let capped = executor
        .execute(
            "query-source",
            "select x from system_range(1,1000)".into(),
            Options {
                max_rows: 450,
                fetch_size: 200,
            },
            send,
            &token,
        )
        .await
        .unwrap();
    assert_eq!(capped.preview.result_sets[0].received_rows, 450);
    assert!(capped.preview.result_sets[0].limited);
    assert_eq!(
        capped.preview.result_sets[0]
            .chunks
            .iter()
            .map(|chunk| chunk.len())
            .collect::<Vec<_>>(),
        [200, 200, 50]
    );
    let failed = execute(&executor, "select * from missing_table").await;
    assert_eq!(failed.completion.as_ref().unwrap().outcome, Outcome::Failed);
    assert!(
        failed
            .completion
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .sql_state
            .is_some()
    );
    assert!(
        !connections
            .session("query-source")
            .unwrap()
            .transaction_usable
    );
    connections.rollback("query-source", &token).await.unwrap();
    assert!(
        connections
            .session("query-source")
            .unwrap()
            .transaction_usable
    );
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
    assert!(!root.path().join("unused-credentials").exists());
}
