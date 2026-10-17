use kode_core::{DocumentOrigin, WorkspaceGeneration};
use kode_database::{
    bridge::runtime::Installation,
    catalog::{DriverRegistry, UrlCatalog},
    config::{DataSourceRepository, DatabasePaths},
    model::DataSourceDefinition,
    model::{MetadataSourceDocument, MetadataSourceKind},
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_tui::{
    app::{
        panel::DATABASE,
        runtime::{AppRuntime, database::DatabaseConfiguration},
        state::AppWork,
        workspace::prepare_workspace,
    },
    editor::host::{ExecutionAction, HostRole, SaveTarget},
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
fn alt4() -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character('4'),
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
        model::{DatabaseObjectType, MetadataRequest, MetadataRequestKind},
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
    app.state.dispatch(&alt4()).unwrap();
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
    until(&mut app, |app| {
        !app.state
            .database
            .tree
            .lines
            .iter()
            .any(|line| line.label == "Loading metadata…")
    })
    .await;
    let session = services.connections.session("fixture").unwrap();
    let unsupported_packages = services
        .metadata
        .introspect(
            "fixture",
            MetadataRequest {
                kind: MetadataRequestKind::Schema,
                catalog: session.catalog.clone(),
                schema: session.schema.clone(),
                category: None,
                object_name: None,
                object_type: None,
                object_id: None,
            },
            true,
            &CancellationToken::new(),
        )
        .await
        .expect("read H2 package capabilities through the Rust metadata client");
    let capabilities = unsupported_packages
        .capabilities
        .as_ref()
        .expect("H2 metadata must return provider capabilities");
    assert!(!capabilities.packages);
    assert!(!capabilities.package_members);
    assert!(!capabilities.package_source);
    assert!(!capabilities.package_ddl);
    assert!(unsupported_packages.objects.iter().all(|object| {
        object.object_type != DatabaseObjectType::Package && object.name != "Packages"
    }));
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
    until(&mut app, |app| {
        app.state.database.tree.lines.iter().any(|line| {
            line.object
                .as_ref()
                .is_some_and(|object| object.object_type == DatabaseObjectType::Category)
        })
    })
    .await;
    assert!(app.state.database.tree.lines.iter().all(|line| {
        line.object.as_ref().is_none_or(|object| {
            object.object_type != DatabaseObjectType::Package && object.name != "Packages"
        })
    }));
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
    app.state.dispatch(&alt4()).unwrap();
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
    app.state.dispatch(&alt4()).unwrap();
    app.state.database.tree.selected=app.state.database.tree.lines.iter().position(|line|matches!(&line.key,kode_tui::panels::database::tree::NodeKey::Session{id,..}if id==&second_console)).unwrap();
    app.state.dispatch(&key(Key::Delete, false)).unwrap();
    assert_eq!(app.state.database.tree.sessions.len(), 1);
    app.state.dispatch(&alt4()).unwrap();
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
async fn oracle_adapter_strips_selected_sql_terminator_only_from_driver_payload() {
    use kode_core::{ByteOffset, Selection};
    use kode_database::bridge::wire::{Cell, Outcome};

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
    ds.driver.id = "oracle".into();
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
    app.state.dispatch(&alt4()).unwrap();
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
    let sql = "SELECT 1;";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
    app.state
        .editor
        .as_mut()
        .unwrap()
        .input
        .state
        .caret
        .selection = Selection {
        anchor: ByteOffset(0),
        head: ByteOffset(sql.len()),
    };
    let console = app.state.active_sql().unwrap().to_string();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    let query = app
        .state
        .sql
        .manager
        .get(&console)
        .unwrap()
        .history
        .back()
        .unwrap();
    assert_eq!(query.sql, "SELECT 1");
    assert_eq!(
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert_eq!(
        app.state
            .documents
            .document(app.state.editor.as_ref().unwrap().document_id())
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        sql
    );
    let script = "CREATE TABLE oracle_script (payload INT);\nINSERT INTO oracle_script VALUES (1);\nSELECT payload FROM oracle_script;";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(script.into()))
        .unwrap();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
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
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    let manager = app.state.sql.manager.get(&console).unwrap();
    assert_eq!(manager.history.back().unwrap().sql, script);
    assert_eq!(
        manager
            .result
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap()
            .outcome,
        Outcome::Completed
    );
    assert!(matches!(
        &manager.result.as_ref().unwrap().preview.result_sets[0].chunks[0][0][0],
        Cell::Text { text, .. } if text == "1"
    ));
    assert_eq!(
        app.state
            .documents
            .document(app.state.editor.as_ref().unwrap().document_id())
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        script,
        "script execution must leave the editor text untouched"
    );
    let multi_result_script = "SELECT 11 AS first_value; SELECT 12 AS second_value;";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(multi_result_script.into()))
        .unwrap();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
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
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    let multi_result = app.state.sql.manager.get(&console).unwrap();
    assert_eq!(
        multi_result.history.back().unwrap().sql,
        multi_result_script
    );
    let preview = &multi_result.result.as_ref().unwrap().preview;
    assert_eq!(preview.result_sets.len(), 2);
    assert!(matches!(
        &preview.result_sets[0].chunks[0][0][0],
        Cell::Text { text, .. } if text == "11"
    ));
    assert!(matches!(
        &preview.result_sets[1].chunks[0][0][0],
        Cell::Text { text, .. } if text == "12"
    ));
    let failing_script = "INSERT INTO oracle_script VALUES (3); INSERT INTO missing_script_table VALUES (9); INSERT INTO oracle_script VALUES (4);";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(failing_script.into()))
        .unwrap();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
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
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    let failure = app.state.sql.manager.get(&console).unwrap();
    assert_eq!(failure.history.back().unwrap().sql, failing_script);
    let failure_result = failure.result.as_ref().unwrap();
    assert!(
        failure_result.error.is_some()
            || failure_result
                .completion
                .as_ref()
                .and_then(|completion| completion.error.as_ref())
                .is_some(),
        "failed script must retain its SQL error"
    );
    let count_sql = "SELECT COUNT(*) FROM oracle_script;";
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(count_sql.into()))
        .unwrap();
    let previous = app.state.sql.manager.get(&console).unwrap().history.len();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.sql.manager.get(&console).unwrap().history.len() == previous + 1
    })
    .await;
    let count = app.state.sql.manager.get(&console).unwrap();
    assert!(count.history.back().unwrap().success);
    assert!(
        matches!(
            &count.result.as_ref().unwrap().preview.result_sets[0].chunks[0][0][0],
            Cell::Text { text, .. } if text == "2"
        ),
        "statements after the failing statement must not execute"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
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
    let (mut app, clipboard) = make_installation(
        &workspace,
        &db,
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
    );
    app.state.dispatch(&alt4()).unwrap();
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
    let selection = kode_core::Selection {
        anchor: kode_core::ByteOffset(0),
        head: kode_core::ByteOffset(buffer.len()),
    };
    app.state
        .editor
        .as_mut()
        .unwrap()
        .input
        .state
        .caret
        .selection = selection;
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
    let failed = sql_run(&mut app, "select * from MISSING_UI_TABLE", false).await;
    assert_eq!(failed.completion.unwrap().outcome, Outcome::Failed);
    assert_eq!(app.state.focus.focused, focus);
    app.state.dispatch(&key(Key::Function(4), false)).unwrap();
    assert!(
        app.state
            .diagnostic
            .as_ref()
            .unwrap()
            .text
            .contains("MISSING_UI_TABLE")
    );
    assert!(
        app.state
            .diagnostic
            .as_ref()
            .unwrap()
            .text
            .contains("SQLSTATE")
    );
    app.state.dispatch(&key(Key::Character('c'), true)).unwrap();
    assert!(clipboard.local_text().contains("MISSING_UI_TABLE"));
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert_eq!(app.state.focus.focused, focus);
    // F4 follows the focused database explorer even with a SQL console open.
    app.state.focus.focused = Some(app.state.database.tree.id);
    app.state.dispatch(&key(Key::Function(4), false)).unwrap();
    assert!(
        !app.state
            .diagnostic
            .as_ref()
            .unwrap()
            .title
            .starts_with("SQL")
    );
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert_eq!(app.state.focus.focused, Some(app.state.database.tree.id));
    app.state.focus.focused = focus;
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
    app.state.dispatch(&alt4()).unwrap();
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
    app.state.dispatch(&alt4()).unwrap();
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
    restored.state.dispatch(&alt4()).unwrap();
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
    app.state.dispatch(&alt4()).unwrap();
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
    app.state.dispatch(&alt4()).unwrap();
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
    app.state.dispatch(&alt4()).unwrap();
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
    assert!(app.state.database.java_required);
    app.state.dispatch(&key(Key::Function(5), false)).unwrap();
    let dialog = app.state.database.java_runtime.as_ref().unwrap();
    assert!(
        dialog.tag.is_none(),
        "opening the offer must not start installation"
    );
    assert!(!dialog.hidden);
    paint(&mut app);
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert!(app.state.database.java_runtime.as_ref().unwrap().hidden);
    let form = app.state.database.form.as_ref().unwrap();
    assert_eq!(form.focus, 3);
    assert_eq!(form.fields[0].value, "Fixture");
    assert_eq!(form.fields[1].value, "jdbc:h2:mem:app_fixture");
    assert!(
        !db.join("java-runtime").exists(),
        "declining must not create installation files"
    );
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
    app.state.dispatch(&alt4()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    let old = app.database_services().unwrap();
    app.switch_workspace(next.clone()).await.unwrap();
    assert_eq!(app.state.project.root, next);
    assert_eq!(app.state.database.tree.generation, WorkspaceGeneration(2));
    assert!(app.state.database.tree.sources.is_empty());
    let current = app.database_services().unwrap();
    assert!(!Arc::ptr_eq(&old, &current));
    app.state.dispatch(&alt4()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    assert_eq!(app.state.database.tree.sources[0].name, "Fixture");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn database_error_details_preserve_request_identity_full_copy_form_values_and_focus() {
    use kode_tui::panels::database::controller::Context;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let (mut app, clipboard) = make(&workspace, &root.path().join("db"), None);
    let original_focus = app.state.focus.focused;
    app.state
        .database
        .edit(Some(definition()), &mut app.state.focus)
        .unwrap();
    let before = app.state.focus.focused;
    let tag = app.state.tag().unwrap();
    app.state.database.form.as_mut().unwrap().busy = Some(tag);
    let text = format!(
        "Connection failed\n{}\nCAUSE TAIL / SQLSTATE 28000 / vendor 73",
        "long diagnostic 界 ".repeat(400)
    );
    app.state.database.complete(
        tag,
        Context::Test,
        Err(text.clone()),
        true,
        &mut app.state.focus,
    );
    app.state.dispatch(&key(Key::Function(4), false)).unwrap();
    assert!(
        app.state
            .diagnostic
            .as_ref()
            .unwrap()
            .title
            .contains(&tag.request.0.to_string())
    );
    assert!(app.state.diagnostic.as_ref().unwrap().text.ends_with(&text));
    for width in [60, 100] {
        let area = Rect::new(0, 0, width, 24);
        let mut buffer = Buffer::empty(area);
        app.state
            .render(area, &mut buffer, &Theme::compiled())
            .unwrap();
        app.state.dispatch(&key(Key::End, false)).unwrap();
        app.state
            .render(area, &mut buffer, &Theme::compiled())
            .unwrap();
        let screen: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(screen.contains("CAUSE TAIL"));
    }
    app.state.dispatch(&key(Key::Character('c'), true)).unwrap();
    assert!(clipboard.local_text().ends_with(&text));
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert_eq!(app.state.focus.focused, before);
    assert_eq!(
        app.state.database.form.as_ref().unwrap().fields[0].value,
        "Fixture"
    );
    assert_eq!(
        app.state.database.form.as_ref().unwrap().fields[1].value,
        "jdbc:h2:mem:app_fixture"
    );
    // A parent Save can finish and dismiss its form while details remain visible.
    let save_tag = app.state.tag().unwrap();
    app.state.database.form.as_mut().unwrap().busy = Some(save_tag);
    app.state.dispatch(&key(Key::Function(4), false)).unwrap();
    app.state.database.complete(
        save_tag,
        Context::Save,
        Ok(kode_tui::panels::database::controller::Report::Sources {
            sources: vec![definition()],
            warning: None,
        }),
        true,
        &mut app.state.focus,
    );
    assert!(app.state.database.form.is_none());
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert!(app.state.diagnostic.is_none());
    assert_eq!(app.state.focus.focused, original_focus);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn package_source_opens_as_replaced_ephemeral_readonly_sql_history() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db-root");
    let (mut app, clipboard) = make(&workspace, &db, None);
    app.state.tabs.selected = DATABASE;
    app.state.focus.focused = Some(app.state.database.tree.id);
    let document = |text: &str| MetadataSourceDocument {
        title: "PACKAGE — statement 1".into(),
        kind: MetadataSourceKind::SqlSource,
        text: text.into(),
    };
    app.state
        .open_database_source("APP.PACKAGE v1".into(), vec![document("SELECT 'first'")])
        .unwrap();
    let first = app.state.editor.as_ref().unwrap();
    let first_id = first.document_id();
    assert_eq!(first.host.descriptor.role, HostRole::Historical);
    assert!(first.host.descriptor.read_only);
    assert_eq!(first.host.descriptor.save_target, SaveTarget::None);
    assert_eq!(first.host.descriptor.execute, None::<ExecutionAction>);
    assert!(matches!(
        app.state.documents.document(first_id).unwrap().origin(),
        DocumentOrigin::Virtual { owner, .. } if owner == "database-package-source"
    ));
    assert_eq!(
        app.state
            .documents
            .document(first_id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "SELECT 'first'"
    );
    assert!(
        app.state
            .dispatch(&InputEvent::Paste("must not alter package source".into()))
            .is_err()
    );
    assert!(app.state.dispatch(&key(Key::Character('s'), true)).is_err());
    assert_eq!(
        app.state
            .documents
            .document(first_id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "SELECT 'first'"
    );
    assert!(!root.path().join("APP.PACKAGE v1").exists());
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('c'), true)).unwrap();
    assert_eq!(clipboard.local_text(), "SELECT 'first'");
    app.state.dispatch(&key(Key::Character('f'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("first".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state
            .editor
            .as_ref()
            .and_then(|view| view.search.results.report.as_ref())
            .is_some_and(|report| report.matches.len() == 1)
    })
    .await;
    assert_eq!(
        app.state
            .documents
            .document(first_id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "SELECT 'first'"
    );

    app.state.tabs.selected = DATABASE;
    app.state.focus.focused = Some(app.state.database.tree.id);
    app.state
        .open_database_source("APP.PACKAGE v2".into(), vec![document("VALUES 'second'")])
        .unwrap();
    let second = app.state.editor.as_ref().unwrap();
    let second_id = second.document_id();
    assert_ne!(first_id, second_id);
    assert_eq!(
        app.state
            .documents
            .document(second_id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "VALUES 'second'"
    );
    let package_sources = app
        .state
        .documents
        .ids()
        .filter(|id| {
            matches!(
                app.state.documents.document(*id).unwrap().origin(),
                DocumentOrigin::Virtual { owner, .. } if owner == "database-package-source"
            )
        })
        .count();
    assert_eq!(package_sources, 1);
    app.state.focus.focused = app.state.editor.as_ref().map(|view| view.input.id);
    app.state
        .open_database_source("APP.PACKAGE stale".into(), vec![document("SELECT 'stale'")])
        .unwrap();
    assert_eq!(app.state.editor.as_ref().unwrap().document_id(), second_id);
    assert!(app.state.notice.contains("focus changed"));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn sql_search_ctrl_enter_finds_all_without_scheduling_query_execution() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("db-root");
    let (mut app, _) = make(&workspace, &db, None);
    app.state.database.tree.replace_sources(vec![definition()]);
    app.state
        .open_database_console(
            "fixture".into(),
            "SELECT 'needle';\nSELECT 'needle';".into(),
        )
        .unwrap();
    let console = app.state.active_sql().unwrap().to_string();
    assert!(
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .history
            .is_empty()
    );

    app.state.dispatch(&key(Key::Character('f'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("needle".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    assert!(app.state.editor.as_ref().unwrap().search.visible);
    assert!(!app.state.work.iter().any(|work| matches!(
        work,
        AppWork::Editor {
            work: kode_tui::editor::host::EditorWork::Execute(_),
            ..
        }
    )));
    until(&mut app, |app| {
        app.state
            .editor
            .as_ref()
            .and_then(|view| view.search.results.report.as_ref())
            .is_some_and(|report| report.matches.len() == 2)
    })
    .await;
    assert!(
        app.state
            .sql
            .manager
            .get(&console)
            .unwrap()
            .history
            .is_empty()
    );
    assert!(!app.state.sql.running.contains_key(&console));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned Oracle/MariaDB fixture credentials and a built Java helper"]
async fn actual_vendor_package_source_menu_opens_readonly_editor_and_refreshes_after_cleanup() {
    use kode_database::{
        credentials::Secret,
        drivers::Progress,
        execution::{Options, Snapshot},
        model::DatabaseObjectType,
    };
    use kode_tui::panels::database::object_menu::ObjectMenu;

    async fn execute(
        executor: &kode_database::execution::Executor,
        source: &str,
        sql: String,
    ) -> Result<(), String> {
        let (progress, _) = tokio::sync::watch::channel(Snapshot::default());
        let result = executor
            .execute(
                source,
                sql,
                Options::default(),
                progress,
                &CancellationToken::new(),
            )
            .await
            .map_err(|error| error.to_string())?;
        if result.completion.as_ref().is_none_or(|final_result| {
            final_result.outcome != kode_database::bridge::wire::Outcome::Completed
        }) {
            return Err("TUI package fixture SQL did not complete".into());
        }
        Ok(())
    }

    async fn selected_sql(app: &mut AppRuntime, sql: &str) {
        let console = app.state.active_sql().unwrap().to_string();
        let previous = app.state.sql.manager.get(&console).unwrap().history.len();
        app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
        app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
        app.state
            .editor
            .as_mut()
            .unwrap()
            .input
            .state
            .caret
            .selection = kode_core::Selection {
            anchor: kode_core::ByteOffset(0),
            head: kode_core::ByteOffset(sql.len()),
        };
        app.state.dispatch(&key(Key::Enter, true)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while app.state.sql.manager.get(&console).unwrap().history.len() == previous {
            app.pump().unwrap();
            paint(app);
            assert!(
                Instant::now() < deadline,
                "Oracle selected SQL was not submitted"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let manager = app.state.sql.manager.get(&console).unwrap();
        let entry = manager.history.back().unwrap();
        assert_eq!(
            entry.sql,
            kode_database::statement::for_driver(sql, kode_database::statement::Dialect::Oracle)
        );
        assert!(entry.success, "{}", app.state.notice);
        assert_eq!(
            app.state
                .documents
                .document(app.state.editor.as_ref().unwrap().document_id())
                .unwrap()
                .snapshot()
                .text()
                .as_ref(),
            sql,
            "driver adaptation must leave the user's editor text untouched"
        );
    }

    async fn execute_script(app: &mut AppRuntime, sql: &str) {
        let console = app.state.active_sql().unwrap().to_string();
        let previous = app.state.sql.manager.get(&console).unwrap().history.len();
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
        let deadline = Instant::now() + Duration::from_secs(8);
        while app.state.sql.manager.get(&console).unwrap().history.len() == previous {
            app.pump().unwrap();
            paint(app);
            assert!(Instant::now() < deadline, "Oracle script did not finish");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let manager = app.state.sql.manager.get(&console).unwrap();
        let entry = manager.history.back().unwrap();
        assert_eq!(entry.sql, sql);
        assert!(entry.success, "{}", app.state.notice);
        assert_eq!(
            manager
                .result
                .as_ref()
                .unwrap()
                .completion
                .as_ref()
                .unwrap()
                .outcome,
            kode_database::bridge::wire::Outcome::Completed
        );
        assert_eq!(
            app.state
                .documents
                .document(app.state.editor.as_ref().unwrap().document_id())
                .unwrap()
                .snapshot()
                .text()
                .as_ref(),
            sql,
            "script execution must leave the editor text untouched"
        );
    }

    async fn execute_failing_script(app: &mut AppRuntime, sql: &str) {
        let console = app.state.active_sql().unwrap().to_string();
        let previous = app.state.sql.manager.get(&console).unwrap().history.len();
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
        let deadline = Instant::now() + Duration::from_secs(8);
        while app.state.sql.manager.get(&console).unwrap().history.len() == previous {
            app.pump().unwrap();
            paint(app);
            assert!(
                Instant::now() < deadline,
                "Oracle failing script did not finish"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let manager = app.state.sql.manager.get(&console).unwrap();
        assert_eq!(manager.history.back().unwrap().sql, sql);
        let result = manager.result.as_ref().unwrap();
        assert!(
            result
                .error
                .as_ref()
                .is_some_and(|error| !error.message.is_empty())
                || result
                    .completion
                    .as_ref()
                    .and_then(|completion| completion.error.as_ref())
                    .is_some_and(|error| !error.message.is_empty()),
            "Oracle script failure must retain a readable SQL error"
        );
    }

    async fn cancel_query(app: &mut AppRuntime, sql: &str) {
        let console = app.state.active_sql().unwrap().to_string();
        let previous = app.state.sql.manager.get(&console).unwrap().history.len();
        app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
        app.state.dispatch(&InputEvent::Paste(sql.into())).unwrap();
        app.state.dispatch(&key(Key::Enter, true)).unwrap();
        until(app, |app| {
            app.state
                .sql
                .manager
                .get(&console)
                .and_then(|manager| manager.result.as_ref())
                .is_some_and(|result| result.phase == kode_database::execution::Phase::Running)
        })
        .await;
        app.state.dispatch(&key(Key::Escape, false)).unwrap();
        until(app, |app| !app.state.sql.running.contains_key(&console)).await;
        let manager = app.state.sql.manager.get(&console).unwrap();
        assert_eq!(manager.history.len(), previous + 1);
        assert_eq!(manager.history.back().unwrap().sql, sql);
        let completion = manager
            .result
            .as_ref()
            .unwrap()
            .completion
            .as_ref()
            .unwrap();
        assert_eq!(
            completion.outcome,
            kode_database::bridge::wire::Outcome::Cancelled,
        );
        assert_eq!(completion.error.as_ref().unwrap().vendor_code, 1013);
    }

    let vendor = std::env::var("KODE_TEST_PACKAGE_VENDOR").expect("vendor");
    let version = std::env::var("KODE_TEST_PACKAGE_DRIVER_VERSION").expect("driver version");
    let jdbc_url = std::env::var("KODE_TEST_PACKAGE_URL").expect("JDBC URL");
    let username = std::env::var("KODE_TEST_PACKAGE_USER").expect("test username");
    let password = std::env::var("KODE_TEST_PACKAGE_PASSWORD").expect("test password");
    let helper = std::env::var_os("KODE_TEST_JDBC_HELPER")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../jdbc-bridge/target/kode-jdbc-bridge.jar")
        });
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = root.path().join("database");
    let plugins = std::env::var_os("KODE_RS_JDBC_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| db.join("plugins"));
    let (display_name, driver_class, coordinates) = match vendor.as_str() {
        "oracle" => (
            "Oracle",
            "oracle.jdbc.OracleDriver",
            format!("com.oracle.database.jdbc:ojdbc11:{version}"),
        ),
        "mariadb" => (
            "MariaDB",
            "org.mariadb.jdbc.Driver",
            format!("org.mariadb.jdbc:mariadb-java-client:{version}"),
        ),
        _ => panic!("unsupported native package vendor"),
    };
    let source = format!("u12-tui-{vendor}-{}", std::process::id());
    let definition: DataSourceDefinition = serde_json::from_value(json!({
        "id": source,
        "name": display_name,
        "driver": {
            "id": vendor,
            "displayName": display_name,
            "driverClass": driver_class,
            "coordinates": coordinates,
            "providerId": vendor
        },
        "jdbcUrl": jdbc_url,
        "username": username,
        "autoCommit": true
    }))
    .unwrap();
    DataSourceRepository::new(db.clone())
        .upsert(definition.clone())
        .unwrap();
    std::fs::create_dir_all(&plugins).unwrap();
    let (mut app, _) = make_installation(
        &workspace,
        &db,
        Installation {
            java: "java".into(),
            helper_candidates: vec![helper],
        },
    );
    app.state.dispatch(&alt4()).unwrap();
    until(&mut app, |app| app.state.database.tree.sources.len() == 1).await;
    app.state.splash = false;
    let services = app
        .database_services()
        .expect("configured database services");
    let (driver_progress, _) = tokio::sync::watch::channel(Progress::default());
    services
        .installer
        .resolve(
            &definition.driver,
            driver_progress,
            &CancellationToken::new(),
        )
        .await
        .expect("install the selected live qualification driver");
    let token = CancellationToken::new();
    let mut created = false;
    let mut created_table = false;
    let mut opened = false;
    let mut external_opened = false;
    let external_source = format!("{source}-ddl");
    let fixture_table = format!("KODE_RS_U13_{}", std::process::id());
    let fixture = format!("KODE_RS_TUI_{}_界_%", std::process::id());
    let result: Result<(), String> = async {
        services
            .connections
            .open(
                definition.clone(),
                Some(Secret::new(password.clone())),
                &token,
            )
            .await
            .map_err(|error| error.to_string())?;
        opened = true;
        let mut external_definition = definition.clone();
        external_definition.id = external_source.clone();
        external_definition.name = format!("{display_name} external DDL session");
        services
            .connections
            .open(
                external_definition,
                Some(Secret::new(password.clone())),
                &token,
            )
            .await
            .map_err(|error| error.to_string())?;
        external_opened = true;
        let session = services
            .connections
            .session(&source)
            .map_err(|error| error.to_string())?;
        let schema = session
            .schema
            .clone()
            .or_else(|| session.catalog.clone())
            .ok_or_else(|| "Connected session has no selected schema".to_owned())?;
        if vendor == "mariadb" {
            execute(
                &services.executor,
                &source,
                "SET SESSION sql_mode='ORACLE'".into(),
            )
            .await?;
            execute(
                &services.executor,
                &external_source,
                "SET SESSION sql_mode='ORACLE'".into(),
            )
            .await?;
        }
        let slow_source_padding = (0..1400)
            .map(|_| format!("/* {} */", "x".repeat(1020)))
            .collect::<Vec<_>>()
            .join("\n");
        let (specification, body) = if vendor == "oracle" {
            (
                format!("CREATE PACKAGE \"{fixture}\" AS FUNCTION calc(n NUMBER) RETURN NUMBER; FUNCTION calc(n VARCHAR2) RETURN VARCHAR2; PROCEDURE increment_value(n IN OUT NUMBER); END;"),
                format!("CREATE PACKAGE BODY \"{fixture}\" AS\n{slow_source_padding}\nFUNCTION calc(n NUMBER) RETURN NUMBER IS BEGIN RETURN n + 1; END; FUNCTION calc(n VARCHAR2) RETURN VARCHAR2 IS BEGIN RETURN 'native-tui-source-界:' || n; END; PROCEDURE increment_value(n IN OUT NUMBER) IS BEGIN n := n + 1; END; END;"),
            )
        } else {
            (
                format!("CREATE PACKAGE \"{fixture}\" AS FUNCTION calc_int(n INT) RETURN INT; FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32); PROCEDURE increment_value(n INOUT INT); END;"),
                format!("CREATE PACKAGE BODY \"{fixture}\" AS\n{slow_source_padding}\nFUNCTION calc_int(n INT) RETURN INT AS BEGIN RETURN n + 1; END; FUNCTION calc_text(n VARCHAR(32)) RETURN VARCHAR(32) AS BEGIN RETURN 'native-tui-source-界:' || n; END; PROCEDURE increment_value(n INOUT INT) AS BEGIN SET n = n + 1; END; END;"),
            )
        };
        execute(&services.executor, &external_source, specification).await?;
        created = true;
        execute(&services.executor, &external_source, body).await?;
        if vendor == "oracle" {
            execute(
                &services.executor,
                &external_source,
                format!("CREATE TABLE {fixture_table} (id NUMBER)"),
            )
            .await?;
            created_table = true;
        }

        app.state.database.tree.selected = 0;
        app.state.dispatch(&key(Key::Character('r'), false)).unwrap();
        until(&mut app, |app| {
            app.state
                .database
                .tree
                .lines
                .iter()
                .any(|line| line.label == "Loading metadata…")
        })
        .await;
        app.state.dispatch(&key(Key::Character('r'), false)).unwrap();
        until(&mut app, |app| {
            !app.state
                .database
                .tree
                .lines
                .iter()
                .any(|line| line.label == "Loading metadata…")
                && app.state.database.tree.notice.starts_with("Loaded")
        })
        .await;
        if created_table {
            let schema_index = app
                .state
                .database
                .tree
                .lines
                .iter()
                .position(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Schema
                            && object.name == schema
                    })
                })
                .ok_or_else(|| "Refreshed Oracle tree did not expose the connected schema".to_owned())?;
            app.state.database.tree.selected = schema_index;
            app.state
                .dispatch(&key(Key::Enter, false))
                .map_err(|error| error.to_string())?;
            until(&mut app, |app| {
                app.state.database.tree.lines.iter().any(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Category
                            && object.name == "Tables"
                    })
                }) && !app
                    .state
                    .database
                    .tree
                    .lines
                    .iter()
                    .any(|line| line.label == "Loading metadata…")
            })
            .await;
            let table_category = app
                .state
                .database
                .tree
                .lines
                .iter()
                .position(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Category
                            && object.name == "Tables"
                    })
                })
                .ok_or_else(|| "Refreshed Oracle schema did not expose its Tables category".to_owned())?;
            app.state.database.tree.selected = table_category;
            app.state
                .dispatch(&key(Key::Enter, false))
                .map_err(|error| error.to_string())?;
            until(&mut app, |app| {
                app.state.database.tree.lines.iter().any(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Table
                            && object.name == fixture_table
                    })
                }) && !app
                    .state
                    .database
                    .tree
                    .lines
                    .iter()
                    .any(|line| line.label == "Loading metadata…")
            })
            .await;
        }
        let mut expanded = std::collections::BTreeSet::new();
        if created_table
            && let Some(object) = app
                .state
                .database
                .tree
                .lines
                .iter()
                .filter_map(|line| line.object.as_ref())
                .find(|object| {
                    object.object_type == DatabaseObjectType::Schema && object.name == schema
                })
        {
            expanded.insert(object.id.clone());
        }
        for _ in 0..8 {
            if app.state.database.tree.lines.iter().any(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.object_type == DatabaseObjectType::Package && object.name == fixture
                })
            }) {
                break;
            }
            let next = app
                .state
                .database
                .tree
                .lines
                .iter()
                .enumerate()
                .find_map(|(index, line)| {
                    let object = line.object.as_ref()?;
                    (line.expandable
                        && match object.object_type {
                            DatabaseObjectType::Catalog => object.name == schema,
                            DatabaseObjectType::Schema => object.name == schema,
                            DatabaseObjectType::Category => object.name == "Packages",
                            _ => true,
                        }
                        && expanded.insert(object.id.clone()))
                    .then_some(index)
                })
                .ok_or_else(|| "No expandable catalog path to the package category".to_owned())?;
            app.state.database.tree.selected = next;
            let previous_count = app.state.database.tree.lines.len();
            app.state.dispatch(&key(Key::Enter, false)).map_err(|e| e.to_string())?;
            until(&mut app, |app| {
                let lines = &app.state.database.tree.lines;
                (lines.iter().any(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Package && object.name == fixture
                    })
                }) || app.state.database.tree.notice.contains("No ")
                    || lines.len() != previous_count)
                    && !lines.iter().any(|line| line.label == "Loading metadata…")
            })
            .await;
        }
        let package_index = app
            .state
            .database
            .tree
            .lines
            .iter()
            .position(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.object_type == DatabaseObjectType::Package && object.name == fixture
                })
            })
            .ok_or_else(|| "Created package did not appear in the live TUI tree".to_owned())?;
        let package = app.state.database.tree.lines[package_index]
            .object
            .as_ref()
            .unwrap()
            .clone();
        if package.schema.as_deref() != Some(&schema)
            || package.attributes.get("sourceAvailable").map(String::as_str) != Some("true")
        {
            return Err("TUI package lost its schema or source capability identity".into());
        }

        let source_request = kode_tui::panels::database::tree::DatabaseTree::request_for(&package)
            .ok_or_else(|| "Live package has no source request".to_owned())?;
        // Expand the real package through the application input path. The
        // helper already checks members; this verifies they reach the TUI tree
        // with their owner and overload identities intact.
        app.state.database.tree.selected = package_index;
        app.state
            .dispatch(&key(Key::Enter, false))
            .map_err(|error| error.to_string())?;
        until(&mut app, |app| {
            app.state.database.tree.lines.iter().any(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.attributes.get("packageName").map(String::as_str)
                        == Some(fixture.as_str())
                        && object.name.eq_ignore_ascii_case("increment_value")
                })
            }) && !app
                .state
                .database
                .tree
                .lines
                .iter()
                .any(|line| line.label == "Loading metadata…")
        })
        .await;
        let members: Vec<_> = app
            .state
            .database
            .tree
            .lines
            .iter()
            .filter_map(|line| line.object.as_ref())
            .filter(|object| {
                object.attributes.get("packageName").map(String::as_str)
                    == Some(fixture.as_str())
            })
            .collect();
        let member_ids: std::collections::BTreeSet<_> =
            members.iter().map(|object| object.id.as_str()).collect();
        if members.len() != 3
            || member_ids.len() != 3
            || members.iter().any(|object| object.schema.as_deref() != Some(&schema))
            || members
                .iter()
                .filter(|object| object.object_type == DatabaseObjectType::Function)
                .count()
                != 2
            || members
                .iter()
                .filter(|object| object.object_type == DatabaseObjectType::Procedure)
                .count()
                != 1
        {
            return Err("Live package members lost their owner, type or overload identity in the TUI tree".into());
        }
        app.state.database.tree.selected = package_index;
        app.state
            .dispatch(&key(Key::Character('m'), false))
            .map_err(|error| error.to_string())?;
        let menu = app
            .state
            .database
            .menu
            .as_ref()
            .ok_or_else(|| "Keyboard package action did not open its source menu".to_owned())?;
        if menu.object.id != package.id
            || !menu.actions.iter().any(|(label, _)| {
                label == "View package DDL" || label == "View package source"
            })
        {
            return Err("Keyboard package action opened the wrong source menu".into());
        }
        app.state
            .dispatch(&key(Key::Escape, false))
            .map_err(|error| error.to_string())?;
        let area = Rect::new(0, 0, 110, 34);
        let mut screen = Buffer::empty(area);
        app.state
            .render(area, &mut screen, &Theme::compiled())
            .map_err(|error| error.to_string())?;
        let (x, y) = (area.y..area.bottom())
            .find_map(|y| {
                (area.x..area.right().saturating_sub(7)).find_map(|x| {
                    let cells: String = (0..8).map(|offset| screen[(x + offset, y)].symbol()).collect();
                    (cells == "[source]").then_some((x, y))
                })
            })
            .ok_or_else(|| "Live package source button was not visible".to_owned())?;
        app.state
            .dispatch(&InputEvent::Pointer(PointerInput {
                x,
                y,
                action: PointerAction::Down(crossterm::event::MouseButton::Left),
                modifiers: Modifiers::default(),
            }))
            .map_err(|error| error.to_string())?;
        if app
            .state
            .database
            .menu
            .as_ref()
            .is_none_or(|menu| menu.object.id != package.id)
        {
            return Err("Live package source button opened the wrong menu".into());
        }
        app.state
            .database
            .dispatch(&key(Key::Enter, false), &mut app.state.focus)
            .map_err(|error| error.to_string())?;
        let source_work = app
            .state
            .database
            .take_intents()
            .into_iter()
            .find_map(|intent| match intent {
                kode_tui::panels::database::controller::Intent::Work(work) => Some(*work),
                _ => None,
            })
            .ok_or_else(|| "Package source action did not schedule its controller work".to_owned())?;
        if !matches!(
            &source_work.operation,
            kode_tui::panels::database::controller::Operation::Source { .. }
        ) {
            return Err("Package source action scheduled the wrong operation".into());
        }
        let stale_source = services
            .metadata
            .source(&source, source_request.clone(), &CancellationToken::new())
            .await
            .map_err(|error| error.to_string())?;
        // Match the cache-generation change performed by a metadata refresh before publication.
        services.metadata.invalidate(&source, &source_request);
        let stale_context_is_current = services.metadata.source_is_current(
            &source,
            &source_request,
            &stale_source,
        );
        app.state.database.complete(
            source_work.tag,
            source_work.context(),
            Ok(kode_tui::panels::database::controller::Report::Source(
                stale_source,
            )),
            stale_context_is_current,
            &mut app.state.focus,
        );
        if stale_context_is_current
            || app.state.database.tree.notice
                != "Package source context changed; reopen it from the current connection"
            || app.state.database.take_intents().iter().any(|intent| {
                matches!(
                    intent,
                    kode_tui::panels::database::controller::Intent::Source { .. }
                )
            })
        {
            return Err("Refreshed package metadata did not reject the stale source result".into());
        }

        app.state.tabs.selected = DATABASE;
        app.state.focus.focused = Some(app.state.database.tree.id);
        app.state.database.tree.selected = package_index;
        app.state.database.menu = Some(ObjectMenu::new(source.clone(), package));
        app.state.dispatch(&key(Key::Enter, false)).map_err(|e| e.to_string())?;
        app.state.tabs.selected = kode_tui::app::panel::SETTINGS;
        app.state.focus_panel();
        until(&mut app, |app| {
            app.state.database.tree.notice
                == "Package source context changed; reopen it from the selected object"
        })
        .await;
        if app.state.editor.is_some() {
            return Err("Late package source completion replaced the newer Settings focus".into());
        }

        app.state.tabs.selected = DATABASE;
        app.state.focus_panel();
        let package = app.state.database.tree.lines[package_index]
            .object
            .as_ref()
            .unwrap()
            .clone();
        app.state.database.menu = Some(ObjectMenu::new(source.clone(), package));
        app.state.dispatch(&key(Key::Enter, false)).map_err(|e| e.to_string())?;
        until(&mut app, |app| {
            app.state.editor.as_ref().is_some_and(|editor| {
                editor.host.descriptor.role == HostRole::Historical
                    && editor.host.descriptor.read_only
            })
        })
        .await;
        let editor = app.state.editor.as_ref().unwrap();
        if app.state.focus.focused != Some(editor.input.id) {
            return Err("Native package-source action did not focus its read-only editor".into());
        }
        if editor.host.descriptor.save_target != SaveTarget::None
            || editor.host.descriptor.execute.is_some()
        {
            return Err("Native package-source editor exposed save or execute".into());
        }
        let text = app
            .state
            .documents
            .document(editor.document_id())
            .map_err(|error| error.to_string())?
            .snapshot()
            .text()
            .to_string();
        if !text.contains("native-tui-source-界:") || !text.contains("RETURN n + 1") {
            return Err("Native TUI editor did not display the actual package body".into());
        }
        let uppercase = text.to_ascii_uppercase();
        if !uppercase.contains("PACKAGE")
            || !uppercase.contains("FUNCTION")
            || !uppercase.contains("INCREMENT_VALUE")
        {
            return Err("Native TUI editor did not display the package specification".into());
        }
        let source_document = editor.document_id();
        let cancellation_package = app.state.database.tree.lines[package_index]
            .object
            .as_ref()
            .unwrap()
            .clone();
        app.state.tabs.selected = DATABASE;
        app.state.focus.focused = Some(app.state.database.tree.id);
        app.state.database.tree.selected = package_index;
        app.state.database.menu = Some(ObjectMenu::new(source.clone(), cancellation_package.clone()));
        app.state.dispatch(&key(Key::Enter, false)).map_err(|e| e.to_string())?;
        app.pump().map_err(|error| error.to_string())?;
        tokio::time::sleep(Duration::from_millis(5)).await;
        app.state.dispatch(&key(Key::Escape, false)).map_err(|e| e.to_string())?;
        if app.state.database.tree.notice != "Package source request cancelled" {
            return Err("Esc did not cancel the live package-source request".into());
        }
        let cancel_deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < cancel_deadline {
            app.pump().map_err(|error| error.to_string())?;
            paint(&mut app);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        if app.state.database.tree.notice != "Package source request cancelled"
            || app.state.focus.focused != Some(app.state.database.tree.id)
            || app.state.editor.as_ref().map(|view| view.document_id()) != Some(source_document)
            || !app.state.database.take_intents().iter().all(|intent| {
                !matches!(intent, kode_tui::panels::database::controller::Intent::Source { .. })
            })
        {
            return Err("Cancelled live package-source completion replaced the editor or focus".into());
        }
        if vendor == "oracle" {
            app.state
                .open_database_console(source.clone(), String::new())
                .map_err(|error| error.to_string())?;
            selected_sql(&mut app, "SELECT 1 FROM DUAL;").await;
            selected_sql(&mut app, "BEGIN NULL; END;").await;
            execute_script(
                &mut app,
                &format!(
                    "INSERT INTO {fixture_table} VALUES (1);\nBEGIN INSERT INTO {fixture_table} VALUES (2); END;\nSELECT COUNT(*) FROM {fixture_table};"
                ),
            )
            .await;
            let result = app
                .state
                .sql
                .manager
                .get(app.state.active_sql().unwrap())
                .unwrap()
                .result
                .as_ref()
                .unwrap()
                .clone();
            if !result.preview.result_sets.iter().any(|set| {
                set.chunks.iter().flat_map(|chunk| chunk.iter()).any(|row| {
                    matches!(row.first(), Some(kode_database::bridge::wire::Cell::Text { text, .. }) if text == "2")
                })
            }) {
                return Err("Oracle script did not execute all statements before its final query".into());
            }
            execute_failing_script(
                &mut app,
                &format!(
                    "INSERT INTO {fixture_table} VALUES (3); INSERT INTO KODE_RS_NO_TABLE_{} VALUES (9); INSERT INTO {fixture_table} VALUES (4);",
                    std::process::id()
                ),
            )
            .await;
            selected_sql(&mut app, &format!("SELECT COUNT(*) FROM {fixture_table};")).await;
            let result = app
                .state
                .sql
                .manager
                .get(app.state.active_sql().unwrap())
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            if !result.preview.result_sets.iter().any(|set| {
                set.chunks.iter().flat_map(|chunk| chunk.iter()).any(|row| {
                    matches!(row.first(), Some(kode_database::bridge::wire::Cell::Text { text, .. }) if text == "3")
                })
            }) {
                return Err("Oracle script executed statements after its first failure".into());
            }
            sql_control(&mut app, 'a').await;
            selected_sql(&mut app, &format!("INSERT INTO {fixture_table} VALUES (5);")).await;
            sql_control(&mut app, 'r').await;
            selected_sql(&mut app, &format!("SELECT COUNT(*) FROM {fixture_table};")).await;
            let count = app
                .state
                .sql
                .manager
                .get(app.state.active_sql().unwrap())
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            if !count.preview.result_sets.iter().any(|set| {
                set.chunks.iter().flat_map(|chunk| chunk.iter()).any(|row| {
                    matches!(row.first(), Some(kode_database::bridge::wire::Cell::Text { text, .. }) if text == "3")
                })
            }) {
                return Err("Oracle rollback left an uncommitted insert visible".into());
            }
            selected_sql(&mut app, &format!("INSERT INTO {fixture_table} VALUES (6);")).await;
            sql_control(&mut app, 'c').await;
            selected_sql(&mut app, &format!("SELECT COUNT(*) FROM {fixture_table};")).await;
            let count = app
                .state
                .sql
                .manager
                .get(app.state.active_sql().unwrap())
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            if !count.preview.result_sets.iter().any(|set| {
                set.chunks.iter().flat_map(|chunk| chunk.iter()).any(|row| {
                    matches!(row.first(), Some(kode_database::bridge::wire::Cell::Text { text, .. }) if text == "4")
                })
            }) {
                return Err("Oracle commit did not retain the inserted row".into());
            }
            cancel_query(
                &mut app,
                "SELECT SUM(DBMS_RANDOM.VALUE) FROM DUAL CONNECT BY LEVEL <= 1000000000",
            )
            .await;
        }
        Ok(())
    }
    .await;
    let cleanup = if created {
        let package = execute(
            &services.executor,
            &external_source,
            format!("DROP PACKAGE \"{fixture}\""),
        )
        .await;
        if vendor == "oracle" {
            let table = execute(
                &services.executor,
                &external_source,
                format!("DROP TABLE {fixture_table} PURGE"),
            )
            .await;
            package.and(table)
        } else {
            package
        }
    } else {
        Ok(())
    };
    let refreshed: Result<(), String> = if result.is_ok() && cleanup.is_ok() {
        async {
            app.state.tabs.selected = DATABASE;
            app.state.focus_panel();
            let category = app
                .state
                .database
                .tree
                .lines
                .iter()
                .position(|line| {
                    line.object.as_ref().is_some_and(|object| {
                        object.object_type == DatabaseObjectType::Category
                            && object.name == "Packages"
                    })
                })
                .ok_or_else(|| {
                    "Package category disappeared before post-drop refresh".to_owned()
                })?;
            app.state.database.tree.selected = category;
            app.state
                .dispatch(&key(Key::Character('r'), false))
                .map_err(|error| error.to_string())?;
            if !app
                .state
                .database
                .tree
                .lines
                .iter()
                .any(|line| line.label == "Loading metadata…")
            {
                return Err("Package refresh did not start a metadata request".into());
            }
            until(&mut app, |app| {
                app.state.database.tree.notice.starts_with("Loaded")
                    && !app
                        .state
                        .database
                        .tree
                        .lines
                        .iter()
                        .any(|line| line.label == "Loading metadata…")
            })
            .await;
            if app.state.database.tree.lines.iter().any(|line| {
                line.object.as_ref().is_some_and(|object| {
                    object.object_type == DatabaseObjectType::Package && object.name == fixture
                })
            }) {
                return Err("Dropped package remained in the refreshed TUI tree".into());
            }
            Ok(())
        }
        .await
    } else {
        Ok(())
    };
    if external_opened {
        let _ = services.connections.close(&external_source, &token).await;
    }
    if opened {
        let _ = services.connections.close(&source, &token).await;
    }
    services.metadata.shutdown().await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    cleanup.unwrap();
    result.unwrap();
    refreshed.unwrap();
}
