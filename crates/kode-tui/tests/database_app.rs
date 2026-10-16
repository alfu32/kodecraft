use kode_core::{DocumentOrigin, WorkspaceGeneration};
use kode_database::{
    bridge::runtime::Installation,
    catalog::{DriverRegistry, UrlCatalog},
    config::{DataSourceRepository, DatabasePaths},
    model::DataSourceDefinition,
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_tui::{
    app::{
        runtime::{AppRuntime, database::DatabaseConfiguration},
        workspace::prepare_workspace,
    },
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::{
    path::Path,
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
fn alt6() -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character('6'),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn definition() -> DataSourceDefinition {
    serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":"jdbc:h2:mem:app_fixture","username":"sa"})).unwrap()
}
fn make(root: &Path, db: &Path, java: Option<&Path>) -> (AppRuntime, Arc<ClipboardService>) {
    make_installation(
        root,
        db,
        Installation {
            java: java
                .unwrap_or_else(|| Path::new("/synthetic/absent-java"))
                .into(),
            helper_candidates: vec![],
        },
    )
}
fn make_installation(
    root: &Path,
    db: &Path,
    installation: Installation,
) -> (AppRuntime, Arc<ClipboardService>) {
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let clipboard = Arc::new(ClipboardService::internal());
    let mut app = AppRuntime::with_services(
        prepared,
        clipboard.clone(),
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
        installation,
    })
    .unwrap();
    (app, clipboard)
}

#[tokio::test]
#[ignore = "requires built Java helper and H2 JAR; scripts/test-jdbc-bridge.sh supplies them"]
async fn actual_java_h2_panel_download_test_connect_metadata_typing_sessions_disconnect_and_shutdown()
 {
    use kode_database::{
        execution::{Executor, Options, Snapshot},
        model::DatabaseObjectType,
    };
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db-root");
    let helper = std::env::var_os("KODE_TEST_JDBC_HELPER")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../jdbc-bridge/target/kode-jdbc-bridge.jar")
        });
    let h2 = std::env::var_os("KODE_TEST_H2_JAR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new("/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar")
                .into()
        });
    assert!(helper.is_file());
    assert!(h2.is_file());
    let mut ds = definition();
    ds.driver.driver_class.clear(); // Exercise actual JDBC service discovery.
    ds.jdbc_url = format!("jdbc:h2:file:{}", root.path().join("fixture").display());
    ds.driver.jar_path = Some(h2.to_str().unwrap().into());
    ds.driver.artifact = Some(kode_database::model::JdbcDriverArtifact {
        repository_type: kode_database::model::JdbcRepositoryType::LocalFile,
        ..Default::default()
    });
    DataSourceRepository::new(db.clone()).upsert(ds).unwrap();
    let (mut app, _) = make_installation(
        &workspace,
        &db,
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
    );
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state.splash = false;
    app.state.dispatch(&key(Key::Function(2), false)).unwrap();
    let alt = |ch| {
        InputEvent::Key(KeyInput {
            key: Key::Character(ch),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        })
    };
    app.state.dispatch(&alt('d')).unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .install
            .as_ref()
            .is_some_and(|dialog| dialog.done)
    })
    .await;
    assert!(app.state.database.install.as_ref().unwrap().success);
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    assert!(app.state.database.install.is_none());
    assert!(app.state.database.form.is_some());
    app.state.dispatch(&alt('t')).unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .form
            .as_ref()
            .is_some_and(|form| form.busy.is_none() && form.status.starts_with("Success:"))
    })
    .await;
    {
        let form = app.state.database.form.as_mut().unwrap();
        form.focus = 4;
    }
    app.state
        .dispatch(&InputEvent::Paste("synthetic-wrong-password".into()))
        .unwrap();
    app.state.dispatch(&alt('t')).unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .form
            .as_ref()
            .is_some_and(|form| form.busy.is_none() && form.status.contains("28000"))
    })
    .await;
    let form = app.state.database.form.as_ref().unwrap();
    assert_eq!(form.fields[3].value, "synthetic-wrong-password");
    assert!(!form.status.contains("synthetic-wrong-password"));
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&key(Key::Backspace, false)).unwrap();
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert_eq!(app.state.focus.focused, Some(app.state.database.tree.id));
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    let services = app.database_services().unwrap();
    until(&mut app, |_| {
        services.connections.session("fixture").is_ok()
    })
    .await;
    let executor = Executor::new(services.connections.clone());
    let (send, _) = tokio::sync::watch::channel(Snapshot::default());
    let result = executor
        .execute(
            "fixture",
            "create table customer(id int primary key, name varchar(20))".into(),
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
    app.pump().unwrap();
    app.state.database.tree.selected = 0;
    app.state
        .dispatch(&key(Key::Character('r'), false))
        .unwrap();
    async fn select(app: &mut AppRuntime, kind: DatabaseObjectType, name: Option<&str>) {
        until(app, |app| {
            app.state.database.tree.lines.iter().any(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.object_type == kind && name.is_none_or(|name| object.name == name)
                })
            })
        })
        .await;
        let index = app
            .state
            .database
            .tree
            .lines
            .iter()
            .position(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.object_type == kind && name.is_none_or(|name| object.name == name)
                })
            })
            .unwrap();
        app.state.database.tree.selected = index;
        app.state.dispatch(&key(Key::Enter, false)).unwrap();
    }
    select(&mut app, DatabaseObjectType::Catalog, None).await;
    select(&mut app, DatabaseObjectType::Schema, Some("PUBLIC")).await;
    select(&mut app, DatabaseObjectType::Category, Some("Tables")).await;
    select(&mut app, DatabaseObjectType::Table, Some("CUSTOMER")).await;
    app.state
        .dispatch(&key(Key::Character('s'), false))
        .unwrap();
    let document = app.state.editor.as_ref().unwrap().document_id();
    let focus = app.state.focus.focused;
    app.state
        .dispatch(&InputEvent::Paste("select * from customer;".into()))
        .unwrap();
    until(&mut app, |app| {
        app.state.database.tree.lines.iter().any(|line| {
            line.object
                .as_ref()
                .is_some_and(|o| o.object_type == DatabaseObjectType::Column && o.name == "NAME")
        })
    })
    .await;
    assert_eq!(app.state.focus.focused, focus);
    assert_eq!(
        app.state
            .documents
            .document(document)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "-- Fixture\nselect * from customer;"
    );
    assert_eq!(app.state.database.tree.sessions.len(), 1);
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let second = app.state.editor.as_ref().unwrap().document_id();
    assert_ne!(second, document);
    app.state.dispatch(&alt6()).unwrap();
    app.state.database.tree.selected = app
        .state
        .database
        .tree
        .lines
        .iter()
        .position(|line| {
            matches!(
                &line.key,
                kode_tui::panels::database::tree::NodeKey::Sessions(_)
            )
        })
        .unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    let first_console = app.state.sql.documents[&document].clone();
    let second_console = app.state.sql.documents[&second].clone();
    app.state.database.tree.selected=app.state.database.tree.lines.iter().position(|line|matches!(&line.key,kode_tui::panels::database::tree::NodeKey::Session{id,..}if id==&first_console)).unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    assert_eq!(app.state.editor.as_ref().unwrap().document_id(), document);
    app.state.dispatch(&alt6()).unwrap();
    app.state.database.tree.selected=app.state.database.tree.lines.iter().position(|line|matches!(&line.key,kode_tui::panels::database::tree::NodeKey::Session{id,..}if id==&second_console)).unwrap();
    app.state.dispatch(&key(Key::Delete, false)).unwrap();
    assert_eq!(app.state.database.tree.sessions.len(), 1);
    app.state.dispatch(&alt6()).unwrap();
    app.state.database.tree.selected = 0;
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    until(&mut app, |_| {
        services.connections.snapshot("fixture").phase
            == kode_database::connections::Phase::Disconnected
    })
    .await;
    executor.shutdown().await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert!(!db.join("credentials-v1.json").exists());
}
fn paint(app: &mut AppRuntime) {
    let area = Rect::new(0, 0, 110, 34);
    let mut buffer = Buffer::empty(area);
    app.state
        .render(area, &mut buffer, &Theme::compiled())
        .unwrap();
}
async fn sql_run(
    app: &mut AppRuntime,
    text: &str,
    all: bool,
) -> kode_database::execution::Snapshot {
    let console = app.state.active_sql().unwrap().to_string();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(text.into())).unwrap();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                shift: all,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    until(app, |app| {
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    app.state
        .sql
        .manager
        .get(&console)
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .clone()
}
async fn sql_control(app: &mut AppRuntime, ch: char) {
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character(ch),
            modifiers: Modifiers {
                ctrl: true,
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    until(app, |app| app.state.sql.controls.is_empty()).await;
    assert!(
        app.state.notice.contains("Transaction operation completed"),
        "{}",
        app.state.notice
    );
}
#[tokio::test]
#[ignore = "requires built Java helper and H2 JAR; scripts/test-jdbc-bridge.sh supplies them"]
async fn actual_java_h2_sql_console_targets_results_transactions_context_cancel_and_focus() {
    use kode_database::{
        bridge::wire::{Cell, Outcome},
        execution::Phase,
    };
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db");
    let helper =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../jdbc-bridge/target/kode-jdbc-bridge.jar");
    let h2 =
        Path::new("/tmp/kode-rs-jdbc-maven-repository/com/h2database/h2/2.2.224/h2-2.2.224.jar");
    assert!(helper.is_file() && h2.is_file());
    let mut ds = definition();
    ds.auto_commit = false;
    ds.driver.jar_path = Some(h2.to_string_lossy().into_owned());
    ds.driver.artifact = Some(kode_database::model::JdbcDriverArtifact {
        repository_type: kode_database::model::JdbcRepositoryType::LocalFile,
        ..Default::default()
    });
    DataSourceRepository::new(db.clone()).upsert(ds).unwrap();
    let (mut app, _) = make_installation(
        &workspace,
        &db,
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
    );
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    let services = app.database_services().unwrap();
    until(&mut app, |_| {
        services.connections.session("fixture").is_ok()
    })
    .await;
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let first = app.state.editor.as_ref().unwrap().document_id();
    let first_console = app.state.active_sql().unwrap().to_string();
    let editor_focus = app.state.focus.focused;
    assert_eq!(
        sql_run(&mut app, "create table UI_ITEM(ID int)", false)
            .await
            .completion
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(
        sql_run(&mut app, "insert into UI_ITEM values(1)", false)
            .await
            .preview
            .update_counts,
        [1]
    );
    sql_control(&mut app, 'r').await;
    let count = sql_run(&mut app, "select count(*) from UI_ITEM", false).await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="0")
    );
    sql_run(&mut app, "insert into UI_ITEM values(2)", false).await;
    sql_control(&mut app, 'c').await;
    sql_control(&mut app, 'r').await;
    // Current at EOF only selects the final statement; the earlier insert must
    // never run again merely because the script ends with a semicolon.
    let count = sql_run(
        &mut app,
        "insert into UI_ITEM values(3);select count(*) from UI_ITEM;",
        false,
    )
    .await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="1")
    );
    assert_eq!(
        app.state
            .sql
            .manager
            .get(&first_console)
            .unwrap()
            .history
            .back()
            .unwrap()
            .sql,
        "select count(*) from UI_ITEM"
    );
    let script = "insert into UI_ITEM values(4);insert into UI_ITEM values(5);";
    sql_run(&mut app, script, true).await;
    assert_eq!(
        app.state
            .sql
            .manager
            .get(&first_console)
            .unwrap()
            .history
            .back()
            .unwrap()
            .sql,
        script
    );
    let count = sql_run(&mut app, "select count(*) from UI_ITEM", false).await;
    assert!(
        matches!(&count.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="3")
    );
    sql_control(&mut app, 'r').await;
    let result = sql_run(
        &mut app,
        "select null as N, X'00FF' as B, X from system_range(1,600)",
        false,
    )
    .await;
    assert_eq!(result.preview.result_sets[0].received_rows, 500);
    assert!(result.preview.result_sets[0].limited);
    let panel = app.state.sql.results.get(&first_console).unwrap();
    let results_id = panel.id;
    let selection = app
        .state
        .editor
        .as_ref()
        .unwrap()
        .input
        .state
        .caret
        .selection;
    app.state.dispatch(&key(Key::Left, false)).unwrap();
    assert_ne!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .selection,
        selection
    );
    let selection = app
        .state
        .editor
        .as_ref()
        .unwrap()
        .input
        .state
        .caret
        .selection;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::PageDown,
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.sql.results[&first_console].row > 0);
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .selection,
        selection
    );
    assert_eq!(app.state.focus.focused, editor_focus);
    let results_area = app.state.geometry.sql_results;
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: results_area.x,
            y: results_area.y,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers::default(),
        }))
        .unwrap();
    assert_eq!(app.state.focus.focused, Some(results_id));
    app.state.dispatch(&key(Key::Down, false)).unwrap();
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .selection,
        selection
    );
    app.state.focus.focused = editor_focus;
    sql_control(&mut app, 'a').await;
    assert!(services.connections.session("fixture").unwrap().auto_commit);
    sql_run(&mut app, "create schema CONSOLE_A", false).await;
    app.state
        .sql
        .manager
        .set_context(&first_console, None, Some("CONSOLE_A".into()));
    let schema = sql_run(&mut app, "select current_schema", false).await;
    assert!(
        matches!(&schema.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="CONSOLE_A")
    );
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let schema = sql_run(&mut app, "select current_schema", false).await;
    assert!(
        matches!(&schema.preview.result_sets[0].chunks[0][0][0],Cell::Text{text,..}if text=="PUBLIC")
    );
    assert!(
        app.state
            .sql
            .manager
            .get(app.state.active_sql().unwrap())
            .unwrap()
            .state
            .auto_commit
    );
    app.state.open_database_session(first).unwrap();
    assert!(
        app.state
            .sql
            .manager
            .get(&first_console)
            .unwrap()
            .result
            .is_some()
    );
    let focus = app.state.focus.focused;
    let sql = "select sum(sqrt(X)) from system_range(1,1000000000)";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state
            .sql
            .manager
            .get(&first_console)
            .unwrap()
            .result
            .as_ref()
            .is_some_and(|result| result.phase == Phase::Running)
    })
    .await;
    app.state
        .dispatch(&InputEvent::Paste("\n-- typed during execution".into()))
        .unwrap();
    let buffer = app
        .state
        .documents
        .document(first)
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    until(&mut app, |app| {
        !app.state.sql.running.contains_key(&first_console)
    })
    .await;
    let console = app.state.sql.manager.get(&first_console).unwrap();
    assert_eq!(
        console
            .result
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .outcome,
        Outcome::Cancelled
    );
    assert_eq!(console.history.back().unwrap().sql, sql);
    assert_eq!(
        app.state
            .documents
            .document(first)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        buffer
    );
    assert_eq!(app.state.focus.focused, focus);
    let failed = sql_run(&mut app, "select * from MISSING_UI_TABLE", false).await;
    assert_eq!(failed.completion.unwrap().outcome, Outcome::Failed);
    assert_eq!(app.state.focus.focused, focus);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn sql_cancel_retry_is_bounded_and_captures_the_original_execution_identity() {
    use kode_database::{
        bridge::wire::CancelStatus,
        execution::{Phase, Snapshot},
    };
    use kode_tui::{app::state::AppWork, panels::database::console::Running};
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("project");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db");
    DataSourceRepository::new(db.clone())
        .upsert(definition())
        .unwrap();
    let (mut app, _) = make(&workspace, &db, None);
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let console = app.state.active_sql().unwrap().to_string();
    let tag = app.state.tag().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (_, progress) = tokio::sync::watch::channel(Snapshot {
        execution_id: Some("original-execution".into()),
        phase: Phase::Cancelling,
        cancel_status: Some(CancelStatus::Unsupported),
        ..Default::default()
    });
    app.state.sql.running.insert(
        console.clone(),
        Running {
            tag,
            cancel,
            progress,
            cancel_retry: None,
        },
    );
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    let requests: Vec<_> = app
        .state
        .take_work()
        .into_iter()
        .filter_map(|work| {
            if let AppWork::SqlCancel {
                console,
                request,
                execution,
                ..
            } = work
            {
                Some((console, request, execution))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        requests,
        [(console.clone(), tag.request, "original-execution".into())]
    );
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert!(
        !app.state
            .take_work()
            .iter()
            .any(|work| matches!(work, AppWork::SqlCancel { .. }))
    );
    app.state.sql.running.remove(&console);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn leaving_with_an_unsubmitted_sql_save_dialog_flushes_the_buffer_without_waiting_for_a_filename()
 {
    for open_dialog in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = make(root.path(), &root.path().join("db"), None);
        app.state.database.tree.replace_sources(vec![definition()]);
        app.state
            .open_database_console("fixture".into(), String::new())
            .unwrap();
        app.state
            .dispatch(&InputEvent::Paste("select 'retained on quit';".into()))
            .unwrap();
        app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
        if open_dialog {
            app.pump().unwrap();
            assert!(app.state.sql.dialog.is_some());
        }
        tokio::time::timeout(Duration::from_secs(2), app.flush_before_leave())
            .await
            .expect("unsubmitted SQL save must not stall closure")
            .unwrap();
        assert_eq!(
            app.state.persistence.session().database["sessions"][0]["buffer"],
            "-- Fixture\nselect 'retained on quit';"
        );
        app.quiesce().await.unwrap();
    }
}
#[tokio::test]
async fn sql_save_restore_hover_and_find_precedence_use_the_real_editor_and_session_workers() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db");
    DataSourceRepository::new(db.clone())
        .upsert(definition())
        .unwrap();
    let (mut app, _) = make(&workspace, &db, None);
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    let id = app.state.editor.as_ref().unwrap().document_id();
    let console = app.state.active_sql().unwrap().to_string();
    assert_eq!(console, "fixture:1");
    let sql = "select '界;😀';\nselect 42;";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    paint(&mut app);
    let selection = app
        .state
        .editor
        .as_ref()
        .unwrap()
        .input
        .state
        .caret
        .selection;
    let execute = app.state.sql.toolbar[0];
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: execute.x,
            y: execute.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }))
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.as_ref().unwrap().host.transient.is_some()
    })
    .await;
    let view = app.state.editor.as_ref().unwrap();
    assert_eq!(view.input.state.caret.selection, selection);
    let range = view.host.transient.unwrap().range;
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .slice(range)
            .unwrap()
            .to_string()
            .trim(),
        "select 42"
    );
    app.state.dispatch(&key(Key::Character('f'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("select".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    app.pump().unwrap();
    assert!(app.state.sql.running.is_empty());
    assert!(
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .history
            .is_empty()
    );
    assert!(app.state.editor.as_ref().unwrap().host.transient.is_none());
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    app.state.sql.manager.set_context(
        &console,
        Some("saved catalog".into()),
        Some("saved schema".into()),
    );
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_some()).await;
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("queries/fixture.sql".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_none()).await;
    let path = workspace.join("queries/fixture.sql");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), sql);
    // A second save retains an external change and leaves the dialog/path live.
    app.state
        .dispatch(&InputEvent::Paste("\n-- modified editor".into()))
        .unwrap();
    std::fs::write(&path, "external file content").unwrap();
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_some()).await;
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .sql
            .dialog
            .as_ref()
            .is_some_and(|dialog| !dialog.busy && !dialog.dialog.finished)
    })
    .await;
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "external file content"
    );
    assert_eq!(
        app.state.sql.dialog.as_ref().unwrap().dialog.field.value,
        path.to_string_lossy()
    );
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    let final_buffer = app
        .state
        .documents
        .document(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let (mut restored, _) = make(&workspace, &db, None);
    paint(&mut restored);
    let restored_id = restored.state.editor.as_ref().unwrap().document_id();
    assert_eq!(restored.state.active_sql(), Some(console.as_str()));
    assert_eq!(
        restored
            .state
            .documents
            .document(restored_id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        final_buffer
    );
    assert_eq!(
        restored
            .state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .state
            .schema
            .as_deref(),
        Some("saved schema")
    );
    assert!(
        restored
            .database_services()
            .unwrap()
            .connections
            .session("fixture")
            .is_err()
    );
    // Loading definitions and opening another console keeps the source counter.
    restored.state.dispatch(&alt6()).unwrap();
    until(&mut restored, |app| {
        app.state.database.tree.sources.len() == 1
    })
    .await;
    restored
        .state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    assert_eq!(restored.state.active_sql(), Some("fixture:2"));
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}

#[tokio::test]
async fn database_configuration_failure_stays_in_its_panel_and_native_file_edit_save_still_work() {
    let root = tempfile::tempdir().unwrap();
    let note = root.path().join("Note.txt");
    std::fs::write(&note, "native document").unwrap();
    let prepared = prepare_workspace(
        root.path(),
        WorkspaceGeneration(1),
        Some(note.clone()),
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    app.database_configuration_failed(kode_core::DomainError::Unsupported(
        "Synthetic database configuration error".into(),
    ));
    until(&mut app, |app| app.state.editor.is_some()).await;
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .tree
            .notice
            .contains("Synthetic database configuration error")
    })
    .await;
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("still editable: ".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(&mut app, |_| {
        std::fs::read_to_string(&note)
            .unwrap()
            .starts_with("still editable: ")
    })
    .await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
async fn until(app: &mut AppRuntime, condition: impl Fn(&AppRuntime) -> bool) {
    let start = Instant::now();
    loop {
        app.pump().unwrap();
        paint(app);
        if condition(app) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "timeout: {} / {}",
            app.state.notice,
            app.state.database.tree.notice
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
#[tokio::test]
async fn startup_is_inert_then_modal_clipboard_save_and_immediate_quit_use_actual_jobs() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("db-root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let (mut app, clipboard) = make(&workspace, &db, None);
    until(&mut app, |app| {
        !app.state.scan.files.is_empty() || !app.state.database.initialized
    })
    .await;
    assert!(!db.exists());
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| {
        app.state.database.initialized && app.state.database.tree.notice.contains("loaded")
    })
    .await;
    assert!(!db.exists());
    assert_eq!(app.state.focus.focused, Some(app.state.database.tree.id));
    app.state
        .dispatch(&key(Key::Character('n'), false))
        .unwrap();
    assert!(app.state.database.form.is_some());
    let registry = DriverRegistry::compiled().unwrap();
    let h2 = registry
        .definitions()
        .iter()
        .position(|d| d.id == "h2")
        .unwrap();
    let form = app.state.database.form.as_mut().unwrap();
    form.select_driver(h2, false);
    form.fields[1].value = "jdbc:h2:mem:app_saved".into();
    form.fields[1].cursor = form.fields[1].value.len();
    form.focus = 1;
    clipboard.remember("Saved from clipboard").unwrap();
    app.state.dispatch(&key(Key::Character('v'), true)).unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .form
            .as_ref()
            .is_some_and(|form| form.fields[0].value == "Saved from clipboard")
    })
    .await;
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('q'), true)).unwrap();
    app.flush_before_leave().await.unwrap();
    assert_eq!(
        DataSourceRepository::new(db.clone()).load().unwrap()[0].name,
        "Saved from clipboard"
    );
    assert!(!db.join("credentials-v1.json").exists());
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn missing_java_error_preserves_form_and_console_launch_uses_the_shared_editor_without_java()
{
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db-root");
    DataSourceRepository::new(db.clone())
        .upsert(definition())
        .unwrap();
    let (mut app, _) = make(&workspace, &db, None);
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state.splash = false;
    app.state.dispatch(&key(Key::Function(2), false)).unwrap();
    app.state.database.form.as_mut().unwrap().focus = 3;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('t'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    until(&mut app, |app| {
        app.state
            .database
            .form
            .as_ref()
            .is_some_and(|form| form.busy.is_none() && form.status.contains("Java"))
    })
    .await;
    let form = app.state.database.form.as_ref().unwrap();
    assert_eq!(form.focus, 3);
    assert_eq!(form.fields[0].value, "Fixture");
    assert_eq!(form.fields[1].value, "jdbc:h2:mem:app_fixture");
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert_eq!(app.state.focus.focused, Some(app.state.database.tree.id));
    app.state
        .dispatch(&key(Key::Character('s'), false))
        .unwrap();
    let view = app.state.editor.as_ref().unwrap();
    assert_eq!(
        app.state
            .sql_sources
            .get(&view.document_id())
            .map(String::as_str),
        Some("fixture")
    );
    assert!(
        matches!(app.state.documents.document(view.document_id()).unwrap().origin(),DocumentOrigin::Virtual{owner,..}if owner=="database")
    );
    app.state
        .dispatch(&InputEvent::Paste("select 1;".into()))
        .unwrap();
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .to_string(),
        "-- Fixture\nselect 1;"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn workspace_switch_rebinds_database_ownership_and_keeps_global_definitions() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let next = root.path().join("next");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&next).unwrap();
    let db = root.path().join("db-root");
    DataSourceRepository::new(db.clone())
        .upsert(definition())
        .unwrap();
    let (mut app, _) = make(&first, &db, None);
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    let old = app.database_services().unwrap();
    app.switch_workspace(next.clone()).await.unwrap();
    assert_eq!(app.state.project.root, next);
    assert_eq!(app.state.database.tree.generation, WorkspaceGeneration(2));
    assert!(app.state.database.tree.sources.is_empty());
    let current = app.database_services().unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    app.state.dispatch(&alt6()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    assert_eq!(app.state.database.tree.sources[0].name, "Fixture");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
