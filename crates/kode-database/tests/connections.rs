#![cfg(unix)]
use kode_database::{
    bridge::{
        client::{Failure, Options},
        runtime::{Installation, Runtime},
    },
    connections::{Connections, Context, Phase},
    credentials::{CredentialStore, Secret},
    drivers::Installer,
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
fn definition(id: &str) -> DataSourceDefinition {
    serde_json::from_value(json!({"id":id,"name":id,"driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":format!("jdbc:h2:mem:{id}"),"username":"synthetic-user"})).unwrap()
}
fn fake(root: &Path) -> (Connections, Arc<FakeJava>, CredentialStore) {
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
        Options::default(),
        factory.clone(),
    );
    let credentials = CredentialStore::new(root.join("credentials"));
    (
        Connections::new(Installer::new(runtime), credentials.clone()),
        factory,
        credentials,
    )
}
async fn requested(factory: &FakeJava, method: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if std::fs::read_to_string(factory.pid.with_extension("requests"))
                .unwrap_or_default()
                .lines()
                .any(|line| line == method)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn construction_and_snapshot_are_inert_and_shutdown_needs_no_java_or_credentials() {
    let root = tempfile::tempdir().unwrap();
    let (connections, factory, _) = fake(root.path());
    assert_eq!(connections.snapshot("unused").phase, Phase::Disconnected);
    assert!(connections.session("unused").is_err());
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
    connections.shutdown().await.unwrap();
    assert!(!root.path().join("credentials").exists());
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn credentials_are_injected_from_private_snapshot_and_context_and_transaction_state_are_shared()
 {
    let root = tempfile::tempdir().unwrap();
    let (connections, factory, credentials) = fake(root.path());
    let cancel = CancellationToken::new();
    credentials
        .put("password-ref", &Secret::new("synthetic-password".into()))
        .unwrap();
    credentials
        .put("token-ref", &Secret::new("synthetic-token".into()))
        .unwrap();
    let before = std::fs::read(root.path().join("credentials/credentials-v1.json")).unwrap();
    let mut def = definition("credentials");
    def.credential_reference = Some("password-ref".into());
    def.credential_properties
        .insert("token".into(), "token-ref".into());
    def.properties
        .insert("user".into(), "overridden-user".into());
    def.auto_commit = false;
    assert!(
        connections
            .test(def.clone(), None, &cancel)
            .await
            .unwrap()
            .success
    );
    assert_eq!(connections.snapshot(&def.id).phase, Phase::Disconnected);
    let first = connections.open(def.clone(), None, &cancel).await.unwrap();
    let second = connections.open(def.clone(), None, &cancel).await.unwrap();
    assert_eq!(first.connection_id, second.connection_id);
    assert!(!first.auto_commit);
    assert!(
        connections
            .commit(&def.id, &cancel)
            .await
            .unwrap()
            .transaction_usable
    );
    assert!(
        connections
            .rollback(&def.id, &cancel)
            .await
            .unwrap()
            .transaction_usable
    );
    let changed = connections
        .set_context(
            &def.id,
            Context {
                auto_commit: Some(true),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(changed.auto_commit);
    assert!(connections.session(&def.id).unwrap().auto_commit);
    let mut different = def.clone();
    different.auto_commit = true;
    assert_eq!(
        connections
            .open(different, None, &cancel)
            .await
            .unwrap_err()
            .bridge
            .kind,
        Failure::Busy
    );
    connections
        .definition_changed(&def.id, &cancel)
        .await
        .unwrap();
    assert_eq!(connections.snapshot(&def.id).phase, Phase::Disconnected);
    let next = connections.open(def.clone(), None, &cancel).await.unwrap();
    assert_ne!(first.connection_id, next.connection_id);
    assert!(!format!("{next:?}").contains("synthetic-password"));
    assert_eq!(
        std::fs::read(root.path().join("credentials/credentials-v1.json")).unwrap(),
        before
    );
    let missing = definition("missing");
    let mut missing = missing;
    missing.credential_reference = Some("absent".into());
    assert_eq!(
        connections
            .test(missing, None, &cancel)
            .await
            .unwrap_err()
            .bridge
            .kind,
        Failure::Unavailable
    );
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 2);
    connections.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancelled_and_dropped_open_are_closed_and_tracked_before_workspace_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let (connections, factory, _) = fake(root.path());
    let cancel = CancellationToken::new();
    let owned = connections.clone();
    let token = cancel.clone();
    let work = tokio::spawn(async move { owned.open(definition("blocked"), None, &token).await });
    requested(&factory, "connection.open").await;
    cancel.cancel();
    let failure = work.await.unwrap().unwrap_err();
    assert_eq!(failure.bridge.kind, Failure::Cancelled);
    assert!(failure.bridge.unknown_outcome);
    assert_ne!(connections.snapshot("blocked").phase, Phase::Connected);
    let owned = connections.clone();
    let work = tokio::spawn(async move {
        owned
            .open(
                definition("blocked-dropped"),
                None,
                &CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if std::fs::read_to_string(factory.pid.with_extension("requests"))
                .unwrap_or_default()
                .lines()
                .filter(|line| *line == "connection.open")
                .count()
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    work.abort();
    let _ = work.await;
    tokio::time::timeout(Duration::from_secs(3), connections.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(connections.session("blocked-dropped").is_err());
}
#[tokio::test]
async fn helper_crash_with_pending_commit_never_reports_rollback_or_replays_and_requires_explicit_reconnect()
 {
    let root = tempfile::tempdir().unwrap();
    let (connections, factory, _) = fake(root.path());
    let cancel = CancellationToken::new();
    let mut def = definition("crash");
    def.auto_commit = false;
    def.properties
        .insert("fixtureMode".into(), "blockCommit".into());
    let first = connections.open(def.clone(), None, &cancel).await.unwrap();
    let client = connections.runtime().get(&cancel).await.unwrap();
    let owned = connections.clone();
    let work = tokio::spawn(async move { owned.commit("crash", &CancellationToken::new()).await });
    requested(&factory, "connection.commit").await;
    assert!(
        client
            .request("test.crash", json!({}), false, &cancel)
            .await
            .is_err()
    );
    assert!(work.await.unwrap().unwrap_err().bridge.unknown_outcome);
    tokio::time::timeout(Duration::from_secs(2), async {
        while connections.snapshot("crash").phase != Phase::Error {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(connections.snapshot("crash").unknown_outcome);
    assert!(connections.snapshot("crash").session.is_none());
    assert!(connections.rollback("crash", &cancel).await.is_err());
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 2);
    let requests = std::fs::read_to_string(factory.pid.with_extension("requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|line| *line == "connection.commit")
            .count(),
        1
    );
    assert!(!requests.lines().any(|line| line == "connection.rollback"));
    connections.runtime().restart(&cancel).await.unwrap();
    let next = connections.open(def, None, &cancel).await.unwrap();
    assert_ne!(first.helper_epoch, next.helper_epoch);
    assert_ne!(first.connection_id, next.connection_id);
    connections.shutdown().await.unwrap();
}
#[tokio::test]
#[ignore = "requires built Java helper; scripts/test-jdbc-bridge.sh supplies an isolated runtime"]
async fn actual_java_h2_connection_test_context_transactions_credentials_and_reconnect() {
    let root = tempfile::tempdir().unwrap();
    let jar = PathBuf::from(std::env::var_os("KODE_TEST_JDBC_HELPER").expect("helper JAR"));
    let runtime = Runtime::new(
        Installation {
            java: "java".into(),
            helper_candidates: vec![jar],
        },
        Options::default(),
        Arc::new(SystemProcessFactory),
    );
    let credentials = CredentialStore::new(root.path().join("credentials"));
    credentials
        .put("password", &Secret::new("synthetic-password".into()))
        .unwrap();
    let connections = Connections::new(Installer::new(runtime), credentials);
    let cancel = CancellationToken::new();
    let mut def = definition("native-h2");
    def.jdbc_url = format!("jdbc:h2:mem:native_{}", std::process::id());
    def.username = "sa".into();
    def.credential_reference = Some("password".into());
    def.auto_commit = false;
    let tested = connections.test(def.clone(), None, &cancel).await.unwrap();
    assert_eq!(tested.product_name, "H2");
    assert!(!tested.product_version.is_empty());
    let first = connections.open(def.clone(), None, &cancel).await.unwrap();
    assert!(!first.auto_commit);
    assert_eq!(
        connections
            .open(def.clone(), None, &cancel)
            .await
            .unwrap()
            .connection_id,
        first.connection_id
    );
    assert!(
        !connections
            .commit(&def.id, &cancel)
            .await
            .unwrap()
            .auto_commit
    );
    assert!(
        connections
            .rollback(&def.id, &cancel)
            .await
            .unwrap()
            .transaction_usable
    );
    let schema = connections
        .set_context(
            &def.id,
            Context {
                schema: Some(Some("INFORMATION_SCHEMA".into())),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(schema.schema.as_deref(), Some("INFORMATION_SCHEMA"));
    let reset = connections
        .set_context(
            &def.id,
            Context {
                schema: Some(None),
                auto_commit: Some(true),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(reset.schema.as_deref(), Some("PUBLIC"));
    assert!(reset.auto_commit);
    let failure = connections
        .set_context(
            &def.id,
            Context {
                schema: Some(Some("MISSING_SCHEMA".into())),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .unwrap_err();
    assert!(failure.sql.is_some());
    assert!(failure.sql.as_ref().unwrap().sql_state.is_some());
    assert!(!format!("{failure:?}").contains("synthetic-password"));
    assert_eq!(
        connections.session(&def.id).unwrap().schema.as_deref(),
        Some("PUBLIC")
    );
    let wrong = connections
        .test(
            def.clone(),
            Some(Secret::new("synthetic-wrong".into())),
            &cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(
        wrong.sql.as_ref().unwrap().sql_state.as_deref(),
        Some("28000")
    );
    assert_eq!(
        connections.session(&def.id).unwrap().connection_id,
        first.connection_id
    );
    connections.close(&def.id, &cancel).await.unwrap();
    let next = connections.open(def, None, &cancel).await.unwrap();
    assert_ne!(first.connection_id, next.connection_id);
    connections.shutdown().await.unwrap();
}
