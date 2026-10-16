//! D09 combines real application owners with the actual external JVM. Fixtures
//! never touch a developer datasource, credential store, session or repository.
#![cfg(target_os = "linux")]
use kode_core::WorkspaceGeneration;
use kode_database::{
    bridge::runtime::Installation,
    catalog::{DriverRegistry, UrlCatalog},
    config::{DataSourceRepository, DatabasePaths},
    execution::{Phase, Snapshot},
    model::DataSourceDefinition,
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_tui::{
    app::{
        runtime::{AppRuntime, database::DatabaseConfiguration},
        state::OpenDestination,
        workspace::{OpenRequest, prepare_workspace},
    },
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
fn key(key: Key, ctrl: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl,
            ..Default::default()
        },
        repeat: false,
    })
}
fn alt(ch: char) -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character(ch),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn helper() -> PathBuf {
    std::env::var_os("KODE_TEST_JDBC_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../jdbc-bridge/target/kode-jdbc-bridge.jar")
        })
}
fn source(root: &Path) -> DataSourceDefinition {
    let jar = std::env::var_os("KODE_TEST_H2_JAR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar",
            )
        });
    serde_json::from_value(json!({"id":"fixture","name":"Qualification","driver":{"id":"h2","driverClass":"org.h2.Driver","jarPath":jar,"artifact":{"repositoryType":"LOCAL_FILE"}},"jdbcUrl":format!("jdbc:h2:file:{};WRITE_DELAY=0",root.join("database").display()),"username":"sa","autoCommit":true})).unwrap()
}
fn make(project: &Path, db: &Path, java: PathBuf) -> AppRuntime {
    let prepared = prepare_workspace(
        project,
        WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let registry = Arc::new(DriverRegistry::compiled().unwrap());
    let urls = Arc::new(UrlCatalog::compiled(&registry).unwrap());
    app.configure_database(DatabaseConfiguration {
        paths: DatabasePaths {
            root: db.into(),
            legacy_root: None,
            plugin_root: db.join("plugins"),
            legacy_plugin_root: None,
        },
        registry,
        urls,
        installation: Installation {
            java,
            helper_candidates: vec![helper()],
        },
    })
    .unwrap();
    app
}
async fn until(app: &mut AppRuntime, condition: impl Fn(&AppRuntime) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        app.pump().unwrap();
        let area = Rect::new(0, 0, 110, 34);
        app.state
            .render(area, &mut Buffer::empty(area), &Theme::compiled())
            .unwrap();
        if condition(app) {
            return;
        }
        assert!(
            Instant::now() < end,
            "notice={} / database={}",
            app.state.notice,
            app.state.database.tree.notice
        );
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
}
async fn connect(app: &mut AppRuntime) {
    app.state.dispatch(&alt('6')).unwrap();
    until(app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state.database.tree.selected = 0;
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    let services = app.database_services().unwrap();
    until(app, |_| services.connections.session("fixture").is_ok()).await;
}
async fn query(app: &mut AppRuntime, sql: &str) -> Snapshot {
    let id = app.state.active_sql().unwrap().to_string();
    let count = app.state.sql.manager.get(&id).unwrap().history.len();
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(app, |app| {
        app.state.sql.manager.get(&id).unwrap().history.len() > count
    })
    .await;
    app.state
        .sql
        .manager
        .get(&id)
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .clone()
}
async fn rest_after_database_failure(app: &mut AppRuntime) {
    use kode_rest::model::NodePath;
    use kode_tui::panels::rest::editor::form::Tab;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/after-jdbc-failure",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            assert!(request.len() < 64 * 1024);
        }
        let headers = String::from_utf8(request).unwrap();
        assert!(headers.starts_with("POST /after-jdbc-failure "));
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().to_string())
            })
            .unwrap()
            .parse()
            .unwrap();
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        assert_eq!(body, b"current REST buffer after JDBC failure");
        let body = b"REST survives JDBC failure";
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        socket.write_all(body).await.unwrap();
    });
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Independent REST","request":{"method":"POST","url":url,"body":{"mode":"raw","raw":"old"}}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(app, |app| app.state.rest.form.is_some()).await;
    let tab = app
        .state
        .rest
        .form
        .as_ref()
        .unwrap()
        .geometry
        .tabs
        .iter()
        .find(|(tab, _)| *tab == Tab::Body)
        .unwrap()
        .1;
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: tab.x,
            y: tab.y,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers::default(),
        }))
        .unwrap();
    until(app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            view.host.descriptor.role == kode_tui::editor::host::HostRole::RestBody
        })
    })
    .await;
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(
            "current REST buffer after JDBC failure".into(),
        ))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(app, |app| {
        !app.state.rest.run.responses.is_empty()
            && app.state.rest.response.installed.is_some()
            && app.state.rest.run.running.is_none()
    })
    .await;
    assert_eq!(
        app.state.rest.run.responses[0].body.as_ref(),
        b"REST survives JDBC failure"
    );
    assert_eq!(
        app.state.focus.focused,
        Some(app.state.editor.as_ref().unwrap().input.id)
    );
    server.await.unwrap();
}
fn wrapper(root: &Path) -> PathBuf {
    let path = root.join("java-wrapper");
    let log = root.join("helpers.jsonl");
    let script = format!(
        "#!/usr/bin/python3\nimport json,os,sys\nif '-jar' in sys.argv:\n with open({},'a') as out:\n  out.write(json.dumps({{'pid':os.getpid(),'start':open('/proc/self/stat').read().rsplit(') ',1)[1].split()[19]}})+'\\n')\nos.execvp('java',['java',*sys.argv[1:]])\n",
        serde_json::to_string(&log).unwrap()
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn records(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("helpers.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn kill_owned(root: &Path) {
    // pidfd pins the exact process; the birth check refuses a recycled PID.
    let code = "import json,os,signal,sys; r=json.loads(open(sys.argv[1]).read().splitlines()[-1]); fd=os.pidfd_open(r['pid']); assert open('/proc/%s/stat'%r['pid']).read().rsplit(') ',1)[1].split()[19]==r['start']; signal.pidfd_send_signal(fd,signal.SIGKILL); os.close(fd)";
    assert!(
        std::process::Command::new("python3")
            .args(["-c", code])
            .arg(root.join("helpers.jsonl"))
            .status()
            .unwrap()
            .success()
    );
}
#[tokio::test]
async fn wrong_java_version_is_actionable_redacted_and_native_edit_save_index_remain_available() {
    use kode_intelligence::facade::IntelligenceResources;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let db = root.path().join("config");
    DataSourceRepository::new(db.clone())
        .upsert(source(root.path()))
        .unwrap();
    let java = root.path().join("java17");
    std::fs::write(&java,"#!/usr/bin/python3\nimport sys\nsys.stderr.write('openjdk version \"17.0.1\"\\nsynthetic-launcher-private-value\\n')\n").unwrap();
    std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o700)).unwrap();
    let note = project.join("Native.kt");
    std::fs::write(&note, "class BeforeJavaError {}\n").unwrap();
    let mut app = make(&project, &db, java);
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    app.state.dispatch(&alt('6')).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.database.tree.notice.contains("Java 21 or newer")
    })
    .await;
    assert!(
        !app.state
            .database
            .tree
            .notice
            .contains("synthetic-launcher-private-value")
    );
    rest_after_database_failure(&mut app).await;
    native_file(&mut app, &note, "class NativeAfterWrongJava {}\n").await;
    until(&mut app, |app| {
        app.intelligence()
            .unwrap()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|symbol| symbol.name == "NativeAfterWrongJava")
    })
    .await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert!(
        !std::fs::read_to_string(project.join(".kode-rs.json"))
            .unwrap()
            .contains("synthetic-launcher-private-value")
    );
    assert!(!db.join("credentials-v1.json").exists());
}
#[tokio::test]
#[ignore = "actual Java/H2 busy workspace qualification; scripts/test-jdbc-bridge.sh supplies fixtures"]
async fn workspace_switch_during_sql_reaps_helper_preserves_buffer_and_drops_old_results_without_replaying()
 {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let db = root.path().join("config");
    DataSourceRepository::new(db.clone())
        .upsert(source(root.path()))
        .unwrap();
    let java = wrapper(root.path());
    let mut app = make(&first, &db, java);
    connect(&mut app).await;
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let console = app.state.active_sql().unwrap().to_string();
    let document = app.state.editor.as_ref().unwrap().document_id();
    let sql = "select sum(sqrt(X)) from system_range(1,1000000000)";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .result
            .as_ref()
            .is_some_and(|result| result.phase == Phase::Running)
    })
    .await;
    app.state
        .dispatch(&InputEvent::Paste(
            "\n-- preserved during workspace switch".into(),
        ))
        .unwrap();
    let buffer = app
        .state
        .documents
        .document(document)
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    let old = app.database_services().unwrap();
    tokio::time::timeout(Duration::from_secs(5), app.switch_workspace(second.clone()))
        .await
        .expect("busy workspace switch must cancel and join")
        .unwrap();
    assert_eq!(app.state.project.root, second);
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    assert_eq!(app.state.sql.manager.all().count(), 0);
    assert!(old.connections.session("fixture").is_err());
    for record in records(root.path()) {
        assert!(!Path::new(&format!("/proc/{}", record["pid"].as_u64().unwrap())).exists());
    }
    assert_eq!(records(root.path()).len(), 1);
    connect(&mut app).await;
    assert_eq!(records(root.path()).len(), 2);
    app.switch_workspace(first.clone()).await.unwrap();
    assert_eq!(records(root.path()).len(), 2);
    assert_eq!(app.state.active_sql(), Some(console.as_str()));
    let document = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .documents
            .document(document)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        buffer
    );
    assert!(app.state.sql.running.is_empty());
    assert!(
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .result
            .is_none()
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    for record in records(root.path()) {
        assert!(!Path::new(&format!("/proc/{}", record["pid"].as_u64().unwrap())).exists());
    }
}
async fn native_file(app: &mut AppRuntime, path: &Path, text: &str) {
    app.state
        .open(
            OpenRequest {
                path: path.into(),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(app,|app|app.state.editor.as_ref().is_some_and(|view|matches!(app.state.documents.document(view.document_id()).unwrap().origin(),kode_core::DocumentOrigin::File(origin)if origin==path))).await;
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(text.into())).unwrap();
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(app, |_| std::fs::read_to_string(path).unwrap() == text).await;
}
#[tokio::test]
#[ignore = "actual Java/H2 crash qualification; scripts/test-jdbc-bridge.sh supplies fixtures"]
async fn actual_helper_crash_during_sql_preserves_outcome_and_native_editor_git_index_then_explicit_reconnect_does_not_replay()
 {
    use kode_database::bridge::wire::Cell;
    use kode_intelligence::facade::IntelligenceResources;
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    git2::Repository::init(&project).unwrap();
    let note = project.join("Native.kt");
    std::fs::write(&note, "class NativeBeforeJdbc {}\n").unwrap();
    let db = root.path().join("config");
    DataSourceRepository::new(db.clone())
        .upsert(source(root.path()))
        .unwrap();
    let java = wrapper(root.path());
    let mut app = make(&project, &db, java);
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    connect(&mut app).await;
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    query(&mut app, "create table MARKER(ID int)").await;
    let console = app.state.active_sql().unwrap().to_string();
    let sql_document = app.state.editor.as_ref().unwrap().document_id();
    let sql = "insert into MARKER values(7);select sum(sqrt(X)) from system_range(1,1000000000)";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    until(&mut app, |app| {
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .result
            .as_ref()
            .is_some_and(|result| result.phase == Phase::Running)
    })
    .await;
    native_file(&mut app, &note, "class NativeDuringJdbc {}\n").await;
    assert!(app.state.sql.running.contains_key(&console));
    assert_eq!(records(root.path()).len(), 1);
    kill_owned(root.path());
    until(&mut app, |app| {
        !app.state.sql.running.contains_key(&console)
    })
    .await;
    let result = app
        .state
        .sql
        .manager
        .get(&console)
        .unwrap()
        .result
        .as_ref()
        .unwrap();
    assert!(result.unknown_outcome);
    assert!(result.error.is_some());
    assert_eq!(
        app.state
            .documents
            .document(sql_document)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        sql
    );
    rest_after_database_failure(&mut app).await;
    native_file(&mut app, &note, "class NativeAfterJdbcCrash {}\n").await;
    until(&mut app, |app| {
        app.intelligence()
            .unwrap()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|symbol| symbol.name == "NativeAfterJdbcCrash")
    })
    .await;
    app.state.dispatch(&alt('5')).unwrap();
    app.state.git.request_refresh();
    until(&mut app, |app| {
        app.state
            .git
            .read
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot
                    .status
                    .entries
                    .iter()
                    .any(|entry| entry.path == Path::new("Native.kt"))
            })
    })
    .await;
    assert_eq!(
        records(root.path()).len(),
        1,
        "native activity must not reconnect JDBC"
    );
    let first = records(root.path())[0]["pid"].as_u64().unwrap();
    assert!(!Path::new(&format!("/proc/{first}")).exists());
    app.state.dispatch(&alt('6')).unwrap();
    app.state
        .dispatch(&key(Key::Character('R'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.database.tree.notice.contains("restarted")
    })
    .await;
    connect(&mut app).await;
    assert_eq!(records(root.path()).len(), 2);
    app.state.open_database_session(sql_document).unwrap();
    let count = query(&mut app, "select count(*) from MARKER").await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="1")
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    for record in records(root.path()) {
        assert!(!Path::new(&format!("/proc/{}", record["pid"].as_u64().unwrap())).exists());
    }
}
