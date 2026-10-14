use kode_platform::{
    jobs::{CancellationToken, Clock},
    logging::FileLog,
    paths::{ResourceOrigin, ResourcePaths, compiled_resources},
    process::{
        ManagedChild, ProcessFactory, ProcessSpec, SystemProcessFactory, TailBuffer, run_capture,
    },
};
use std::{collections::HashMap, fs, time::Duration};

struct FakeClock;
impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        99
    }
}

#[test]
fn resource_precedence_is_independent_of_working_directory() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["explicit", "rust", "legacy", "executable"] {
        fs::create_dir(temp.path().join(name)).unwrap();
        fs::write(temp.path().join(name).join("catalog"), name).unwrap();
    }
    let mut paths = ResourcePaths {
        explicit: Some(temp.path().join("explicit")),
        rust_home: Some(temp.path().join("rust")),
        legacy_home: Some(temp.path().join("legacy")),
        executable_dir: temp.path().join("executable"),
        config_dir: temp.path().join("config/kode-rs"),
    };
    let defaults: HashMap<&str, &[u8]> = HashMap::from([("catalog", b"compiled".as_slice())]);
    assert_eq!(
        paths.resolve("catalog", &defaults).unwrap().bytes,
        b"explicit"
    );
    paths.explicit = None;
    assert_eq!(paths.resolve("catalog", &defaults).unwrap().bytes, b"rust");
    fs::remove_file(temp.path().join("rust/catalog")).unwrap();
    assert_eq!(
        paths.resolve("catalog", &defaults).unwrap().bytes,
        b"executable"
    );
    paths.rust_home = None;
    assert_eq!(
        paths.resolve("catalog", &defaults).unwrap().bytes,
        b"legacy"
    );
    paths.legacy_home = None;
    fs::remove_file(temp.path().join("executable/catalog")).unwrap();
    assert_eq!(
        paths.resolve("catalog", &defaults).unwrap().origin,
        ResourceOrigin::Compiled
    );
    assert!(paths.resolve("../private", &defaults).is_err());
    assert!(paths.resolve("/absolute", &defaults).is_err());
    assert!(!compiled_resources()["styles/app.css"].is_empty());
    assert!(!compiled_resources()["keyword-patterns.txt"].is_empty());
}

#[test]
fn logs_rotate_without_terminal_output_and_stderr_tail_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("log.jsonl");
    let log = FileLog::open(&path, 4096).unwrap();
    for _ in 0..20 {
        log.record(
            &FakeClock,
            "info",
            "test",
            &"x".repeat(1000),
            serde_json::json!({"request":1}),
        )
        .unwrap();
    }
    assert!(fs::metadata(&path).unwrap().len() <= 4096);
    assert!(path.with_extension("previous.jsonl").exists());
    for line in fs::read_to_string(path).unwrap().lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["time_ms"], 99);
    }
    let tail = TailBuffer::new(4);
    tail.append(b"abcdef");
    assert_eq!(tail.bytes(), b"cdef");
}

#[cfg(unix)]
#[tokio::test]
async fn child_eof_exit_stderr_and_cancellation_are_drained_and_reaped() {
    let child = ManagedChild::spawn(ProcessSpec::new("/bin/cat"), &SystemProcessFactory).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.args = vec![
        "-c".into(),
        "printf output; printf error >&2; exit 7".into(),
    ];
    let output = run_capture(
        spec,
        vec![],
        CancellationToken::new(),
        &SystemProcessFactory,
    )
    .await
    .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"output");
    assert_eq!(output.stderr, "error");
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.args = vec!["-c".into(), "while :; do :; done".into()];
    let token = CancellationToken::new();
    let cancellation = token.clone();
    let task =
        tokio::spawn(async move { run_capture(spec, vec![], token, &SystemProcessFactory).await });
    tokio::task::yield_now().await;
    cancellation.cancel();
    let output = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(output.cancelled);
    assert!(!output.status.success());
    let child = ManagedChild::spawn(ProcessSpec::new("/bin/cat"), &SystemProcessFactory).unwrap();
    assert!(!child.terminate_and_reap().await.unwrap().success());
    let mut spec = ProcessSpec::new("/bin/sh");
    spec.args = vec!["-c".into(), "sleep 60 & printf parent-exited".into()];
    let output = tokio::time::timeout(
        Duration::from_secs(2),
        run_capture(
            spec,
            vec![],
            CancellationToken::new(),
            &SystemProcessFactory,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.stdout, b"parent-exited");
}

struct FailedFactory;
impl ProcessFactory for FailedFactory {
    fn spawn(&self, _: &ProcessSpec) -> kode_core::Result<tokio::process::Child> {
        Err(kode_core::DomainError::MissingResource(
            "injected process factory".into(),
        ))
    }
}
#[tokio::test]
async fn process_factory_is_injectable_and_missing_java_is_not_a_startup_requirement() {
    assert!(matches!(
        ManagedChild::spawn(ProcessSpec::new("java"), &FailedFactory),
        Err(kode_core::DomainError::MissingResource(_))
    ));
    // Constructing the platform runtime and defaults never launches Java/LSP/ttyd.
    let _notifications = kode_platform::jobs::Notifications::new(std::sync::Arc::new(FakeClock));
    assert!(!compiled_resources().is_empty());
}
