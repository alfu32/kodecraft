use git2::Repository;
use kode_core::{DocumentOrigin, WorkspaceGeneration};
use kode_git::write::{WriteJob, WriteRequest};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock},
    paths::ResourcePaths,
};
use kode_tui::{
    app::{
        runtime::{AppRuntime, lsp::LspConfiguration},
        workspace::prepare_workspace,
    },
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_millis(&self) -> u64 {
        0
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
fn ctrl_tab() -> InputEvent {
    key(Key::Tab, true)
}
fn alt3() -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character('3'),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn write(root: &Path, request: WriteRequest) {
    WriteJob {
        root: root.into(),
        request,
    }
    .run(&CancellationToken::new())
    .unwrap();
}
fn init(root: &Path) -> Repository {
    write(root, WriteRequest::Init);
    let repo = Repository::open(root).unwrap();
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Synthetic Author").unwrap();
    config
        .set_str("user.email", "synthetic@example.invalid")
        .unwrap();
    config.set_bool("core.autocrlf", false).unwrap();
    config
        .set_str(
            "core.excludesfile",
            root.join(".git/no-global-excludes").to_str().unwrap(),
        )
        .unwrap();
    config
        .set_str(
            "core.attributesfile",
            root.join(".git/no-global-attributes").to_str().unwrap(),
        )
        .unwrap();
    fs::write(root.join(".gitignore"), ".kode-rs*\n").unwrap();
    fs::write(root.join("Note.kt"), "first\n").unwrap();
    write(
        root,
        WriteRequest::Stage(vec![".gitignore".into(), "Note.kt".into()]),
    );
    write(
        root,
        WriteRequest::Commit {
            message: "baseline".into(),
        },
    );
    repo
}
fn make(root: &Path, file: Option<&str>) -> AppRuntime {
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        file.map(|file| root.join(file)),
        &CancellationToken::new(),
    )
    .unwrap();
    AppRuntime::with_services(
        prepared,
        Arc::new(ClipboardService::internal()),
        Arc::new(ZeroClock),
    )
    .unwrap()
}
fn paint(app: &mut AppRuntime) {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
    app.state
        .render(buffer.area, &mut buffer, &Theme::compiled())
        .unwrap();
}
async fn until(app: &mut AppRuntime, predicate: impl Fn(&AppRuntime) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        app.pump().unwrap();
        paint(app);
        if predicate(app) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "notice={}; Git={}; read={:?}; write={:?}; error={:?}",
            app.state.notice,
            app.state.git.message,
            app.state.git.read.busy,
            app.state.git.write_busy,
            app.state.git.read.error
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
async fn ready(app: &mut AppRuntime) {
    until(app, |app| {
        app.state.git.read.snapshot.is_some()
            && app.state.git.read.busy.is_none()
            && app.state.git.write_busy.is_none()
    })
    .await;
}
fn draft(app: &AppRuntime) -> String {
    let id = app.state.git.commit_editor.as_ref().unwrap().document_id();
    app.state
        .documents
        .document(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string()
}
fn status_focus(app: &mut AppRuntime) {
    app.state.dispatch(&alt3()).unwrap();
    for _ in 0..3 {
        if app.state.focus.focused == Some(app.state.git.id()) {
            return;
        }
        app.state.dispatch(&ctrl_tab()).unwrap();
    }
    assert_eq!(app.state.focus.focused, Some(app.state.git.id()));
}
fn choose_branch(app: &mut AppRuntime, branch: &str) {
    status_focus(app);
    let index = app
        .state
        .git
        .read
        .snapshot
        .as_ref()
        .unwrap()
        .branches
        .entries
        .iter()
        .position(|entry| entry.name == branch)
        .unwrap();
    app.state
        .dispatch(&key(Key::Character('b'), false))
        .unwrap();
    paint(app);
    app.state.dispatch(&key(Key::Home, false)).unwrap();
    for _ in 0..index {
        app.state.dispatch(&key(Key::Down, false)).unwrap();
    }
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
}
#[tokio::test]
async fn real_git_panel_stages_commits_uses_shared_clipboard_local_completion_and_history_tag_actions()
 {
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project");
    fs::create_dir(&root).unwrap();
    let repo = init(&root);
    let baseline = repo.head().unwrap().target().unwrap();
    fs::write(root.join("Note.kt"), "changed\n").unwrap();
    let mut app = make(&root, Some("Note.kt"));
    // Keep this optional service configured, as in the CLI, without discovering
    // or launching any vendor server from the developer's PATH.
    let catalog = base.path().join("catalog.json");
    fs::write(&catalog, "{\"servers\":[]}").unwrap();
    app.configure_lsp(LspConfiguration {
        paths: ResourcePaths {
            explicit: None,
            rust_home: None,
            legacy_home: None,
            executable_dir: base.path().into(),
            config_dir: base.path().join("config"),
        },
        catalog: Some(catalog),
        discovery: kode_lsp::discovery::DiscoveryOptions {
            platform: kode_lsp::catalog::Platform::detect(),
            install_root: base.path().join("lsp"),
            search_path: vec![],
            configured_commands: Default::default(),
        },
    })
    .unwrap();
    ready(&mut app).await;
    until(&mut app, |app| app.state.editor.is_some()).await;
    status_focus(&mut app);
    let main = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .git
            .read
            .snapshot
            .as_ref()
            .unwrap()
            .status
            .entries
            .len(),
        1
    );
    app.state
        .dispatch(&key(Key::Character(' '), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app
                .state
                .git
                .read
                .snapshot
                .as_ref()
                .unwrap()
                .status
                .entries
                .iter()
                .any(|entry| entry.staged())
    })
    .await;
    app.state.dispatch(&key(Key::Tab, false)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("commit fixture\ncomm".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Character(' '), true)).unwrap();
    until(&mut app, |app| {
        !app.state
            .git
            .commit_editor
            .as_ref()
            .unwrap()
            .completion
            .entries
            .is_empty()
    })
    .await;
    assert_eq!(
        app.state
            .git
            .commit_editor
            .as_ref()
            .unwrap()
            .completion
            .entries[0]
            .label,
        "commit"
    );
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    let message = draft(&app);
    assert_eq!(message, "commit fixture\ncommit");
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('c'), true)).unwrap();
    app.pump().unwrap();
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('v'), true)).unwrap();
    until(&mut app, |app| {
        app.state
            .documents
            .document(main)
            .unwrap()
            .snapshot()
            .text()
            .as_ref()
            == message
    })
    .await;
    assert!(app.state.documents.document(main).unwrap().is_dirty());
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"changed\n");
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    let focus = app.state.focus.focused;
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app
                .state
                .git
                .read
                .snapshot
                .as_ref()
                .unwrap()
                .commits
                .entries
                .len()
                == 2
    })
    .await;
    let committed = repo.head().unwrap().target().unwrap();
    assert_ne!(committed, baseline);
    assert_eq!(
        repo.find_commit(committed).unwrap().message_bytes(),
        message.as_bytes()
    );
    assert!(draft(&app).is_empty());
    assert_eq!(app.state.focus.focused, focus);
    assert_eq!(
        app.state
            .documents
            .document(main)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        message
    );
    app.state.dispatch(&ctrl_tab()).unwrap();
    app.state
        .dispatch(&key(Key::Character('t'), false))
        .unwrap();
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("release/v1.0".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app
                .state
                .git
                .read
                .snapshot
                .as_ref()
                .unwrap()
                .tags
                .entries
                .len()
                == 1
    })
    .await;
    assert_eq!(
        repo.find_reference("refs/tags/release/v1.0")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        committed
    );
    app.state.dispatch(&key(Key::Down, false)).unwrap();
    assert_eq!(
        app.state.git.selected_commit.as_deref(),
        Some(baseline.to_string().as_str())
    );
    app.state
        .dispatch(&key(Key::Character('T'), false))
        .unwrap();
    paint(&mut app);
    app.state
        .dispatch(&InputEvent::Paste("release/v1.0".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app
                .state
                .git
                .read
                .snapshot
                .as_ref()
                .unwrap()
                .tags
                .entries
                .iter()
                .any(|tag| tag.commit.as_deref() == Some(baseline.to_string().as_str()))
    })
    .await;
    assert_eq!(repo.head().unwrap().target(), Some(committed));
    assert_eq!(
        repo.find_reference("refs/tags/release/v1.0")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        baseline
    );
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.git.commit_files(&baseline.to_string()).is_some()
    })
    .await;
    app.state.dispatch(&key(Key::Down, false)).unwrap();
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| app.state.git.latest_diff.is_some()).await;
    let diff = app.state.git.latest_diff.as_ref().unwrap().contents();
    assert_eq!(
        diff.revision.as_deref(),
        Some(baseline.to_string().as_str())
    );
    assert_eq!(diff.path, Path::new(".gitignore"));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"changed\n");
}
#[tokio::test]
async fn checkout_reloads_clean_files_keeps_sidebar_focus_and_refuses_dirty_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    let first = repo.head().unwrap().target().unwrap();
    let original = String::from_utf8(repo.head().unwrap().name_bytes().to_vec())
        .unwrap()
        .trim_start_matches("refs/heads/")
        .to_owned();
    repo.branch("older", &repo.find_commit(first).unwrap(), false)
        .unwrap();
    fs::write(root.join("Note.kt"), "second\n").unwrap();
    write(root, WriteRequest::Stage(vec!["Note.kt".into()]));
    write(
        root,
        WriteRequest::Commit {
            message: "second".into(),
        },
    );
    let mut app = make(root, Some("Note.kt"));
    ready(&mut app).await;
    until(&mut app, |app| app.state.editor.is_some()).await;
    let old = app.state.editor.as_ref().unwrap().document_id();
    choose_branch(&mut app, "older");
    let focus = app.state.focus.focused;
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app.state.editor.as_ref().is_some_and(|view| {
                app.state
                    .documents
                    .document(view.document_id())
                    .unwrap()
                    .snapshot()
                    .text()
                    .as_ref()
                    == "first\n"
            })
    })
    .await;
    assert_eq!(repo.head().unwrap().target(), Some(first));
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"first\n");
    assert_ne!(app.state.editor.as_ref().unwrap().document_id(), old);
    assert!(app.state.documents.document(old).is_err());
    assert_eq!(app.state.focus.focused, focus);
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    choose_branch(&mut app, &original);
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && app.state.git.message.contains("dirty editor buffers")
    })
    .await;
    assert_eq!(repo.head().unwrap().target(), Some(first));
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "draft first\n"
    );
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"first\n");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn commit_acknowledgement_preserves_newer_typing_and_quit_drains_the_accepted_commit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("Note.kt"), "staged\n").unwrap();
    write(root, WriteRequest::Stage(vec!["Note.kt".into()]));
    let previous = repo.head().unwrap().target().unwrap();
    let mut app = make(root, None);
    ready(&mut app).await;
    status_focus(&mut app);
    app.state.dispatch(&key(Key::Tab, false)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("message snapshot".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    app.pump().unwrap();
    assert!(app.state.git.write_busy.is_some());
    app.state
        .dispatch(&InputEvent::Paste(" — newer draft".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Character('q'), true)).unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let oid = repo.head().unwrap().target().unwrap();
    assert_ne!(oid, previous);
    assert_eq!(
        repo.find_commit(oid).unwrap().message_bytes(),
        b"message snapshot"
    );
    assert_eq!(draft(&app), "message snapshot — newer draft");
    assert!(app.state.git.message.contains("Current message kept"));
    assert!(!repo.path().join("index.lock").exists());
    assert!(app.state.documents.ids().all(|id| matches!(
        app.state.documents.document(id).unwrap().origin(),
        DocumentOrigin::Virtual { .. }
    )));
}

#[tokio::test]
async fn initialize_and_commit_errors_keep_the_draft_and_external_index_lock() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut app = make(root, None);
    until(&mut app, |app| app.state.git.repo_available == Some(false)).await;
    status_focus(&mut app);
    app.state
        .dispatch(&key(Key::Character('i'), false))
        .unwrap();
    ready(&mut app).await;
    let repo = Repository::open(root).unwrap();
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "").unwrap();
    config.set_str("user.email", "").unwrap();
    config.set_bool("core.autocrlf", false).unwrap();
    fs::write(root.join(".gitignore"), ".kode-rs*\n").unwrap();
    fs::write(root.join("file.txt"), "fixture\n").unwrap();
    app.state.git.request_refresh();
    app.pump().unwrap();
    ready(&mut app).await;
    app.state
        .dispatch(&key(Key::Character('a'), false))
        .unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.git.read.busy.is_none()
            && app
                .state
                .git
                .read
                .snapshot
                .as_ref()
                .unwrap()
                .status
                .entries
                .iter()
                .any(|entry| entry.staged())
    })
    .await;
    app.state.dispatch(&key(Key::Tab, false)).unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && app.state.git.message.contains("message")
    })
    .await;
    assert!(repo.head().is_err());
    ready(&mut app).await;
    app.state
        .dispatch(&InputEvent::Paste("retained draft".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && app.state.git.message.contains("identity")
    })
    .await;
    assert_eq!(draft(&app), "retained draft");
    assert!(repo.head().is_err());
    config.set_str("user.name", "Synthetic Author").unwrap();
    config
        .set_str("user.email", "synthetic@example.invalid")
        .unwrap();
    ready(&mut app).await;
    let lock = repo.path().join("index.lock");
    fs::write(&lock, b"external writer").unwrap();
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && app.state.git.message.contains("lock")
    })
    .await;
    assert_eq!(fs::read(&lock).unwrap(), b"external writer");
    assert_eq!(draft(&app), "retained draft");
    assert!(repo.head().is_err());
    fs::remove_file(lock).unwrap();
    ready(&mut app).await;
    app.state.dispatch(&key(Key::Enter, true)).unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && draft(app).is_empty()
    })
    .await;
    assert_eq!(
        repo.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .message_bytes(),
        b"retained draft"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn checkout_requeues_the_latest_explicit_open_and_refreshes_cached_binary_bytes() {
    use kode_tui::app::{state::OpenDestination, workspace::OpenRequest};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("data.bin"), [0u8, 1, 2]).unwrap();
    write(root, WriteRequest::Stage(vec!["data.bin".into()]));
    write(
        root,
        WriteRequest::Commit {
            message: "binary baseline".into(),
        },
    );
    let older = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("older", &older, false).unwrap();
    fs::write(root.join("Note.kt"), "newer\n").unwrap();
    fs::write(root.join("data.bin"), [0u8, 9, 8]).unwrap();
    write(
        root,
        WriteRequest::Stage(vec!["Note.kt".into(), "data.bin".into()]),
    );
    write(
        root,
        WriteRequest::Commit {
            message: "newer".into(),
        },
    );
    let original = repo.head().unwrap().shorthand().unwrap().to_owned();
    let mut app = make(root, Some("data.bin"));
    ready(&mut app).await;
    until(&mut app, |app| app.state.active_hex.is_some()).await;
    let path = root.join("data.bin");
    let old_view = app.state.hexes[&path].id;
    choose_branch(&mut app, "older");
    // This user request arrives while checkout is accepted but before its ack.
    // It must supersede automatic reopening of the previously active binary.
    app.state
        .open(
            OpenRequest {
                path: root.join("Note.kt"),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.editor.as_ref().is_some_and(|view| {
                app.state
                    .documents
                    .document(view.document_id())
                    .unwrap()
                    .snapshot()
                    .text()
                    .as_ref()
                    == "first\n"
            })
    })
    .await;
    assert_eq!(app.state.focus.focused, app.state.viewer_id());
    assert!(!app.state.hexes.contains_key(&path));
    app.state
        .open(
            OpenRequest {
                path: path.clone(),
                saved: None,
                navigation: None,
            },
            OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| app.state.active_hex.as_ref() == Some(&path)).await;
    assert_ne!(app.state.hexes[&path].id, old_view);
    assert_eq!(app.state.hexes[&path].buffer.bytes(), [0u8, 1, 2]);
    assert_eq!(fs::read(&path).unwrap(), [0u8, 1, 2]);
    ready(&mut app).await;
    choose_branch(&mut app, &original);
    // Opening a search preview during an accepted checkout would attach stale
    // bytes; the shell explicitly asks the user to wait for its acknowledgement.
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('F'),
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.search.is_none());
    assert!(app.state.notice.contains("Wait for branch checkout"));
    let sidebar = app.state.focus.focused;
    until(&mut app, |app| {
        app.state.git.write_busy.is_none()
            && app.state.active_hex.as_ref() == Some(&path)
            && app.state.hexes[&path].buffer.bytes() == [0u8, 9, 8]
    })
    .await;
    assert_eq!(app.state.focus.focused, sidebar);
    app.state.dispatch(&key(Key::Function(6), false)).unwrap();
    app.state.dispatch(&key(Key::Delete, false)).unwrap();
    assert!(app.state.hexes[&path].modified);
    ready(&mut app).await;
    choose_branch(&mut app, "older");
    until(&mut app, |app| {
        app.state.git.write_busy.is_none() && app.state.git.message.contains("dirty editor buffers")
    })
    .await;
    assert_eq!(repo.head().unwrap().shorthand().unwrap(), original);
    assert_eq!(fs::read(&path).unwrap(), [0u8, 9, 8]);
    assert_eq!(app.state.hexes[&path].buffer.bytes(), [9u8, 8]);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn actual_diff_preview_restores_only_the_clicked_captured_chunk_and_returns_to_the_clean_editor()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("Note.kt"), "first\ncontext\ncontext2\nlast\n").unwrap();
    write(root, WriteRequest::Stage(vec!["Note.kt".into()]));
    write(
        root,
        WriteRequest::Commit {
            message: "diff base".into(),
        },
    );
    let changed = "FIRST\ncontext\ncontext2\nLAST\n";
    fs::write(root.join("Note.kt"), changed).unwrap();
    let index = fs::read(repo.path().join("index")).unwrap();
    let mut app = make(root, Some("Note.kt"));
    ready(&mut app).await;
    until(&mut app, |app| app.state.editor.is_some()).await;
    let original = app.state.editor.as_ref().unwrap().document_id();
    status_focus(&mut app);
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.editor.is_some())
    })
    .await;
    let diff = app.state.diff.as_ref().unwrap();
    let id = diff.editor.as_ref().unwrap().document_id();
    assert!(app.state.documents.document(id).unwrap().is_read_only());
    let preview = app
        .state
        .documents
        .document(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    app.state
        .dispatch(&InputEvent::Paste("forbidden".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Delete, false)).unwrap();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        preview
    );
    app.state.diff.as_mut().unwrap().notice.clear();
    app.state.dispatch(&key(Key::Character('a'), true)).unwrap();
    app.state.dispatch(&key(Key::Character('c'), true)).unwrap();
    app.pump().unwrap();
    app.state.dispatch(&key(Key::Character('v'), true)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.notice.contains("read-only"))
    })
    .await;
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        preview
    );
    app.state.dispatch(&key(Key::Home, true)).unwrap();
    let area = app.state.diff.as_ref().unwrap().area;
    let side = area.width.saturating_sub(3) / 2;
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: area.x + side,
            y: area.y + 1,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Default::default(),
        }))
        .unwrap();
    let expected = "first\ncontext\ncontext2\nLAST\n";
    until(&mut app, |app| {
        app.state.diff.as_ref().is_some_and(|diff| {
            diff.restore_busy.is_none()
                && diff.plan.contents().new.bytes == expected.as_bytes()
                && diff.editor.is_some()
        }) && app.state.editor.is_some()
    })
    .await;
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), expected.as_bytes());
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
    assert!(app.state.documents.document(original).is_err());
    assert_eq!(app.state.focus.focused, app.state.viewer_id());
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('1'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.diff.is_none());
    let view = app.state.editor.as_ref().unwrap();
    assert_eq!(
        app.state
            .documents
            .document(view.document_id())
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        expected
    );
    ready(&mut app).await;
    status_focus(&mut app);
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.editor.is_some())
    })
    .await;
    for _ in 0..3 {
        app.state.dispatch(&key(Key::Down, false)).unwrap();
    }
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    assert!(
        app.state.diff.as_ref().unwrap().restore_busy.is_some(),
        "notice={}; cursor={:?}; focus={:?}; id={:?}; text={:?}",
        app.state.diff.as_ref().unwrap().notice,
        app.state
            .diff
            .as_ref()
            .unwrap()
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .cursor(),
        app.state.focus.focused,
        app.state.diff.as_ref().unwrap().id(),
        app.state
            .documents
            .document(
                app.state
                    .diff
                    .as_ref()
                    .unwrap()
                    .editor
                    .as_ref()
                    .unwrap()
                    .document_id()
            )
            .unwrap()
            .snapshot()
            .text()
    );
    app.state.dispatch(&key(Key::Character('q'), true)).unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert_eq!(
        fs::read(root.join("Note.kt")).unwrap(),
        b"first\ncontext\ncontext2\nlast\n"
    );
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), index);
}

