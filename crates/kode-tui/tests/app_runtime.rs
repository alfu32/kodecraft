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
            "timeout: {}",
            app.state.notice
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
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    app.pump().unwrap();
    app.switch_workspace(second.path().into()).await.unwrap();
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    assert_eq!(app.state.project.root, second.path());
    assert_eq!(app.state.documents.ids().count(), 0);
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
