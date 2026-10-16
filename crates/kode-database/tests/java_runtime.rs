#![cfg(unix)]
use kode_database::java_runtime::{self, Installer, Options, Progress};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec, SystemProcessFactory},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use tokio::sync::watch;
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn package(extra: Option<&str>, bad_file: bool) -> Vec<u8> {
    package_with_java(
        b"#!/bin/sh\nprintf 'openjdk version \"21.0.9\"\\n' >&2\n",
        extra,
        bad_file,
    )
}
fn package_with_java(script: &[u8], extra: Option<&str>, bad_file: bool) -> Vec<u8> {
    let files = BTreeMap::from([
        ("bin/java", (script.to_vec(), true)),
        ("release", (b"JAVA_VERSION=\"21.0.9\"".to_vec(), false)),
        ("lib/modules", (b"synthetic-module-image".to_vec(), false)),
        (
            "legal/java.base/LICENSE",
            (b"Synthetic fixture license".to_vec(), false),
        ),
    ]);
    let entries:BTreeMap<_,_>=files.iter().map(|(name,(data,executable))|(*name,json!({"size":data.len(),"sha256":if bad_file && *name=="lib/modules" {"0".repeat(64)}else{sha(data)},"executable":executable}))).collect();
    let manifest = json!({"schemaVersion":1,"target":java_runtime::native_target().unwrap(),"javaMajor":21,"javaVersion":"21.0.9","modules":["java.base","java.sql","java.naming","jdk.crypto.ec"],"files":entries});
    let mut zip = zip::ZipWriter::new(Cursor::new(vec![]));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    zip.start_file("runtime-manifest.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    for (name, (data, _)) in files {
        zip.start_file(format!("runtime/{name}"), options).unwrap();
        zip.write_all(&data).unwrap();
    }
    if let Some(extra) = extra {
        if extra == "<symlink>" {
            zip.add_symlink("runtime/symlink", "../../escape", options)
                .unwrap();
        } else {
            zip.start_file(extra, options).unwrap();
            zip.write_all(b"must not escape staging").unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}
struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.base.trim_start_matches("http://"));
        if let Some(task) = self.task.take() {
            task.join().unwrap();
        }
    }
}
impl Server {
    fn new(bytes: Vec<u8>, wrong_checksum: bool, slow: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let seen = Arc::new(Mutex::new(vec![]));
        let captured = seen.clone();
        let address = base.clone();
        let task = thread::spawn(move || {
            let name = format!(
                "kode-jdbc-runtime-{}.zip",
                java_runtime::native_target().unwrap()
            );
            let checksum = format!(
                "{}  {name}\n",
                if wrong_checksum {
                    "0".repeat(64)
                } else {
                    sha(&bytes)
                }
            )
            .into_bytes();
            let release=json!({"tag_name":"runtime-fixture","assets":[{"name":name,"browser_download_url":format!("{address}/runtime.zip"),"size":bytes.len()},{"name":format!("{name}.sha256"),"browser_download_url":format!("{address}/checksum"),"size":checksum.len()}]}).to_string().into_bytes();
            let no_assets = json!({"tag_name":"fixture-without-runtime","assets":[]})
                .to_string()
                .into_bytes();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = vec![];
                let mut buf = [0; 1024];
                while !request.windows(4).any(|b| b == b"\r\n\r\n") && request.len() < 8192 {
                    let Ok(n) = stream.read(&mut buf) else { break };
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8_lossy(&request);
                let route = request
                    .lines()
                    .next()
                    .unwrap_or("")
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("");
                captured.lock().unwrap().push(route.into());
                let (status, body) = match route {
                    "/latest" => ("200 OK", release.as_slice()),
                    "/checksum" => ("200 OK", checksum.as_slice()),
                    "/no-assets" => ("200 OK", no_assets.as_slice()),
                    "/runtime.zip" => ("200 OK", bytes.as_slice()),
                    "/malformed" => ("200 OK", b"SYNTHETIC-PRIVATE-ERROR invalid json".as_slice()),
                    _ => ("404 Not Found", b"SYNTHETIC-PRIVATE-ERROR".as_slice()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                for chunk in body.chunks(if slow && route == "/runtime.zip" {
                    128
                } else {
                    64 * 1024
                }) {
                    if slow && route == "/runtime.zip" {
                        thread::sleep(Duration::from_millis(30));
                    }
                    if stream.write_all(chunk).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            base,
            stop,
            task: Some(task),
            seen,
        }
    }
    fn options(&self, root: &Path) -> Options {
        Options {
            root: root.into(),
            release_url: format!("{}/latest", self.base),
            target: java_runtime::native_target().unwrap().into(),
        }
    }
}
#[derive(Default)]
struct Spawns {
    count: AtomicUsize,
    pid: AtomicU32,
}
impl ProcessFactory for Spawns {
    fn spawn(&self, spec: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        self.count.fetch_add(1, Ordering::SeqCst);
        let child = SystemProcessFactory.spawn(spec)?;
        self.pid.store(child.id().unwrap(), Ordering::SeqCst);
        Ok(child)
    }
}
async fn install_with_factory(
    options: Options,
    cancel: CancellationToken,
    sender: watch::Sender<Progress>,
    factory: Arc<dyn ProcessFactory>,
) -> kode_core::Result<java_runtime::Receipt> {
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        Installer::new(options, handle, factory)?.run(&cancel, &sender)
    })
    .await
    .unwrap()
}
async fn install(
    options: Options,
    cancel: CancellationToken,
    sender: watch::Sender<Progress>,
) -> kode_core::Result<java_runtime::Receipt> {
    install_with_factory(options, cancel, sender, Arc::new(SystemProcessFactory)).await
}

#[tokio::test]
async fn synthetic_manifest_installs_atomically_with_one_release_lookup_and_restart_discovery() {
    let server = Server::new(package(None, false), false, false);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    assert!(
        java_runtime::current_java(&root, java_runtime::native_target().unwrap())
            .unwrap()
            .is_none()
    );
    assert!(!root.exists());
    let (sender, progress) = watch::channel(Progress::default());
    let receipt = install(server.options(&root), CancellationToken::new(), sender)
        .await
        .unwrap();
    assert_eq!(receipt.release, "runtime-fixture");
    assert_eq!(progress.borrow().phase, "Ready");
    let java = java_runtime::current_java(&root, java_runtime::native_target().unwrap())
        .unwrap()
        .unwrap();
    assert!(java.is_file());
    assert!(
        std::process::Command::new(java)
            .arg("-version")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        *server.seen.lock().unwrap(),
        vec!["/latest", "/checksum", "/runtime.zip"]
    );
    assert!(!root.join(".install.lock").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
}
#[tokio::test]
async fn checksum_and_archive_failures_leave_previous_runtime_and_no_staging_or_escape() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let valid = Server::new(package(None, false), false, false);
    let (sender, _) = watch::channel(Progress::default());
    install(valid.options(&root), CancellationToken::new(), sender)
        .await
        .unwrap();
    let previous = std::fs::read(root.join("current.json")).unwrap();
    for (extra, bad_file, bad_checksum) in [
        (None, false, true),
        (Some("../escape"), false, false),
        (Some("runtime/undeclared"), false, false),
        (None, true, false),
        (Some("<symlink>"), false, false),
    ] {
        let server = Server::new(package(extra, bad_file), bad_checksum, false);
        let (sender, phase) = watch::channel(Progress::default());
        let factory = Arc::new(Spawns::default());
        assert!(
            install_with_factory(
                server.options(&root),
                CancellationToken::new(),
                sender,
                factory.clone()
            )
            .await
            .is_err()
        );
        assert_eq!(
            factory.count.load(Ordering::SeqCst),
            0,
            "unverified runtime was executed"
        );
        assert_eq!(phase.borrow().phase, "Failed");
        assert_eq!(std::fs::read(root.join("current.json")).unwrap(), previous);
        assert!(!directory.path().join("escape").exists());
        assert!(!root.join(".install.lock").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    }
    let corrupt = Server::new(b"not a ZIP runtime".to_vec(), false, false);
    let (sender, _) = watch::channel(Progress::default());
    assert!(
        install(corrupt.options(&root), CancellationToken::new(), sender)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(root.join("current.json")).unwrap(), previous);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
}
#[tokio::test]
async fn cancellation_releases_install_lock_partial_download_and_allows_explicit_retry() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let slow = Server::new(package(None, false), false, true);
    let (sender, mut phase) = watch::channel(Progress::default());
    let cancel = CancellationToken::new();
    let options = slow.options(&root);
    let token = cancel.clone();
    let task = tokio::spawn(install(options, token, sender));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if phase.borrow().phase == "Downloading" {
                break;
            }
            phase.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let (sender, _) = watch::channel(Progress::default());
    assert!(
        install(slow.options(&root), CancellationToken::new(), sender)
            .await
            .is_err()
    );
    cancel.cancel();
    assert!(matches!(
        task.await.unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(!root.join(".install.lock").exists());
    assert!(!root.join("current.json").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    let ready = Server::new(package(None, false), false, false);
    let (sender, _) = watch::channel(Progress::default());
    install(ready.options(&root), CancellationToken::new(), sender)
        .await
        .unwrap();
}
#[tokio::test]
async fn metadata_http_errors_and_invalid_urls_are_actionable_without_provider_body_or_credentials()
{
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let server = Server::new(package(None, false), false, false);
    for route in ["missing", "malformed"] {
        let mut options = server.options(&root);
        options.release_url = format!("{}/{route}", server.base);
        let (sender, _) = watch::channel(Progress::default());
        let error = install(options, CancellationToken::new(), sender)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("SYNTHETIC-PRIVATE-ERROR"));
        if route == "missing" {
            assert!(error.contains("404"));
        }
        assert!(!root.join(".install.lock").exists());
    }
    let mut options = server.options(&root);
    options.release_url =
        "https://user:synthetic-private-password@example.test/releases/latest".into();
    let (sender, _) = watch::channel(Progress::default());
    let error = install(options, CancellationToken::new(), sender)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-private-password"));
}
#[tokio::test]
#[ignore = "requires the actual artifact generated by scripts/build-java-runtime.py"]
async fn generated_jlink_artifact_downloads_verifies_and_runs_a_real_java_vm() {
    let artifact = std::env::var_os("KODE_TEST_JAVA_RUNTIME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/java-runtime-artifacts")
                .join(format!(
                    "kode-jdbc-runtime-{}.zip",
                    java_runtime::native_target().unwrap()
                ))
        });
    let server = Server::new(std::fs::read(artifact).unwrap(), false, false);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let (sender, _) = watch::channel(Progress::default());
    install(server.options(&root), CancellationToken::new(), sender)
        .await
        .unwrap();
    let java = java_runtime::current_java(&root, java_runtime::native_target().unwrap())
        .unwrap()
        .unwrap();
    let modules = std::process::Command::new(&java)
        .arg("--list-modules")
        .output()
        .unwrap();
    assert!(modules.status.success());
    let modules = String::from_utf8(modules.stdout).unwrap();
    assert!(modules.contains("java.sql@") && modules.contains("jdk.crypto.ec@"));
    let helper =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../jdbc-bridge/target/kode-jdbc-bridge.jar");
    assert!(helper.is_file());
    use kode_database::bridge::{
        client::Options as BridgeOptions,
        runtime::{Installation, Runtime},
    };
    let runtime = Runtime::new_with_managed_java(
        Installation {
            java: "/synthetic/unavailable-java".into(),
            helper_candidates: vec![helper],
        },
        BridgeOptions::default(),
        Arc::new(SystemProcessFactory),
        root.clone(),
    );
    let client = runtime.get(&CancellationToken::new()).await.unwrap();
    assert_eq!(
        client.state(),
        kode_database::bridge::client::State::Running
    );
    assert!(runtime.last_error().is_none());
    runtime.shutdown().await.unwrap();
    let paths = kode_database::config::DatabasePaths {
        root: directory.path().join("db"),
        plugin_root: directory.path().join("drivers"),
        legacy_root: None,
        legacy_plugin_root: None,
    };
    let options =
        kode_database::drivers::Installer::configure(BridgeOptions::default(), &paths).unwrap();
    let helper =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../jdbc-bridge/target/kode-jdbc-bridge.jar");
    let runtime = Runtime::new_with_managed_java(
        Installation {
            java: "/synthetic/unavailable-java".into(),
            helper_candidates: vec![helper.clone()],
        },
        options,
        Arc::new(SystemProcessFactory),
        root,
    );
    let connections = kode_database::connections::Connections::new(
        kode_database::drivers::Installer::new(runtime),
        kode_database::credentials::CredentialStore::new(paths.root),
    );
    let definition=serde_json::from_value(json!({"id":"downloaded-java","name":"Downloaded Java fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":"jdbc:h2:mem:downloaded_java","username":"sa"})).unwrap();
    let cancel = CancellationToken::new();
    connections.open(definition, None, &cancel).await.unwrap();
    let executor = kode_database::execution::Executor::new(connections.clone());
    let (sender, _receiver) = watch::channel(kode_database::execution::Snapshot::default());
    let result = executor
        .execute(
            "downloaded-java",
            "select 73 as RUNTIME_PROOF".into(),
            Default::default(),
            sender,
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        result.completion.as_ref().unwrap().outcome,
        kode_database::bridge::wire::Outcome::Completed
    );
    assert!(
        matches!(&result.preview.result_sets[0].chunks[0][0][0],kode_database::bridge::wire::Cell::Text{text,..} if text=="73")
    );
    executor.shutdown().await;
    connections.shutdown().await.unwrap();
    // A valid local JVM wins without even reading a corrupt managed pointer.
    let corrupt = directory.path().join("unused-managed");
    std::fs::create_dir(&corrupt).unwrap();
    std::fs::write(corrupt.join("current.json"), b"invalid fixture pointer").unwrap();
    let runtime = Runtime::new_with_managed_java(
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
        BridgeOptions::default(),
        Arc::new(SystemProcessFactory),
        corrupt,
    );
    runtime.get(&CancellationToken::new()).await.unwrap();
    assert!(runtime.last_error().is_none());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_platform_assets_and_unavailable_network_leave_no_active_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let server = Server::new(package(None, false), false, false);
    let mut options = server.options(&root);
    options.release_url = format!("{}/no-assets", server.base);
    let (sender, _) = watch::channel(Progress::default());
    let error = install(options, CancellationToken::new(), sender)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("no matching runtime"));
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let mut options = server.options(&root);
    options.release_url = format!("http://{address}/latest");
    let (sender, _) = watch::channel(Progress::default());
    let error = install(options, CancellationToken::new(), sender)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("network access"));
    assert!(!root.join("current.json").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}
#[tokio::test]
async fn cancelling_the_runtime_probe_reaps_its_owned_child_before_removing_staging() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("managed");
    let server = Server::new(
        package_with_java(b"#!/bin/sh\nsleep 60\n", None, false),
        false,
        false,
    );
    let factory = Arc::new(Spawns::default());
    let token = CancellationToken::new();
    let (sender, phase) = watch::channel(Progress::default());
    let task = tokio::spawn(install_with_factory(
        server.options(&root),
        token.clone(),
        sender,
        factory.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if phase.borrow().phase == "Checking Java" && factory.pid.load(Ordering::SeqCst) != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    let pid = factory.pid.load(Ordering::SeqCst);
    token.cancel();
    assert!(matches!(
        task.await.unwrap(),
        Err(kode_core::DomainError::Cancelled)
    ));
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    assert!(!root.join("current.json").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}