#[tokio::test]
async fn diff_restore_reports_dirty_buffer_and_external_change_conflicts_without_overwrite_or_focus_theft()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init(root);
    fs::write(root.join("Note.kt"), "changed\n").unwrap();
    let mut app = make(root, Some("Note.kt"));
    ready(&mut app).await;
    until(&mut app, |app| app.state.editor.is_some()).await;
    app.state
        .dispatch(&InputEvent::Paste("draft ".into()))
        .unwrap();
    status_focus(&mut app);
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.editor.is_some())
    })
    .await;
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state.diff.as_ref().is_some_and(|diff| {
            diff.restore_busy.is_none() && diff.notice.contains("dirty open buffer")
        })
    })
    .await;
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"changed\n");
    assert!(
        app.state
            .documents
            .document(app.state.editor.as_ref().unwrap().document_id())
            .unwrap()
            .is_dirty()
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    // New isolated session avoids intentionally persisted dirty recovery bytes.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init(root);
    fs::write(root.join("Note.kt"), "changed\n").unwrap();
    let mut app = make(root, None);
    ready(&mut app).await;
    status_focus(&mut app);
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.editor.is_some())
    })
    .await;
    fs::write(root.join("Note.kt"), "external\n").unwrap();
    let focus = app.state.focus.focused;
    app.state.dispatch(&key(Key::Enter, false)).unwrap();
    until(&mut app, |app| {
        app.state
            .diff
            .as_ref()
            .is_some_and(|diff| diff.restore_busy.is_none() && !diff.notice.is_empty())
    })
    .await;
    assert_eq!(fs::read(root.join("Note.kt")).unwrap(), b"external\n");
    assert_eq!(app.state.focus.focused, focus);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
