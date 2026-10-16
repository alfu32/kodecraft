use kode_core::WorkspaceGeneration;
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
    paths::ResourcePaths,
};
use kode_tui::{
    app::{
        runtime::{AppRuntime, lsp::LspConfiguration},
        state::AppWork,
        workspace::prepare_workspace,
    },
    input::terminal::*,
    panels::settings::lsp::SettingsAction,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{fs, sync::Arc, time::Duration};
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
async fn until(app: &mut AppRuntime, test: impl Fn(&AppRuntime) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            app.pump().unwrap();
            if test(app) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn actual_http_install_failure_opens_wrapped_owned_details_and_copies_the_full_error() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/{}ACTIONABLE-END.zip",
        listener.local_addr().unwrap(),
        "long-path/".repeat(600)
    );
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = vec![];
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
            assert!(header.len() < 16384);
        }
        socket
            .write_all(
                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let catalog = root.path().join("catalog.json");
    fs::write(&catalog,serde_json::to_vec(&serde_json::json!({"servers":[{"id":"fixture","name":"Fixture","languages":["fixture"],"preferredCommand":["absent-fixture-server"],"versions":[{"version":"1","downloads":[{"url":url,"command":["absent-fixture-server"]}]}]}]})).unwrap()).unwrap();
    let prepared = prepare_workspace(
        root.path(),
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
    app.configure_lsp(LspConfiguration {
        paths: ResourcePaths {
            explicit: None,
            rust_home: None,
            legacy_home: None,
            executable_dir: root.path().into(),
            config_dir: root.path().join("config"),
        },
        catalog: Some(catalog),
        discovery: kode_lsp::discovery::DiscoveryOptions {
            platform: kode_lsp::catalog::Platform::detect(),
            install_root: root.path().join("install"),
            search_path: vec![],
            configured_commands: Default::default(),
        },
    })
    .unwrap();
    until(&mut app, |app| {
        app.state.lsp_settings.statuses.len() == 1 && app.state.lsp_settings.busy.is_none()
    })
    .await;
    app.state
        .work
        .push(AppWork::LspSettings(SettingsAction::Install(
            "fixture".into(),
        )));
    until(&mut app, |app| {
        app.state.lsp_settings.busy.is_none() && app.state.lsp_settings.message.contains("503")
    })
    .await;
    server.await.unwrap();
    assert!(app.state.lsp_settings.message.contains("ACTIONABLE-END"));
    assert!(app.state.lsp_settings.message.len() > 4096);
    app.state.tabs.selected = kode_tui::app::panel::SETTINGS;
    app.state.focus.focused = Some(app.state.lsp_settings.id);
    let before = app.state.focus.focused;
    let area = Rect::new(0, 0, 60, 24);
    app.state
        .render(area, &mut Buffer::empty(area), &Theme::compiled())
        .unwrap();
    app.state.dispatch(&key(Key::Function(4))).unwrap();
    until(&mut app, |app| app.state.diagnostic.is_some()).await;
    assert!(
        app.state
            .diagnostic
            .as_ref()
            .unwrap()
            .title
            .contains("Install fixture"),
        "actual diagnostic title: {}",
        app.state.diagnostic.as_ref().unwrap().title
    );
    let full = app.state.diagnostic.as_ref().unwrap().text.clone();
    app.state
        .render(area, &mut Buffer::empty(area), &Theme::compiled())
        .unwrap();
    app.state.dispatch(&key(Key::End)).unwrap();
    app.state
        .render(area, &mut Buffer::empty(area), &Theme::compiled())
        .unwrap();
    assert!(app.state.diagnostic.as_ref().unwrap().panel.scroll > 0);
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('c'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert_eq!(clipboard.local_text(), full);
    app.state.dispatch(&key(Key::Escape)).unwrap();
    assert_eq!(app.state.focus.focused, before);
    assert!(!root.path().join("install/fixture").exists());
    assert!(!root.path().join("install/.kode-rs-install.lock").exists());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
