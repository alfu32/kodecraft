use kode_core::{DocumentOrigin, WorkspaceGeneration};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock},
};
use kode_tui::{
    app::{
        runtime::AppRuntime,
        state::{AppWork, ConfigAction},
        workspace::prepare_workspace,
    },
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
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
fn make(root: &Path, file: Option<&str>, clock: Arc<FakeClock>) -> AppRuntime {
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        file.map(|file| root.join(file)),
        &CancellationToken::new(),
    )
    .unwrap();
    AppRuntime::with_services(prepared, Arc::new(ClipboardService::internal()), clock).unwrap()
}
async fn until(app: &mut AppRuntime, predicate: impl Fn(&AppRuntime) -> bool) {
    let start = Instant::now();
    loop {
        app.pump().unwrap();
        if predicate(app) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "timeout: {}; {}; watcher={:?}; LSP: {} (busy={:?}, statuses={})",
            app.state.notice,
            app.state.intelligence_status,
            app.state.watcher.as_ref().map(|w| w.source_revision()),
            app.state.lsp_settings.message,
            app.state.lsp_settings.busy,
            app.state.lsp_settings.statuses.len()
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
fn paint(app: &mut AppRuntime) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
    app.state
        .render(buffer.area, &mut buffer, &Theme::compiled())
        .unwrap();
}

fn configure_editor_lsp_fixture(app: &mut AppRuntime, directory: &Path) {
    use kode_lsp::{catalog::Platform, discovery::DiscoveryOptions};
    use kode_platform::paths::ResourcePaths;
    use kode_tui::app::runtime::lsp::LspConfiguration;
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/lsp/server.py")
        .canonicalize()
        .unwrap();
    let catalog = directory.join("catalog.json");
    fs::write(&catalog,serde_json::to_vec(&serde_json::json!({"servers":[{
        "id":"fixture","name":"Fixture","languages":["kotlin"],"versions":[],
        "preferredCommand":["/usr/bin/python3",script,"fragment",directory.join("trace.jsonl"),directory.join("pid")],
        "configuration":{"fixture":{"option":42}}
    }]})).unwrap()).unwrap();
    app.configure_lsp(LspConfiguration {
        paths: ResourcePaths {
            explicit: None,
            rust_home: None,
            legacy_home: None,
            executable_dir: directory.into(),
            config_dir: directory.join("config"),
        },
        catalog: Some(catalog),
        discovery: DiscoveryOptions {
            platform: Platform::detect(),
            install_root: directory.join("install"),
            search_path: vec![],
            configured_commands: Default::default(),
        },
    })
    .unwrap();
}
fn lsp_trace(directory: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(directory.join("trace.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
fn editor_caret(app: &mut AppRuntime, offset: usize) {
    let id = app.state.editor.as_ref().unwrap().document_id();
    let snapshot = app.state.documents.document(id).unwrap().snapshot();
    app.state
        .editor
        .as_mut()
        .unwrap()
        .input
        .state
        .caret
        .move_to(&snapshot, kode_core::ByteOffset(offset), false)
        .unwrap();
    paint(app);
}
#[tokio::test]
async fn editor_lsp_auto_start_completion_save_native_fallback_and_late_reply_keep_typing_and_focus()
 {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let source = "class Customer { val id: Long = 0; val name: String = \"\" }\nfun use(customer: Customer) {\n    customer.\n}\n";
    fs::write(root.join("Use.kt"), source).unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(&root, Some("Use.kt"), clock.clone());
    configure_editor_lsp_fixture(&mut app, directory.path());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.is_some() && app.lsp_service().unwrap().running("fixture")
    })
    .await;
    clock.0.store(300, Ordering::SeqCst);
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    let client = app.lsp_service().unwrap().client("fixture").unwrap();
    let id = app.state.editor.as_ref().unwrap().document_id();
    let position = source.find("customer.\n").unwrap() + "customer.".len();
    editor_caret(&mut app, position);
    let focus = app.state.focus.focused;
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |app| {
        !app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty()
    })
    .await;
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .iter()
            .map(|item| item.source.as_str())
            .collect::<Vec<_>>(),
        ["lsp"]
    );
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("customer.naïve\n")
    );
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(&mut app, |app| {
        !app.state.documents.document(id).unwrap().is_dirty()
            && lsp_trace(directory.path())
                .iter()
                .any(|message| message["method"] == "textDocument/didSave")
    })
    .await;
    assert!(
        fs::read_to_string(root.join("Use.kt"))
            .unwrap()
            .contains("customer.naïve\n")
    );
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('u'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    editor_caret(&mut app, position);
    clock.0.store(600, Ordering::SeqCst);
    app.pump().unwrap();
    // Index-only publication must not discard a current authoritative LSP reply.
    client
        .request(
            "fixture/configure",
            json!({"delay":0.3}),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |_| client.pending_requests() > 0).await;
    let previous_index = app.intelligence().unwrap().snapshot().unwrap().version();
    app.rebuild_index().unwrap();
    until(&mut app, |app| {
        app.intelligence().unwrap().snapshot().unwrap().version() > previous_index
    })
    .await;
    until(&mut app, |app| {
        !app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty()
    })
    .await;
    assert_eq!(
        app.state.editor.as_ref().unwrap().completion.entries[0].source,
        "lsp"
    );
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    for behavior in [json!({"empty":true}), json!({"error":true})] {
        client
            .request("fixture/configure", behavior, &CancellationToken::new())
            .await
            .unwrap();
        app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
        until(&mut app, |app| {
            !app.state
                .editor
                .as_ref()
                .unwrap()
                .completion
                .entries
                .is_empty()
        })
        .await;
        let entries = &app.state.editor.as_ref().unwrap().completion.entries;
        assert_eq!(
            entries
                .iter()
                .map(|item| item.label.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            ["id", "name"].into_iter().collect()
        );
        assert!(entries.iter().all(|item| item.source != "lsp"));
        app.state.dispatch(&key(Key::Escape, false)).unwrap();
    }
    client
        .request(
            "fixture/configure",
            json!({"delay":0.25}),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let prior = lsp_trace(directory.path())
        .iter()
        .filter(|message| message["method"] == "textDocument/completion")
        .count();
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |_| {
        lsp_trace(directory.path())
            .iter()
            .filter(|message| message["method"] == "textDocument/completion")
            .count()
            > prior
    })
    .await;
    assert!(client.pending_requests() > 0);
    app.state
        .dispatch(&InputEvent::Paste("draft".into()))
        .unwrap();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("customer.draft\n")
    );
    assert_eq!(app.state.focus.focused, focus);
    until(&mut app, |_| client.pending_requests() == 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    app.pump().unwrap();
    assert!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty(),
        "late LSP reply resurrected a dismissed popup"
    );
    assert_eq!(app.state.focus.focused, focus);
    let _ = client
        .request("fixture/crash", json!({}), &CancellationToken::new())
        .await;
    until(&mut app, |app| !app.state.lsp_settings.statuses[0].running).await;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('u'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    editor_caret(&mut app, position);
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |app| {
        !app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty()
    })
    .await;
    assert!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .iter()
            .all(|item| item.source != "lsp")
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

fn ctrl_click_editor(app: &mut AppRuntime, offset: usize) {
    use kode_tui::editor::input::InputViewport;
    paint(app);
    let view = app.state.editor.as_mut().unwrap();
    let document = app
        .state
        .documents
        .document(view.document_id())
        .unwrap()
        .clone();
    let geometry = view
        .geometry(&app.state.documents, app.state.geometry.viewer)
        .unwrap();
    let layout = view
        .input
        .layout(
            &document,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let (row, column) = layout
        .position_to_cell(kode_core::ByteOffset(offset))
        .unwrap();
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: geometry.text.x + column as u16,
            y: geometry.text.y + row as u16,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
        }))
        .unwrap();
}
#[tokio::test]
async fn editor_lsp_definition_links_navigate_exact_unicode_bytes_and_empty_primary_uses_native_targets()
 {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    use kode_tui::app::{state::OpenDestination, workspace::OpenRequest};
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let src = root.join("src");
    fs::create_dir(&src).unwrap();
    let source = "fun use(customer: Customer) {}\n";
    fs::write(src.join("Use.kt"), source).unwrap();
    fs::write(src.join("Customer.kt"), "class Customer {}\n").unwrap();
    let primary_source = "// 😀\r\nclass Primary {}\r\n";
    fs::write(src.join("Primary.kt"), primary_source).unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(&root, Some("src/Use.kt"), clock.clone());
    configure_editor_lsp_fixture(&mut app, directory.path());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.is_some() && app.lsp_service().unwrap().running("fixture")
    })
    .await;
    clock.0.store(300, Ordering::SeqCst);
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    let client = app.lsp_service().unwrap().client("fixture").unwrap();
    client.request("fixture/configure",json!({"textDocument/definition":[{
        "targetUri":kode_lsp::documents::file_uri(&src.join("Primary.kt")).unwrap().as_str(),
        "targetRange":{"start":{"line":1,"character":0},"end":{"line":1,"character":16}},
        "targetSelectionRange":{"start":{"line":1,"character":6},"end":{"line":1,"character":13}}
    }]}),&CancellationToken::new()).await.unwrap();
    ctrl_click_editor(&mut app, source.find("Customer").unwrap());
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .origin()
                == &DocumentOrigin::File(src.join("Primary.kt"))
        })
    })
    .await;
    let view = app.state.editor.as_ref().unwrap();
    assert_eq!(
        view.input.state.caret.cursor().0,
        primary_source.find("Primary").unwrap()
    );
    client
        .request(
            "fixture/configure",
            json!({"empty":true}),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    app.state
        .open(
            OpenRequest {
                path: src.join("Use.kt"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .origin()
                == &DocumentOrigin::File(src.join("Use.kt"))
        })
    })
    .await;
    // Opening changes native query epoch before this immediate click is queued.
    ctrl_click_editor(&mut app, source.find("Customer").unwrap());
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .origin()
                == &DocumentOrigin::File(src.join("Customer.kt"))
        })
    })
    .await;
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .cursor()
            .0,
        6
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn builtin_lsp_settings_start_restart_crash_repaint_switch_and_quit_reap_actual_children() {
    use kode_lsp::{catalog::Platform, discovery::DiscoveryOptions};
    use kode_platform::paths::ResourcePaths;
    use kode_tui::app::runtime::lsp::LspConfiguration;
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let next = directory.path().join("next");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&next).unwrap();
    let trace = directory.path().join("trace.jsonl");
    let pid_file = directory.path().join("pid");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/lsp/server.py")
        .canonicalize()
        .unwrap();
    let catalog = directory.path().join("catalog.json");
    fs::write(
        &catalog,
        serde_json::to_vec(&json!({"servers":[{
            "id":"fixture", "name":"Fixture", "languages":["kotlin"],
            "preferredCommand":["/usr/bin/python3", fixture, "fragment", trace, pid_file],
            "configuration":{"fixture":{"option":42}}, "versions":[]
        }]}))
        .unwrap(),
    )
    .unwrap();
    let mut app = make(&root, None, Arc::new(FakeClock::default()));
    app.configure_lsp(LspConfiguration {
        paths: ResourcePaths {
            explicit: None,
            rust_home: None,
            legacy_home: None,
            executable_dir: directory.path().into(),
            config_dir: directory.path().join("config"),
        },
        catalog: Some(catalog),
        discovery: DiscoveryOptions {
            platform: Platform::detect(),
            install_root: directory.path().join("install"),
            search_path: vec![],
            configured_commands: Default::default(),
        },
    })
    .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.statuses.len() == 1 && app.state.lsp_settings.busy.is_none()
    })
    .await;
    let service = app.lsp_service().unwrap();
    assert!(
        !service.has_running_servers(),
        "discovery must not launch servers"
    );
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('4'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    paint(&mut app);
    let mut previous_pid = None;
    for (ch, acknowledgement) in [('s', "Started fixture"), ('r', "Restarted fixture")] {
        app.state.dispatch(&key(Key::Character(ch), false)).unwrap();
        until(&mut app, |app| {
            app.state.lsp_settings.busy.is_none()
                && app.state.lsp_settings.message == acknowledgement
                && app.state.lsp_settings.statuses[0].running
        })
        .await;
        let pid = service.client("fixture").unwrap().process_id();
        if let Some(previous) = previous_pid {
            assert_ne!(previous, pid);
            #[cfg(target_os = "linux")]
            assert!(!Path::new(&format!("/proc/{previous}")).exists());
        }
        previous_pid = Some(pid);
    }
    // A process failure changes the rendered state without a key or catalog read.
    let client = service.client("fixture").unwrap();
    let _ = client
        .request("fixture/crash", json!({}), &CancellationToken::new())
        .await;
    until(&mut app, |app| {
        !app.state.lsp_settings.statuses[0].running
            && app.state.lsp_settings.statuses[0].message.is_some()
    })
    .await;
    app.state
        .dispatch(&key(Key::Character('s'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.busy.is_none() && app.state.lsp_settings.statuses[0].running
    })
    .await;
    let before_switch = service.client("fixture").unwrap().process_id();
    app.switch_workspace(next).await.unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.statuses.len() == 1 && app.state.lsp_settings.busy.is_none()
    })
    .await;
    assert!(!service.has_running_servers());
    let next_service = app.lsp_service().unwrap();
    assert!(!Arc::ptr_eq(&service, &next_service));
    assert!(!next_service.has_running_servers());
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{before_switch}")).exists());
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('4'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    paint(&mut app);
    app.state
        .dispatch(&key(Key::Character('s'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.busy.is_none() && app.state.lsp_settings.statuses[0].running
    })
    .await;
    let before_quit = next_service.client("fixture").unwrap().process_id();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert!(!next_service.has_running_servers());
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{before_quit}")).exists());
    let messages: Vec<serde_json::Value> = fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["method"] == "initialize")
            .count(),
        4
    );
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["method"] == "shutdown")
            .count(),
        3
    );
}

