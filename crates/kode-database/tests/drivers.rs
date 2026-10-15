#![cfg(unix)]
use kode_database::{
    bridge::{
        client::{Failure, Options},
        runtime::{Installation, Runtime},
    },
    config::DatabasePaths,
    drivers::{Installer, Phase, Progress},
    model::DriverSpec,
};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec, SystemProcessFactory},
};
use serde_json::json;
use std::{
    path::PathBuf,
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
                "import sys;sys.stderr.write('openjdk version \\\"21.0.9\\\"\\n')".into(),
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
fn spec(id: &str) -> DriverSpec {
    serde_json::from_value(
        json!({"id":id,"providerId":"synthetic","driverClass":"synthetic.Driver"}),
    )
    .unwrap()
}
async fn phase(receive: &mut watch::Receiver<Progress>, expected: Phase) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if receive.borrow().phase == expected {
                return;
            }
            receive.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
fn fake_runtime(directory: &std::path::Path) -> (Runtime, Arc<FakeJava>) {
    let jar = directory.join("synthetic.jar");
    std::fs::write(&jar, []).unwrap();
    let factory = Arc::new(FakeJava {
        pid: directory.join("pid"),
        spawns: AtomicUsize::new(0),
    });
    (
        Runtime::new(
            Installation {
                java: "synthetic-java".into(),
                helper_candidates: vec![jar],
            },
            Options::default(),
            factory.clone(),
        ),
        factory,
    )
}
#[tokio::test]
async fn inert_construction_native_validation_and_independent_cancel_wait_for_original_final() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, factory) = fake_runtime(directory.path());
    let installer = Installer::new(runtime.clone());
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
    let mut invalid = spec("bad");
    invalid
        .extra
        .insert("password".into(), json!("synthetic-secret"));
    let (send, _) = watch::channel(Progress::default());
    assert_eq!(
        installer
            .resolve(&invalid, send, &CancellationToken::new())
            .await
            .unwrap_err()
            .kind,
        Failure::Protocol
    );
    assert_eq!(factory.spawns.load(Ordering::SeqCst), 0);
    let (send, mut receive) = watch::channel(Progress::default());
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let worker = installer.clone();
    let task =
        tokio::spawn(async move { worker.resolve(&spec("synthetic-block"), send, &token).await });
    phase(&mut receive, Phase::Downloading).await;
    cancel.cancel();
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(error.remote_code, Some(-32800));
    assert_eq!(receive.borrow().phase, Phase::Cancelled);
    assert!(!error.unknown_outcome);
    installer.stop_progress().await;
    runtime.shutdown().await.unwrap();
    let requests = std::fs::read_to_string(directory.path().join("pid.requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|method| *method == "driver.resolve")
            .count(),
        1
    );
    assert_eq!(
        requests
            .lines()
            .filter(|method| *method == "driver.cancel")
            .count(),
        1
    );
}
#[tokio::test]
async fn dropped_install_wait_routes_cancellation_and_real_final_status_wins_over_progress() {
    let directory = tempfile::tempdir().unwrap();
    let (runtime, _) = fake_runtime(directory.path());
    let installer = Installer::new(runtime.clone());
    let (send, mut receive) = watch::channel(Progress::default());
    let worker = installer.clone();
    let task = tokio::spawn(async move {
        worker
            .resolve(&spec("synthetic-block"), send, &CancellationToken::new())
            .await
    });
    phase(&mut receive, Phase::Downloading).await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let requests =
                std::fs::read_to_string(directory.path().join("pid.requests")).unwrap_or_default();
            if requests.contains("driver.cancel") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (send, receive) = watch::channel(Progress::default());
    let result = installer
        .resolve(&spec("ready"), send, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(result.resolved_version, "1.0");
    assert_eq!(receive.borrow().phase, Phase::Ready);
    installer.stop_progress().await;
    runtime.shutdown().await.unwrap();
}
#[test]
fn plugin_configuration_is_explicit_and_never_lossily_rewrites_non_unicode_paths() {
    let paths = DatabasePaths {
        root: "/synthetic/database".into(),
        legacy_root: None,
        plugin_root: "/synthetic/plugins".into(),
        legacy_plugin_root: Some("/synthetic/legacy".into()),
    };
    let options = Installer::configure(Options::default(), &paths).unwrap();
    assert!(
        options
            .helper_environment
            .contains(&("KODE_RS_JDBC_HOME".into(), "/synthetic/plugins".into()))
    );
    use std::os::unix::ffi::OsStringExt;
    let invalid = DatabasePaths {
        plugin_root: std::ffi::OsString::from_vec(vec![b'/', 255]).into(),
        ..paths
    };
    assert!(Installer::configure(Options::default(), &invalid).is_err());
}
#[tokio::test]
#[ignore = "requires built Java helper and bundled H2 artifact; scripts/test-jdbc-bridge.sh runs it"]
async fn packaged_helper_builtin_h2_local_multi_jar_alias_reuse_and_manifest_digest_agree() {
    let directory = tempfile::tempdir().unwrap();
    let paths = DatabasePaths {
        root: directory.path().join("database"),
        legacy_root: None,
        plugin_root: directory.path().join("plugins"),
        legacy_plugin_root: None,
    };
    let options = Installer::configure(Options::default(), &paths).unwrap();
    let runtime = Runtime::new(
        Installation {
            java: "java".into(),
            helper_candidates: vec![
                std::env::var_os("KODE_TEST_JDBC_HELPER")
                    .expect("KODE_TEST_JDBC_HELPER")
                    .into(),
            ],
        },
        options,
        Arc::new(SystemProcessFactory),
    );
    let installer = Installer::new(runtime.clone());
    let builtin = installer
        .inspect("h2", &[], "org.h2.Driver", &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(builtin.source, "application-classpath");
    assert!(!builtin.helper_epoch.is_empty());
    assert!(!paths.plugin_root.exists());
    let builtin_spec: DriverSpec = serde_json::from_value(
        json!({"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2"}),
    )
    .unwrap();
    assert_eq!(
        installer
            .inspect_spec(&builtin_spec, &CancellationToken::new())
            .await
            .unwrap()
            .resolved_version
            .as_deref(),
        Some("2.2.224")
    );
    assert!(!paths.plugin_root.exists());
    let jar = PathBuf::from(std::env::var_os("KODE_TEST_H2_JAR").expect("KODE_TEST_H2_JAR"));
    let original = std::fs::read(&jar).unwrap();
    let driver:DriverSpec=serde_json::from_value(json!({"id":"h2-local","providerId":"h2-local","driverClass":"org.h2.Driver","jarPath":jar})).unwrap();
    let (send, receive) = watch::channel(Progress::default());
    let installed = installer
        .resolve(&driver, send, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(receive.borrow().phase, Phase::Ready);
    assert!(installed.manifest_path.is_file());
    assert_eq!(installed.classpath.len(), 1);
    assert_eq!(std::fs::read(&jar).unwrap(), original);
    let info = installer
        .inspect(
            "h2-local",
            &installed.classpath,
            "org.h2.Driver",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(info.source, "isolated-classpath");
    assert_eq!(info.classpath_digest, installed.classpath_digest);
    assert_eq!(
        installer
            .inspect_spec(&driver, &CancellationToken::new())
            .await
            .unwrap()
            .classpath_digest,
        installed.classpath_digest
    );
    let mut alias = driver.clone();
    alias.id = "synthetic-alias".into();
    let (send, _) = watch::channel(Progress::default());
    let reused = installer
        .resolve(&alias, send, &CancellationToken::new())
        .await
        .unwrap();
    assert!(reused.reused);
    assert_eq!(reused.manifest_path, installed.manifest_path);
    // Exercise Maven model/transport classes from the actual shaded JAR too,
    // using a deterministic local repository rather than external network.
    let repository = directory.path().join("repository");
    let artifact = repository.join("synthetic/fixture/h2/1.0");
    std::fs::create_dir_all(&artifact).unwrap();
    std::fs::copy(&jar, artifact.join("h2-1.0.jar")).unwrap();
    std::fs::write(artifact.join("h2-1.0.pom"),"<project><modelVersion>4.0.0</modelVersion><groupId>synthetic.fixture</groupId><artifactId>h2</artifactId><version>1.0</version></project>").unwrap();
    let maven:DriverSpec=serde_json::from_value(json!({"id":"maven-h2","driverClass":"org.h2.Driver","artifact":{"repositoryType":"MAVEN_CUSTOM","groupId":"synthetic.fixture","artifactId":"h2","version":"1.0","repository":format!("file://{}/",repository.display())}})).unwrap();
    let (send, _) = watch::channel(Progress::default());
    let downloaded = installer
        .resolve(&maven, send, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(downloaded.source, "maven");
    assert_eq!(downloaded.classpath.len(), 1);
    assert_eq!(
        installer
            .inspect_spec(&maven, &CancellationToken::new())
            .await
            .unwrap()
            .resolved_version
            .as_deref(),
        Some("1.0")
    );
    let pid = runtime
        .get(&CancellationToken::new())
        .await
        .unwrap()
        .process_id();
    installer.stop_progress().await;
    runtime.shutdown().await.unwrap();
    assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
}
