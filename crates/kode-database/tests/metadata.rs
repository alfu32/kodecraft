#![cfg(unix)]
use kode_database::{
    bridge::{
        client::{Failure, Options as BridgeOptions},
        runtime::{Installation, Runtime},
    },
    connections::{Connections, Context},
    credentials::CredentialStore,
    drivers::Installer,
    execution::{Executor, Options, Snapshot},
    metadata::Metadata,
    model::{DataSourceDefinition, MetadataRequest, MetadataResult},
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
    serde_json::from_value(json!({"id":"metadata-source","name":"Metadata fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":format!("jdbc:h2:mem:metadata_{}",std::process::id()),"username":"sa","autoCommit":true})).unwrap()
}
fn request(kind: &str, schema: Option<&str>) -> MetadataRequest {
    serde_json::from_value(json!({"kind":kind,"schema":schema})).unwrap()
}
fn fake(root: &Path) -> (Connections, Metadata, Arc<FakeJava>) {
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
    (connections.clone(), Metadata::new(connections), factory)
}
async fn introspect(metadata: &Metadata, request: MetadataRequest) -> Arc<MetadataResult> {
    metadata
        .introspect("metadata-source", request, false, &CancellationToken::new())
        .await
        .unwrap()
}
fn count(factory: &FakeJava) -> usize {
    std::fs::read_to_string(factory.pid.with_extension("requests"))
        .unwrap_or_default()
        .lines()
        .filter(|line| *line == "metadata.list")
        .count()
}
async fn wait_count(factory: &FakeJava, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while count(factory) < expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn construction_cache_reads_invalid_disconnected_requests_and_shutdown_are_inert() {
    let root = tempfile::tempdir().unwrap();
    let (connections, metadata, factory) = fake(root.path());
    assert!(
        metadata
            .cached("metadata-source", &request("ROOT", None))
            .is_none()
    );
    assert_eq!(
        metadata
            .introspect(
                "metadata-source",
                request("ROOT", None),
                false,
                &CancellationToken::new()
            )
            .await
            .unwrap_err()
            .kind,
        Failure::Unavailable
    );
    assert_eq!(
        metadata
            .introspect(
                "metadata-source",
                request("OBJECT", None),
                false,
                &CancellationToken::new()
            )
            .await
            .unwrap_err()
            .kind,
        Failure::Unavailable
    );
    metadata.refresh_all("metadata-source");
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("unused-credentials").exists());
}
#[tokio::test]
async fn original_individual_and_datasource_invalidation_preserves_other_requests_and_sources() {
    let root = tempfile::tempdir().unwrap();
    let (connections, metadata, factory) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let root_request = request("ROOT", None);
    let schema = request("SCHEMA", Some("PUBLIC"));
    let a = introspect(&metadata, root_request.clone()).await;
    assert!(metadata.is_current("metadata-source", &root_request, &a));
    let b = introspect(&metadata, schema.clone()).await;
    assert!(Arc::ptr_eq(
        &a,
        &introspect(&metadata, root_request.clone()).await
    ));
    assert_eq!(count(&factory), 2);
    metadata.invalidate("metadata-source", &root_request);
    assert!(!metadata.is_current("metadata-source", &root_request, &a));
    assert!(metadata.cached("metadata-source", &root_request).is_none());
    assert!(Arc::ptr_eq(
        &b,
        &metadata.cached("metadata-source", &schema).unwrap()
    ));
    let mut other = definition();
    other.id = "other".into();
    connections.open(other, None, &token).await.unwrap();
    let other = metadata
        .introspect("other", root_request.clone(), false, &token)
        .await
        .unwrap();
    metadata.refresh_all("metadata-source");
    assert!(metadata.cached("metadata-source", &schema).is_none());
    assert!(Arc::ptr_eq(
        &other,
        &metadata.cached("other", &root_request).unwrap()
    ));
    let fresh = introspect(&metadata, root_request.clone()).await;
    let refreshed = metadata
        .introspect("metadata-source", root_request.clone(), true, &token)
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&fresh, &refreshed));
    assert_eq!(count(&factory), 5);
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn context_sql_and_reconnect_change_cache_identity_without_replaying_requests() {
    let root = tempfile::tempdir().unwrap();
    let (connections, metadata, factory) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let root_request = request("ROOT", None);
    introspect(&metadata, root_request.clone()).await;
    connections
        .set_context(
            "metadata-source",
            Context {
                schema: Some(Some("PUBLIC".into())),
                ..Default::default()
            },
            &token,
        )
        .await
        .unwrap();
    assert!(metadata.cached("metadata-source", &root_request).is_none());
    introspect(&metadata, root_request.clone()).await;
    let executor = Executor::new(connections.clone());
    let (send, _) = tokio::sync::watch::channel(Snapshot::default());
    executor
        .execute(
            "metadata-source",
            "fixture:normal".into(),
            Options::default(),
            send,
            &token,
        )
        .await
        .unwrap();
    assert!(metadata.cached("metadata-source", &root_request).is_none());
    introspect(&metadata, root_request.clone()).await;
    let old = connections.session("metadata-source").unwrap();
    connections.close("metadata-source", &token).await.unwrap();
    assert!(metadata.cached("metadata-source", &root_request).is_none());
    let new = connections.open(definition(), None, &token).await.unwrap();
    assert_ne!(old.connection_id, new.connection_id);
    introspect(&metadata, root_request).await;
    assert_eq!(count(&factory), 4);
    executor.shutdown().await;
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn late_metadata_after_refresh_context_change_or_reconnect_never_populates_new_cache() {
    for change in ["refresh", "context", "reconnect"] {
        let root = tempfile::tempdir().unwrap();
        let (connections, metadata, factory) = fake(root.path());
        let token = CancellationToken::new();
        connections.open(definition(), None, &token).await.unwrap();
        let blocked = request("SCHEMA", Some("__block"));
        let cloned = metadata.clone();
        let input = blocked.clone();
        let task = tokio::spawn(async move {
            cloned
                .introspect("metadata-source", input, false, &CancellationToken::new())
                .await
        });
        wait_count(&factory, 1).await;
        match change {
            "refresh" => metadata.refresh_all("metadata-source"),
            "context" => {
                connections
                    .set_context(
                        "metadata-source",
                        Context {
                            schema: Some(Some("other".into())),
                            ..Default::default()
                        },
                        &token,
                    )
                    .await
                    .unwrap();
            }
            _ => {
                connections.close("metadata-source", &token).await.unwrap();
                connections.open(definition(), None, &token).await.unwrap();
            }
        }
        connections
            .runtime()
            .get(&token)
            .await
            .unwrap()
            .request_with_timeout(
                "test.release-metadata",
                json!({}),
                false,
                Duration::from_secs(2),
                &token,
            )
            .await
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap_err().kind, Failure::Cancelled);
        assert!(metadata.cached("metadata-source", &blocked).is_none());
        introspect(&metadata, blocked).await;
        assert_eq!(count(&factory), 2);
        metadata.shutdown().await;
        connections.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn cancelled_or_dropped_waits_never_cache_and_shutdown_joins_native_work() {
    for dropped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (connections, metadata, factory) = fake(root.path());
        let token = CancellationToken::new();
        connections.open(definition(), None, &token).await.unwrap();
        let blocked = request("SCHEMA", Some("__block"));
        let cloned = metadata.clone();
        let input = blocked.clone();
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let task = tokio::spawn(async move {
            cloned
                .introspect("metadata-source", input, false, &child)
                .await
        });
        wait_count(&factory, 1).await;
        if dropped {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancel.cancel();
            assert_eq!(task.await.unwrap().unwrap_err().kind, Failure::Cancelled);
        }
        metadata.shutdown().await;
        assert!(metadata.cached("metadata-source", &blocked).is_none());
        connections.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn unsupported_message_is_preserved_malformed_results_are_not_cached_and_crash_is_not_replayed()
 {
    let root = tempfile::tempdir().unwrap();
    let (connections, metadata, factory) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let unsupported = introspect(&metadata, request("SCHEMA", Some("__unsupported"))).await;
    assert!(unsupported.objects.is_empty());
    assert!(
        unsupported
            .message
            .as_ref()
            .unwrap()
            .contains("does not support")
    );
    assert!(!unsupported.capabilities.as_ref().unwrap().functions);
    let bad = request("SCHEMA", Some("__bad"));
    assert_eq!(
        metadata
            .introspect("metadata-source", bad.clone(), false, &token)
            .await
            .unwrap_err()
            .kind,
        Failure::Protocol
    );
    assert!(metadata.cached("metadata-source", &bad).is_none());
    let failure = metadata
        .introspect(
            "metadata-source",
            request("SCHEMA", Some("__crash")),
            false,
            &token,
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, Failure::Transport);
    assert_eq!(count(&factory), 3);
    assert!(
        metadata
            .cached("metadata-source", &request("SCHEMA", Some("__unsupported")))
            .is_none()
    );
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 2);
    metadata.shutdown().await;
    let _ = connections.shutdown().await;
}
#[tokio::test]
async fn metadata_sql_failure_preserves_sqlstate_and_applies_owner_transaction_state() {
    let root = tempfile::tempdir().unwrap();
    let (connections, metadata, _) = fake(root.path());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    let failure = metadata
        .introspect(
            "metadata-source",
            request("SCHEMA", Some("__failure")),
            false,
            &token,
        )
        .await
        .unwrap_err();
    assert_eq!(failure.kind, Failure::Remote);
    assert_eq!(
        failure.remote_data().unwrap()["sqlError"]["sqlState"],
        "42000"
    );
    let session = connections.session("metadata-source").unwrap();
    assert!(!session.transaction_usable);
    assert!(!session.auto_commit);
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
}
#[tokio::test]
#[ignore = "requires built Java helper; scripts/test-jdbc-bridge.sh supplies it"]
async fn actual_java_h2_tree_refresh_ddl_context_and_reconnect_use_native_cache() {
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
    let metadata = Metadata::new(connections.clone());
    let executor = Executor::new(connections.clone());
    let token = CancellationToken::new();
    connections.open(definition(), None, &token).await.unwrap();
    async fn sql(executor: &Executor, sql: &str) {
        let (send, _) = tokio::sync::watch::channel(Snapshot::default());
        let result = executor
            .execute(
                "metadata-source",
                sql.into(),
                Options::default(),
                send,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.completion.unwrap().outcome,
            kode_database::bridge::wire::Outcome::Completed
        );
    }
    sql(&executor, "create table parent(id int primary key)").await;
    sql(
        &executor,
        "create table child(id int primary key, parent_id int references parent(id))",
    )
    .await;
    let root_request = request("ROOT", None);
    let tree = introspect(&metadata, root_request.clone()).await;
    assert!(!tree.objects.is_empty());
    assert!(Arc::ptr_eq(
        &tree,
        &introspect(&metadata, root_request.clone()).await
    ));
    let tables: MetadataRequest =
        serde_json::from_value(json!({"kind":"CATEGORY","schema":"PUBLIC","category":"TABLE"}))
            .unwrap();
    let initial = introspect(&metadata, tables.clone()).await;
    assert!(initial.objects.iter().any(|o| o.name == "CHILD"));
    let children: MetadataRequest = serde_json::from_value(
        json!({"kind":"OBJECT","schema":"PUBLIC","objectName":"CHILD","objectType":"TABLE"}),
    )
    .unwrap();
    let child = introspect(&metadata, children).await;
    assert!(
        child
            .objects
            .iter()
            .any(|o| o.object_type == kode_database::model::DatabaseObjectType::ForeignKey)
    );
    sql(&executor, "create table added(id int)").await;
    assert!(metadata.cached("metadata-source", &tables).is_none());
    assert!(
        introspect(&metadata, tables.clone())
            .await
            .objects
            .iter()
            .any(|o| o.name == "ADDED")
    );
    sql(&executor, "create schema extra").await;
    connections
        .set_context(
            "metadata-source",
            Context {
                schema: Some(Some("EXTRA".into())),
                ..Default::default()
            },
            &token,
        )
        .await
        .unwrap();
    assert!(metadata.cached("metadata-source", &tables).is_none());
    introspect(&metadata, root_request.clone()).await;
    connections.close("metadata-source", &token).await.unwrap();
    connections.open(definition(), None, &token).await.unwrap();
    assert!(metadata.cached("metadata-source", &root_request).is_none());
    introspect(&metadata, root_request).await;
    executor.shutdown().await;
    metadata.shutdown().await;
    connections.shutdown().await.unwrap();
    assert!(!root.path().join("unused-credentials").exists());
}