#[tokio::test]
async fn lsp_settings_use_real_download_workers_then_acknowledged_lifecycle_callbacks_and_preserve_the_install_root()
 {
    use kode_lsp::{
        catalog::Platform,
        discovery::{DiscoveryOptions, InstallState},
    };
    use kode_platform::paths::ResourcePaths;
    use kode_tui::{
        app::runtime::lsp::LspConfiguration,
        panels::settings::lsp::{LifecycleAction, ServerLifecycle},
    };
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        sync::{Mutex, atomic::AtomicBool},
    };
    struct Lifecycle {
        running: AtomicBool,
        calls: Mutex<Vec<LifecycleAction>>,
        stops: AtomicU64,
    }
    impl ServerLifecycle for Lifecycle {
        fn running(&self, _: &str) -> bool {
            self.running.load(Ordering::SeqCst)
        }
        fn apply(
            &self,
            id: &str,
            action: LifecycleAction,
            cancel: &CancellationToken,
        ) -> kode_core::Result<String> {
            assert_eq!(id, "local-server");
            if cancel.is_cancelled() {
                return Err(kode_core::DomainError::Cancelled);
            }
            self.calls.lock().unwrap().push(action);
            self.running
                .store(action != LifecycleAction::Stop, Ordering::SeqCst);
            Ok(format!("Acknowledged {action:?}"))
        }
        fn stop_all(&self) -> kode_core::Result<()> {
            self.running.store(false, Ordering::SeqCst);
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let next = directory.path().join("next");
    fs::create_dir(&next).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut input = [0; 4096];
        assert!(stream.read(&mut input).unwrap() > 0);
        let body = b"#!/bin/sh\nexit 0\n";
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        stream.write_all(body).unwrap();
    });
    let catalog = directory.path().join("servers.json");
    fs::write(&catalog,serde_json::to_vec(&serde_json::json!({"servers":[{"id":"local-server","name":"Local Server","languages":["kotlin"],"preferredCommand":["server","--stdio","two words"],"versions":[{"version":"1","downloads":[{"url":format!("http://{address}/server")}]}]}]})).unwrap()).unwrap();
    let install = directory.path().join("install root");
    fs::create_dir(&install).unwrap();
    fs::write(install.join("user.keep"), b"keep").unwrap();
    let mut app = make(&root, None, Arc::new(FakeClock::default()));
    app.configure_lsp(LspConfiguration {
        paths: ResourcePaths {
            explicit: None,
            rust_home: None,
            legacy_home: None,
            executable_dir: directory.path().into(),
            config_dir: directory.path().join("config"),
        },
        catalog: Some(catalog),
        discovery: DiscoveryOptions {
            platform: Platform::detect(),
            install_root: install.clone(),
            search_path: vec![],
            configured_commands: BTreeMap::new(),
        },
    })
    .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.statuses.len() == 1 && app.state.lsp_settings.busy.is_none()
    })
    .await;
    let lifecycle = Arc::new(Lifecycle {
        running: AtomicBool::new(false),
        calls: Mutex::new(vec![]),
        stops: AtomicU64::new(0),
    });
    app.set_lsp_lifecycle(lifecycle.clone()).unwrap();
    until(&mut app, |app| app.state.lsp_settings.busy.is_none()).await;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('4'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    paint(&mut app);
    assert_eq!(app.state.focus.focused, Some(app.state.lsp_settings.id));
    assert_eq!(
        app.state.lsp_settings.statuses[0].install_state,
        InstallState::Missing
    );
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.busy.is_none()
            && app.state.lsp_settings.message.starts_with("Installed ")
    })
    .await;
    server.join().unwrap();
    let status = &app.state.lsp_settings.statuses[0];
    assert_eq!(status.install_state, InstallState::Installed);
    assert!(!status.running);
    assert_eq!(
        status.command.as_ref().unwrap().args[1..],
        ["--stdio", "two words"]
    );
    for (ch, action) in [
        ('s', LifecycleAction::Start),
        ('r', LifecycleAction::Restart),
    ] {
        app.state.dispatch(&key(Key::Character(ch), false)).unwrap();
        until(&mut app, |app| {
            app.state.lsp_settings.busy.is_none()
                && app.state.lsp_settings.message == format!("Acknowledged {action:?}")
        })
        .await;
        assert!(app.state.lsp_settings.statuses[0].running);
    }
    app.state
        .dispatch(&key(Key::Character('y'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.busy.is_none()
            && app.state.lsp_settings.message.starts_with("Removed ")
    })
    .await;
    assert_eq!(
        *lifecycle.calls.lock().unwrap(),
        [
            LifecycleAction::Start,
            LifecycleAction::Restart,
            LifecycleAction::Stop
        ]
    );
    assert!(!app.state.lsp_settings.statuses[0].running);
    assert!(!install.join("local-server").exists());
    assert_eq!(fs::read(install.join("user.keep")).unwrap(), b"keep");
    let owner = app.state.lsp_settings.id;
    app.switch_workspace(next).await.unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.statuses.len() == 1 && app.state.lsp_settings.busy.is_none()
    })
    .await;
    assert_ne!(app.state.lsp_settings.id, owner);
    assert!(!app.state.lsp_settings.statuses[0].running);
    assert_eq!(lifecycle.stops.load(Ordering::SeqCst), 1);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert_eq!(lifecycle.stops.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn native_overlays_repaint_without_input_remap_live_edits_and_refresh_unchanged_dependents() {
    use kode_core::{ByteOffset, LineIndex};
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    let model = directory.path().join("src/Model.kt");
    fs::write(&model, "class Customer { val value: Long = 0 }\n").unwrap();
    let source = "// naïve — 😀\r\nfun inspect(customer: Customer) { customer.value; }\r\n";
    fs::write(directory.path().join("src/Use.kt"), source).unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(directory.path(), Some("src/Use.kt"), clock.clone());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    let mut events = app.notifications.subscribe();
    until(&mut app, |app| app.state.editor.is_some()).await;
    paint(&mut app);
    clock.0.store(300, Ordering::SeqCst);
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
            && app
                .state
                .editor
                .as_ref()
                .unwrap()
                .overlays
                .spans()
                .any(|s| s.class == "semantic.property")
    })
    .await;
    assert!(
        events.has_changed().unwrap(),
        "worker completion must emit repaint without keyboard/mouse input"
    );
    events.borrow_and_update();
    let id = app.state.editor.as_ref().unwrap().document_id();
    let original_revision = app.state.documents.document(id).unwrap().revision();
    let byte = source.find("value").unwrap();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
    app.state
        .render(buffer.area, &mut buffer, &Theme::compiled())
        .unwrap();
    let geometry = app
        .state
        .editor
        .as_ref()
        .unwrap()
        .geometry(&app.state.documents, app.state.geometry.viewer)
        .unwrap();
    let snapshot = app.state.documents.document(id).unwrap().snapshot();
    let layout =
        kode_tui::editor::layout::VisualLayout::build(&snapshot, geometry.text.width as usize, &[])
            .unwrap();
    let (row, column) = layout.position_to_cell(ByteOffset(byte)).unwrap();
    assert_eq!(
        buffer[(
            geometry.text.x + column as u16,
            geometry.text.y + row as u16
        )]
            .fg,
        Theme::compiled().style("semantic.property").fg.unwrap()
    );
    fs::write(&model, "class Customer { fun value(): Long = 0 }\n").unwrap();
    app.rebuild_index().unwrap();
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
            && app
                .state
                .editor
                .as_ref()
                .unwrap()
                .overlays
                .spans()
                .any(|s| s.class == "semantic.method" && s.range.start == ByteOffset(byte))
    })
    .await;
    assert_eq!(
        app.state.documents.document(id).unwrap().revision(),
        original_revision,
        "dependent refresh must not fake an editor edit"
    );
    app.state
        .render(buffer.area, &mut buffer, &Theme::compiled())
        .unwrap();
    assert_eq!(
        buffer[(
            geometry.text.x + column as u16,
            geometry.text.y + row as u16
        )]
            .fg,
        Theme::compiled().style("semantic.method").fg.unwrap()
    );
    // Two edits before a fresh analysis finishes preserve the untouched method.
    app.state
        .dispatch(&InputEvent::Paste("// first 😀\n".into()))
        .unwrap();
    app.state
        .dispatch(&InputEvent::Paste("// second\n".into()))
        .unwrap();
    let current = app.state.documents.document(id).unwrap().snapshot();
    let moved = current.text().find("value").unwrap();
    assert_eq!(
        current.byte_to_position(ByteOffset(moved)).unwrap().line,
        LineIndex(3)
    );
    assert!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .overlays
            .spans()
            .any(|s| s.class == "semantic.method" && s.range.start == ByteOffset(moved))
    );
    clock.0.store(1000, Ordering::SeqCst);
    until(&mut app, |app| {
        app.state.editor.as_ref().unwrap().overlays.status.is_some()
            && app
                .state
                .editor
                .as_ref()
                .unwrap()
                .overlays
                .spans()
                .any(|s| s.class == "semantic.method" && s.range.start == ByteOffset(moved))
    })
    .await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn native_runtime_completes_through_real_editor_and_stops_old_facade_on_workspace_switch() {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::create_dir(first.path().join("src")).unwrap();
    fs::write(
        first.path().join("src/Customer.kt"),
        "class Customer { val id: Int = 0; val name: String = \"\" }\n",
    )
    .unwrap();
    let source = "fun use(customer: Customer) { customer.\n}\n";
    fs::write(first.path().join("src/Use.kt"), source).unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(first.path(), Some("src/Use.kt"), clock.clone());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| app.state.editor.is_some()).await;
    clock.0.store(300, Ordering::SeqCst);
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    let snapshot = app.state.documents.document(id).unwrap().snapshot();
    let caret = kode_core::ByteOffset(source.find("customer.\n").unwrap() + 9);
    app.state
        .editor
        .as_mut()
        .unwrap()
        .input
        .state
        .caret
        .move_to(&snapshot, caret, false)
        .unwrap();
    paint(&mut app);
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |app| {
        !app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty()
    })
    .await;
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .iter()
            .map(|i| i.label.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["id", "name"].into_iter().collect()
    );
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("customer.id")
    );
    assert!(app.state.intelligence_status.contains("index Ready"));
    app.pump().unwrap();
    paint(&mut app);
    let document = app.state.documents.document(id).unwrap().clone();
    let view = app.state.editor.as_mut().unwrap();
    let geometry = view
        .geometry(&app.state.documents, app.state.geometry.viewer)
        .unwrap();
    let layout = view
        .input
        .layout(
            &document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let (row, column) = layout
        .position_to_cell(kode_core::ByteOffset(source.find("Customer").unwrap()))
        .unwrap();
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: geometry.text.x + column as u16,
            y: geometry.text.y + row as u16,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
        }))
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .is_ok_and(|doc| {
                    doc.origin() == &DocumentOrigin::File(first.path().join("src/Customer.kt"))
                })
        })
    })
    .await;
    let provider = app.intelligence().unwrap().facade();
    let request = kode_core::intelligence::IntelligenceRequest {
        tag: kode_core::JobTag {
            workspace: WorkspaceGeneration(1),
            request: kode_core::RequestId::fresh().unwrap(),
            document: Some((snapshot.id, snapshot.revision)),
        },
        snapshot: snapshot.to_service_snapshot(),
        position: caret,
    };
    app.switch_workspace(second.path().into()).await.unwrap();
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    assert!(matches!(
        kode_core::intelligence::IntelligenceProvider::completions(provider.as_ref(), &request),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(
        !app.intelligence()
            .unwrap()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Customer")
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn native_runtime_clear_rebuild_and_external_source_changes_do_not_loop_on_sqlite_writes() {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    let file = directory.path().join("src/Model.kt");
    fs::write(&file, "class Before\n").unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(directory.path(), None, clock.clone());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    assert!(
        app.intelligence()
            .unwrap()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Before")
    );
    app.clear_index().unwrap();
    until(&mut app, |app| {
        let status = app.intelligence().unwrap().status().unwrap();
        status.phase == IndexPhase::Ready && status.files == 0
    })
    .await;
    clock.0.store(6_000, Ordering::SeqCst);
    app.pump().unwrap();
    assert_eq!(app.intelligence().unwrap().status().unwrap().files, 0);
    app.rebuild_index().unwrap();
    until(&mut app, |app| {
        let status = app.intelligence().unwrap().status().unwrap();
        status.phase == IndexPhase::Ready && status.files == 1
    })
    .await;
    let watch_revision = app.state.watcher.as_ref().unwrap().source_revision();
    fs::write(&file, "class After\n").unwrap();
    until(&mut app, |app| {
        app.state.watcher.as_ref().unwrap().source_revision() != watch_revision
    })
    .await;
    clock.0.store(6_200, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        // Notify may deliver create/write/close events in separate batches. A
        // fake clock must keep advancing through the trailing debounce window.
        clock.0.fetch_add(10, Ordering::SeqCst);
        app.pump().unwrap();
        if app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
            && app
                .intelligence()
                .unwrap()
                .snapshot()
                .unwrap()
                .workspace_symbols()
                .any(|s| s.name == "After")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{}",
            app.state.intelligence_status
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let revision = app.intelligence().unwrap().snapshot().unwrap().version();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(350) {
        clock.0.fetch_add(5, Ordering::SeqCst);
        app.pump().unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        app.intelligence().unwrap().snapshot().unwrap().version(),
        revision,
        "native cache writes must not rescan themselves"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn settings_index_clear_rebuild_uses_real_workers_and_preserves_dirty_editor_and_user_databases()
 {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir_all(root.join(".kode/db")).unwrap();
    let legacy = root.join(".kode/db/kode.mv.db");
    fs::write(&legacy, b"synthetic legacy H2 sentinel").unwrap();
    let jdbc = root.join("user-database.mv.db");
    fs::write(&jdbc, b"synthetic user JDBC sentinel").unwrap();
    fs::write(root.join("src/Model.kt"), "class Saved\n").unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(root, Some("src/Model.kt"), clock.clone());
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| {
        clock.0.fetch_add(10, Ordering::SeqCst);
        app.state.editor.is_some()
            && app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("class Unsaved {}\n".into()))
        .unwrap();
    app.pump().unwrap();
    let id = app.state.editor.as_ref().unwrap().document_id();
    let text = app.state.documents.document(id).unwrap().snapshot().text();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('4'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    paint(&mut app);
    app.state.dispatch(&key(Key::Tab, false)).unwrap();
    assert!(app.state.settings.index_selected);
    assert_eq!(
        app.state.focus.focused,
        Some(app.state.settings.native.id())
    );
    app.state
        .dispatch(&key(Key::Character('c'), false))
        .unwrap();
    until(&mut app, |app| {
        let status = app.intelligence().unwrap().status().unwrap();
        status.phase == IndexPhase::Ready && status.files == 0
    })
    .await;
    assert_eq!(
        app.state.documents.document(id).unwrap().snapshot().text(),
        text
    );
    assert_eq!(
        fs::read_to_string(root.join("src/Model.kt")).unwrap(),
        "class Saved\n"
    );
    assert_eq!(fs::read(&legacy).unwrap(), b"synthetic legacy H2 sentinel");
    assert_eq!(fs::read(&jdbc).unwrap(), b"synthetic user JDBC sentinel");
    app.state
        .dispatch(&key(Key::Character('b'), false))
        .unwrap();
    until(&mut app, |app| {
        let status = app.intelligence().unwrap().status().unwrap();
        status.phase == IndexPhase::Ready && status.files > 0
    })
    .await;
    assert!(
        app.intelligence()
            .unwrap()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|symbol| symbol.name == "Unsaved")
    );
    assert!(app.state.intelligence_status.contains("processed"));
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("// responsive\n".into()))
        .unwrap();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("// responsive")
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn native_rescan_abort_waits_for_file_and_index_acknowledgement_without_publishing_cancelled_work()
 {
    use kode_intelligence::{facade::IntelligenceResources, service::IndexPhase};
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    fs::write(directory.path().join("src/Model.kt"), "class Before\n").unwrap();
    let mut app = make(directory.path(), None, Arc::new(FakeClock::default()));
    app.start_intelligence(Arc::new(IntelligenceResources::compiled().unwrap()))
        .await
        .unwrap();
    until(&mut app, |app| {
        app.intelligence().unwrap().status().unwrap().phase == IndexPhase::Ready
    })
    .await;
    let held = app.intelligence().unwrap().snapshot().unwrap();
    app.state.request_scan(true).unwrap();
    app.state.dispatch(&key(Key::Escape, false)).unwrap();
    assert!(!app.state.rescan.as_ref().unwrap().done);
    until(&mut app, |app| app.state.rescan.as_ref().unwrap().done).await;
    assert!(app.state.rescan.as_ref().unwrap().aborted);
    assert_eq!(
        app.intelligence().unwrap().status().unwrap().phase,
        IndexPhase::Cancelled
    );
    assert_eq!(
        app.intelligence().unwrap().snapshot().unwrap().version(),
        held.version()
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn real_shell_opens_edits_saves_flushes_and_restores_shared_dirty_session() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/code.rs"), "original\n").unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut app = make(root, Some("src/code.rs"), clock.clone());
    until(&mut app, |app| {
        app.state.editor.is_some() && app.state.scan.files.len() == 1
    })
    .await;
    paint(&mut app);
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state.documents.document(id).unwrap().origin(),
        &DocumentOrigin::File(root.join("src/code.rs"))
    );
    app.state
        .dispatch(&InputEvent::Paste("changed ".into()))
        .unwrap();
    app.pump().unwrap();
    assert!(app.state.documents.document(id).unwrap().is_dirty());
    app.state.dispatch(&key(Key::Character('s'), true)).unwrap();
    until(&mut app, |app| {
        !app.state.documents.document(id).unwrap().is_dirty()
    })
    .await;
    assert_eq!(
        fs::read_to_string(root.join("src/code.rs")).unwrap(),
        "changed original\n"
    );
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Character('q'), true)).unwrap();
    app.flush_before_leave().await.unwrap();
    assert!(app.ready_to_leave());
    app.quiesce().await.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("src/code.rs")).unwrap(),
        "changed original\n"
    );
    let mut restored = make(root, None, clock);
    until(&mut restored, |app| app.state.editor.is_some()).await;
    let id = restored.state.editor.as_ref().unwrap().document_id();
    assert!(restored.state.documents.document(id).unwrap().is_dirty());
    assert_eq!(
        restored
            .state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "changed draft original\n"
    );
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}
#[tokio::test]
async fn workspace_switch_persists_old_state_advances_generation_and_rejects_old_jobs() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::write(first.path().join("one.rs"), "one").unwrap();
    fs::write(second.path().join("two.rs"), "two").unwrap();
    let mut app = make(first.path(), Some("one.rs"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.editor.is_some()).await;
    let previous_document = app.state.editor.as_ref().unwrap().document_id();
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    app.pump().unwrap();
    app.switch_workspace(second.path().into()).await.unwrap();
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    assert_eq!(app.state.project.root, second.path());
    assert!(app.state.documents.document(previous_document).is_err());
    assert!(app.state.documents.ids().all(|id| !matches!(
        app.state.documents.document(id).unwrap().origin(),
        DocumentOrigin::File(_)
    )));
    assert_eq!(
        app.state.documents.ids().count(),
        1,
        "the new workspace owns only its virtual commit-message document"
    );
    assert!(first.path().join(".kode-rs.json").exists());
    until(&mut app, |app| !app.state.files.tree.flattened().is_empty()).await;
    assert!(
        app.state
            .files
            .tree
            .flattened()
            .iter()
            .all(|entry| entry.entry.path.starts_with(second.path()))
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn project_changes_serialize_and_failed_persistence_keeps_workspace_and_dirty_buffers_open() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for name in ["alpha", "beta"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("file.rs"), "text").unwrap();
    let mut app = make(root, Some("file.rs"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.editor.is_some()).await;
    for path in ["alpha", "beta"] {
        let tag = app.state.tag().unwrap();
        app.state.work.push(AppWork::Configure {
            tag,
            action: ConfigAction::AddSource(path.into()),
        });
    }
    until(&mut app, |app| {
        app.state.project.config.source_roots.len() == 2
    })
    .await;
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    app.pump().unwrap();
    fs::write(root.join(".kode-rs.json"), "external state must remain").unwrap();
    assert!(app.flush_before_leave().await.is_err());
    assert!(!app.state.closing);
    assert_eq!(
        fs::read_to_string(root.join(".kode-rs.json")).unwrap(),
        "external state must remain"
    );
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert!(app.state.documents.document(id).unwrap().is_dirty());
    // Explicit test repair allows orderly cleanup, leaving the tested external
    // state in a separate recoverable file instead of discarding it.
    fs::rename(
        root.join(".kode-rs.json"),
        root.join("external-recovery.json"),
    )
    .unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn reopening_owned_missing_file_keeps_dirty_history_and_navigation_selects_utf16_position() {
    use kode_core::{
        ByteOffset, LineIndex, TextRange, Utf16Column, document::Utf16Position,
        intelligence::NavigationTarget,
    };
    use kode_tui::{
        app::{state::OpenDestination, workspace::OpenRequest},
        editor::host::EditorWork,
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("one.rs"), "one\n").unwrap();
    fs::write(
        root.join("two.rs"),
        format!("{}😀identifier\n", "line\n".repeat(60)),
    )
    .unwrap();
    let mut app = make(root, Some("two.rs"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.editor.is_some()).await;
    paint(&mut app);
    let id = app.state.editor.as_ref().unwrap().document_id();
    app.state
        .dispatch(&InputEvent::Paste("draft\n".into()))
        .unwrap();
    app.pump().unwrap();
    let history = app.state.documents.document(id).unwrap().history_depth();
    let text = app.state.documents.document(id).unwrap().snapshot().text();
    app.state
        .open(
            OpenRequest {
                path: root.join("one.rs"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| {
        app.state
            .editor
            .as_ref()
            .is_some_and(|view| view.document_id() != id)
    })
    .await;
    fs::remove_file(root.join("two.rs")).unwrap();
    let owner = app.state.editor.as_ref().unwrap().owner();
    app.state.work.push(AppWork::Editor {
        owner,
        work: EditorWork::Navigate(NavigationTarget {
            path: "two.rs".into(),
            document: Some(id),
            identifier_range: TextRange::caret(ByteOffset(0)),
            location: Some(Utf16Position {
                line: LineIndex(61),
                column: Utf16Column(2),
            }),
            declaration_range: None,
            language: Some("rust".into()),
            kind: None,
            name: None,
            missing_source: false,
        }),
    });
    until(&mut app, |app| {
        app.state
            .editor
            .as_ref()
            .is_some_and(|view| view.document_id() == id)
    })
    .await;
    paint(&mut app);
    let document = app.state.documents.document(id).unwrap();
    assert_eq!(document.snapshot().text(), text);
    assert_eq!(document.history_depth(), history);
    assert!(document.is_dirty());
    let view = app.state.editor.as_ref().unwrap();
    assert_eq!(
        document
            .snapshot()
            .byte_to_utf16(view.input.state.caret.cursor())
            .unwrap(),
        Utf16Position {
            line: LineIndex(61),
            column: Utf16Column(2)
        }
    );
    assert!(view.input.state.scroll_rows > 20);
    assert!(61 < view.input.state.scroll_rows + app.state.geometry.viewer.height as usize);
    assert_eq!(app.state.focus.focused, Some(view.input.id));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn terminal_failure_recovers_dirty_text_when_session_write_conflicts_and_joins_services() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("file.rs"), "original\n").unwrap();
    let mut app = make(root, Some("file.rs"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.editor.is_some()).await;
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("recover ".into()))
        .unwrap();
    app.pump().unwrap();
    fs::write(root.join(".kode-rs.json"), "external session").unwrap();
    assert!(app.terminal_failed().await.is_err());
    assert!(app.state.watcher.is_none());
    assert_eq!(
        fs::read_to_string(root.join(".kode-rs.json")).unwrap(),
        "external session"
    );
    let files: Vec<_> = fs::read_dir(root.join(".kode-rs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let recovered: kode_workspace::session::EditorStateV1 =
        serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
    assert_eq!(recovered.history.current.text, "recover original\n");
    assert_eq!(
        fs::read_to_string(root.join("file.rs")).unwrap(),
        "original\n"
    );
}

#[tokio::test]
async fn binary_routing_recents_rename_reopen_and_restore_keep_byte_edits_separate_from_files() {
    use kode_tui::app::{state::OpenDestination, workspace::OpenRequest};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("bytes.bin"), [0xff, 0xfe, 0, 65, 66]).unwrap();
    fs::write(root.join("text.rs"), "text\n").unwrap();
    let mut app = make(root, Some("bytes.bin"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.active_hex.is_some()).await;
    paint(&mut app);
    assert!(app.state.editor.is_none());
    assert!(app.state.notice.contains("in memory"));
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('x'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.active_hex_view().unwrap().buffer.is_empty());
    assert_eq!(
        fs::read(root.join("bytes.bin")).unwrap(),
        [0xff, 0xfe, 0, 65, 66]
    );
    app.state
        .open(
            OpenRequest {
                path: root.join("text.rs"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| app.state.editor.is_some()).await;
    app.state
        .open(
            OpenRequest {
                path: root.join("bytes.bin"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| app.state.active_hex.is_some()).await;
    assert!(app.state.active_hex_view().unwrap().buffer.is_empty());
    until(&mut app, |app| !app.state.files.tree.flattened().is_empty()).await;
    paint(&mut app);
    app.state.files.select_path(root.join("bytes.bin")).unwrap();
    app.state.focus.focused = Some(app.state.files.id);
    app.state.dispatch(&key(Key::Function(2), false)).unwrap();
    assert!(app.state.files.has_modal());
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("renamed bytes.bin".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.active_hex.as_deref() == Some(root.join("renamed bytes.bin").as_path())
    })
    .await;
    assert!(app.state.active_hex_view().unwrap().buffer.is_empty());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let session: kode_workspace::session::ProjectSessionV1 =
        serde_json::from_slice(&fs::read(root.join(".kode-rs.json")).unwrap()).unwrap();
    assert!(
        session
            .recent_files
            .iter()
            .any(|recent| recent.viewer_type == "HEX"
                && recent.path == Path::new("renamed bytes.bin"))
    );
    let mut restored = make(root, None, Arc::new(FakeClock::default()));
    until(&mut restored, |app| app.state.active_hex.is_some()).await;
    // Source only persists viewer recents; binary edits are explicitly temporary.
    assert_eq!(
        restored.state.active_hex_view().unwrap().buffer.bytes(),
        [0xff, 0xfe, 0, 65, 66]
    );
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}

#[tokio::test]
async fn image_route_worker_cells_controls_focus_recents_and_restart_are_real() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/images/checker.png");
    fs::copy(fixture, root.join("picture.png")).unwrap();
    let mut app = make(root, Some("picture.png"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.image.is_some()).await;
    paint(&mut app);
    until(&mut app, |app| {
        app.state
            .image
            .as_ref()
            .is_some_and(|view| view.cells.is_some())
    })
    .await;
    assert!(app.state.editor.is_none() && app.state.active_hex.is_none());
    let id = app.state.image.as_ref().unwrap().id;
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    assert_ne!(app.state.focus.focused, Some(id));
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    assert_eq!(app.state.focus.focused, Some(id));
    app.state.dispatch(&key(Key::Right, false)).unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    paint(&mut app);
    until(&mut app, |app| {
        app.state
            .image
            .as_ref()
            .is_some_and(|view| view.pending.is_none())
    })
    .await;
    assert_eq!(
        app.state.image.as_ref().unwrap().settings.mode,
        kode_platform::images::cells::CellMode::Demi
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let session: kode_workspace::session::ProjectSessionV1 =
        serde_json::from_slice(&fs::read(root.join(".kode-rs.json")).unwrap()).unwrap();
    assert_eq!(session.recent_files[0].viewer_type, "IMAGE");
    let mut restored = make(root, None, Arc::new(FakeClock::default()));
    until(&mut restored, |app| app.state.image.is_some()).await;
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}
#[tokio::test]
async fn invalid_image_falls_back_to_raw_hex_with_error_and_extension_defaults_respect_overrides() {
    use kode_tui::app::{state::OpenDestination, workspace::OpenRequest};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("broken.png"), b"\x89PNG\r\n\x1a\ninvalid").unwrap();
    let mut app = make(root, Some("broken.png"), Arc::new(FakeClock::default()));
    until(&mut app, |app| app.state.active_hex.is_some()).await;
    assert!(app.state.notice.contains("image"));
    assert!(app.state.image.is_none());
    fs::write(root.join("custom.tga"), "explicit text override").unwrap();
    let mut detector = kode_workspace::mime::MimeDetector::compiled();
    detector.register_extension("tga", "text/plain").unwrap();
    app.set_detector(detector).unwrap();
    app.state
        .open(
            OpenRequest {
                path: root.join("custom.tga"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| app.state.editor.is_some()).await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "explicit text override"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
