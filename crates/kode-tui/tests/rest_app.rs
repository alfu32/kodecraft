use kode_core::{DocumentOrigin, WorkspaceGeneration};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_rest::model::{EnvironmentValue, NodePath};
use kode_tui::{
    app::{runtime::AppRuntime, workspace::prepare_workspace},
    input::terminal::{InputEvent, Key, KeyInput, Modifiers},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn ctrl(ch: char) -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character(ch),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn alt7() -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character('7'),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn make(root: &Path, generation: u64) -> (AppRuntime, Arc<ClipboardService>) {
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(generation),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let clipboard = Arc::new(ClipboardService::internal());
    let app = AppRuntime::with_services(
        prepared,
        clipboard.clone(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    (app, clipboard)
}
fn render(app: &mut AppRuntime) {
    let area = Rect::new(0, 0, 100, 34);
    let mut buffer = Buffer::empty(area);
    app.state
        .render(area, &mut buffer, &Theme::compiled())
        .unwrap();
}
async fn until(app: &mut AppRuntime, predicate: impl Fn(&AppRuntime) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            app.pump().unwrap();
            render(app);
            if predicate(app) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
fn submit(app: &mut AppRuntime, text: &str) {
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state.dispatch(&InputEvent::Paste(text.into())).unwrap();
    app.state.dispatch(&key(Key::Enter)).unwrap();
}
#[tokio::test]
async fn original_environment_editor_typing_clipboard_native_search_and_metadata_use_the_shared_editor()
 {
    let root = tempfile::tempdir().unwrap();
    let (mut app, clipboard) = make(root.path(), 1);
    app.state.dispatch(&alt7()).unwrap();
    assert_eq!(app.state.tabs.selected, 6);
    render(&mut app);
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&key(Key::Character('n'))).unwrap();
    submit(&mut app, "Development");
    let environment = app.state.rest.model.environments()[0].id.clone();
    until(&mut app, |app| {
        app.state.rest.environment_document(&environment).is_some()
    })
    .await;
    let document = app.state.rest.environment_document(&environment).unwrap();
    assert_eq!(app.state.editor.as_ref().unwrap().document_id(), document);
    app.state
        .dispatch(&InputEvent::Paste("host=example.test".into()))
        .unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .model
            .environment(&environment)
            .unwrap()
            .values
            .len()
            == 1
    })
    .await;
    for event in [
        key(Key::End),
        key(Key::Enter),
        key(Key::Character('n')),
        key(Key::Character('e')),
        key(Key::Character('w')),
        key(Key::Character('=')),
        key(Key::Character('1')),
    ] {
        app.state.dispatch(&event).unwrap();
    }
    until(&mut app, |app| {
        app.state
            .rest
            .model
            .environment(&environment)
            .unwrap()
            .values
            .last()
            .is_some_and(|value| value.key == "new" && value.text() == "1")
    })
    .await;
    clipboard.remember("pasted=ok").unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state.dispatch(&ctrl('v')).unwrap();
    until(&mut app, |app| {
        app.state
            .documents
            .document(document)
            .unwrap()
            .snapshot()
            .text()
            .as_ref()
            == "pasted=ok"
            && app
                .state
                .rest
                .model
                .environment(&environment)
                .unwrap()
                .values
                .len()
                == 1
            && app
                .state
                .rest
                .model
                .environment(&environment)
                .unwrap()
                .values[0]
                .key
                == "pasted"
    })
    .await;
    app.state.dispatch(&ctrl('f')).unwrap();
    assert!(app.state.editor.as_ref().unwrap().search.visible);
    app.state.dispatch(&key(Key::Escape)).unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn raw_environment_drafts_selected_path_and_divider_survive_restart_without_runtime_data() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    let environment = app.state.rest.model.create_environment("Saved");
    app.state.rest.model.set_environment_values(
        &environment,
        vec![EnvironmentValue {
            key: "object".into(),
            value: json!({"nested":7}),
            secret: true,
            enabled: false,
            extra: serde_json::Map::from_iter([("x-future".into(), json!(3))]),
            ..Default::default()
        }],
    );
    let folder = app
        .state
        .rest
        .model
        .add_folder(&NodePath::default(), "Selected")
        .unwrap();
    app.state.rest.model.select(&folder);
    app.state.rest.model.set_environment_panel_height(11);
    app.state.open_rest_environment(&environment, None).unwrap();
    until(&mut app, |app| {
        app.state.rest.environment_document(&environment).is_some()
    })
    .await;
    let id = app.state.rest.environment_document(&environment).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    let draft = "# comment\nobject={\"nested\":7}\nnew=1\nunfinished";
    app.state
        .dispatch(&InputEvent::Paste(draft.into()))
        .unwrap();
    app.flush_before_leave().await.unwrap();
    let saved = app.state.store.load().unwrap().session;
    assert_eq!(saved.rest_api["ui"]["environmentPanelHeight"], 11);
    assert_eq!(saved.rest_api["ui"]["selectedPath"], folder.encode());
    assert!(saved.rest_api.get("cookies").is_none());
    assert!(saved.rest_api.get("responses").is_none());
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        draft
    );
    app.quiesce().await.unwrap();
    let (mut restored, _) = make(root.path(), 2);
    until(&mut restored, |app| {
        app.state.rest.environment_document(&environment).is_some()
    })
    .await;
    let id = restored
        .state
        .rest
        .environment_document(&environment)
        .unwrap();
    assert_eq!(
        restored
            .state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        draft
    );
    let value = &restored
        .state
        .rest
        .model
        .environment(&environment)
        .unwrap()
        .values[0];
    assert_eq!(value.value, json!({"nested":7}));
    assert!(value.secret);
    assert!(!value.enabled);
    assert_eq!(value.extra["x-future"], 3);
    assert!(
        matches!(restored.state.documents.document(id).unwrap().origin(),DocumentOrigin::Virtual{owner,..}if owner=="rest-environment")
    );
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}
#[tokio::test]
async fn file_import_inspection_and_guarded_export_are_real_worker_actions_and_preserve_unknown_fields()
 {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    let original = json!({"info":{"name":"Imported"},"item":[{"name":"Call","request":{"method":"GET","url":"https://example.invalid"},"response":[{"body":"saved example"}]}],"x-future":{"keep":true}});
    std::fs::write(root.path().join("collection.json"), original.to_string()).unwrap();
    app.state.dispatch(&alt7()).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Character('i'))).unwrap();
    submit(&mut app, "collection.json");
    until(&mut app, |app| {
        app.state.rest.model.collection_name() == "Imported"
    })
    .await;
    assert_eq!(app.state.rest.model.collection(), &original);
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app,|app|app.state.editor.as_ref().is_some_and(|view|matches!(app.state.documents.document(view.document_id()).unwrap().origin(),DocumentOrigin::Virtual{owner,..}if owner=="rest"))).await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert!(app.state.documents.document(id).unwrap().is_read_only());
    let snapshot = app.state.documents.document(id).unwrap().snapshot().text();
    assert!(
        app.state
            .dispatch(&InputEvent::Paste("cannot mutate".into()))
            .is_err()
    );
    assert_eq!(
        app.state.documents.document(id).unwrap().snapshot().text(),
        snapshot
    );
    app.state.dispatch(&alt7()).unwrap();
    app.state.dispatch(&key(Key::Character('e'))).unwrap();
    submit(&mut app, "exports/collection.json");
    let export = root.path().join("exports/collection.json");
    until(&mut app, |_| export.exists()).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&export).unwrap()).unwrap(),
        original
    );
    until(&mut app, |app| app.state.rest.exports.contains_key(&export)).await;
    std::fs::write(&export, "external edit").unwrap();
    app.state.dispatch(&key(Key::Character('e'))).unwrap();
    submit(&mut app, "exports/collection.json");
    until(&mut app, |app| {
        app.state
            .rest
            .sidebar
            .message
            .contains("file changed on disk")
    })
    .await;
    assert_eq!(std::fs::read_to_string(&export).unwrap(), "external edit");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn environment_copy_uses_current_buffer_even_before_semantic_worker_publication() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    let source = app.state.rest.model.create_environment("Source");
    app.state.open_rest_environment(&source, None).unwrap();
    until(&mut app, |app| {
        app.state.rest.environment_document(&source).is_some()
    })
    .await;
    app.state
        .dispatch(&InputEvent::Paste("latest=buffer".into()))
        .unwrap();
    // No pump between editing and submitting copy: the model parse is pending.
    app.state
        .rest
        .sidebar
        .perform(
            kode_tui::panels::rest::sidebar::Action::CopyEnvironment(source.clone()),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    submit(&mut app, "Copy");
    let copy = app
        .state
        .rest
        .model
        .environments()
        .last()
        .unwrap()
        .id
        .clone();
    until(&mut app, |app| {
        app.state.rest.environment_document(&copy).is_some()
    })
    .await;
    let id = app.state.rest.environment_document(&copy).unwrap();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "latest=buffer"
    );
    assert_ne!(copy, source);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn copying_an_unparsed_buffer_then_immediately_quitting_retains_both_drafts() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    let source = app.state.rest.model.create_environment("Source");
    app.state.open_rest_environment(&source, None).unwrap();
    until(&mut app, |app| {
        app.state.rest.environment_document(&source).is_some()
    })
    .await;
    let draft = "# keep comment\nlatest=buffer\nunfinished";
    app.state
        .dispatch(&InputEvent::Paste(draft.into()))
        .unwrap();
    app.state
        .rest
        .sidebar
        .perform(
            kode_tui::panels::rest::sidebar::Action::CopyEnvironment(source.clone()),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    submit(&mut app, "Copy");
    let copy = app
        .state
        .rest
        .model
        .environments()
        .last()
        .unwrap()
        .id
        .clone();
    assert!(app.state.rest.environment_document(&copy).is_none());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let (mut restored, _) = make(root.path(), 2);
    for environment in [&source, &copy] {
        restored
            .state
            .open_rest_environment(environment, None)
            .unwrap();
        until(&mut restored, |app| {
            app.state.rest.environment_document(environment).is_some()
        })
        .await;
        let id = restored
            .state
            .rest
            .environment_document(environment)
            .unwrap();
        assert_eq!(
            restored
                .state
                .documents
                .document(id)
                .unwrap()
                .snapshot()
                .text()
                .as_ref(),
            draft
        );
        assert_eq!(
            restored
                .state
                .rest
                .model
                .environment(environment)
                .unwrap()
                .values[0]
                .text(),
            "buffer"
        );
    }
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
}

#[tokio::test]
async fn explicit_export_filename_replaces_an_existing_target_without_an_extra_prompt() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("selected.json");
    std::fs::write(&path, "previous contents").unwrap();
    let (mut app, _) = make(root.path(), 1);
    let expected = app.state.rest.model.export_collection();
    app.state.dispatch(&alt7()).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Character('e'))).unwrap();
    submit(&mut app, "selected.json");
    until(&mut app, |app| app.state.rest.exports.contains_key(&path)).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(),
        *expected
    );
    assert!(!app.state.rest.sidebar.has_modal());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

async fn server() -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (hits, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let mut gate = Some(gate);
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0; 1];
                if socket.read_exact(&mut byte).await.is_err() {
                    return;
                }
                request.extend_from_slice(&byte);
                assert!(request.len() < 64 * 1024);
            }
            let line = String::from_utf8(request)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_string();
            hits.send(line).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
            if let Some(gate) = gate.take() {
                let _ = gate.await;
            }
            let _ = socket.write_all(b"d").await;
        }
    });
    (url, receiver, release, worker)
}
async fn next_hit(
    app: &mut AppRuntime,
    hits: &mut tokio::sync::mpsc::UnboundedReceiver<String>,
) -> String {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            app.pump().unwrap();
            if let Ok(hit) = hits.try_recv() {
                return hit;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn sidebar_run_uses_current_environment_and_depth_first_selection_with_identity_across_moves()
{
    use kode_tui::panels::rest::sidebar::Action;
    let (url, mut hits, release, server) = server().await;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{"name":"Run"},"item":[
        {"name":"Selected","item":[
            {"id":"duplicate","name":"First","request":{"method":"GET","url":format!("{url}/{{{{path}}}}/first")}},
            {"name":"Nested","item":[{"id":"duplicate","name":"Second","request":{"method":"GET","url":format!("{url}/second")}}]}
        ]},
        {"id":"duplicate","name":"Outside","request":{"url":"{{must-not-resolve}}"}}
    ]})).unwrap();
    let environment = app.state.rest.model.create_environment("Current");
    app.state
        .rest
        .model
        .activate_environment(Some(&environment));
    app.state.open_rest_environment(&environment, None).unwrap();
    until(&mut app, |app| {
        app.state.rest.environment_document(&environment).is_some()
    })
    .await;
    app.state
        .dispatch(&InputEvent::Paste("path=current-buffer".into()))
        .unwrap();
    app.state.dispatch(&alt7()).unwrap();
    app.state.rest.sidebar.cursor = NodePath(vec![0]);
    render(&mut app);
    app.state.dispatch(&key(Key::Character('r'))).unwrap();
    assert_eq!(
        next_hit(&mut app, &mut hits).await,
        "GET /current-buffer/first HTTP/1.1"
    );
    let original_key = app.state.rest.run.keys[&NodePath(vec![0, 0])].clone();
    app.state
        .rest
        .sidebar
        .perform(
            Action::MoveDown(NodePath(vec![0])),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    app.state.drain_ui().unwrap();
    assert_eq!(app.state.rest.run.keys[&NodePath(vec![1, 0])], original_key);
    release.send(()).unwrap();
    assert_eq!(next_hit(&mut app, &mut hits).await, "GET /second HTTP/1.1");
    until(&mut app, |app| app.state.rest.run.summary.is_some()).await;
    let summary = app.state.rest.run.summary.as_ref().unwrap();
    assert!(!summary.cancelled);
    assert_eq!(
        summary
            .outcomes
            .iter()
            .map(|outcome| outcome.item.name.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(summary.outcomes[0].item.id, original_key);
    assert_ne!(summary.outcomes[0].item.id, summary.outcomes[1].item.id);
    assert_eq!(app.state.rest.run.responses.len(), 2);
    assert_eq!(app.state.rest.run.responses[0].key.item_id, original_key);
    assert_eq!(app.state.rest.run.responses[0].body.as_ref(), b"abcd");
    assert!(hits.try_recv().is_err());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn run_again_cancels_the_current_request_and_never_sends_the_next_one() {
    let (url, mut hits, _release, server) = server().await;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state
        .rest
        .model
        .replace_collection(json!({"info":{},"item":[
            {"name":"First","request":{"url":format!("{url}/first")}},
            {"name":"Never","request":{"url":format!("{url}/never")}}
        ]}))
        .unwrap();
    app.state.dispatch(&alt7()).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Character('r'))).unwrap();
    assert_eq!(next_hit(&mut app, &mut hits).await, "GET /first HTTP/1.1");
    app.state.dispatch(&key(Key::Character('r'))).unwrap();
    until(&mut app, |app| app.state.rest.run.summary.is_some()).await;
    let summary = app.state.rest.run.summary.as_ref().unwrap();
    assert!(summary.cancelled);
    assert_eq!(summary.outcomes.len(), 1);
    assert_eq!(
        summary.outcomes[0].error,
        Some(kode_rest::http::Failure::Cancelled)
    );
    assert_eq!(app.state.rest.run.responses.len(), 1);
    assert!(hits.try_recv().is_err());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn validation_and_stale_preparation_fail_before_network_and_do_not_retarget_items() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state
        .rest
        .model
        .replace_collection(
            json!({"info":{},"item":[{"name":"Invalid","request":{"url":"{{missing}}"}}]}),
        )
        .unwrap();
    app.state.run_rest_selection(NodePath::default()).unwrap();
    until(&mut app, |app| app.state.rest.run.summary.is_some()).await;
    assert!(
        app.state.rest.run.summary.as_ref().unwrap().outcomes[0]
            .validation_error
            .as_ref()
            .unwrap()
            .contains("Unresolved")
    );
    assert!(app.state.rest.run.responses.is_empty());
    app.state.run_rest_selection(NodePath::default()).unwrap();
    app.state
        .rest
        .model
        .rename(&NodePath(vec![0]), "Changed")
        .unwrap();
    until(&mut app, |app| app.state.rest.run.pending.is_none()).await;
    assert!(
        app.state
            .rest
            .sidebar
            .message
            .contains("changed while preparing")
    );
    assert!(app.state.rest.run.running.is_none());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn a_prepared_environment_waits_for_the_modal_and_a_later_file_open_supersedes_it() {
    use kode_tui::{
        app::{state::OpenDestination, workspace::OpenRequest},
        panels::rest::sidebar::Action,
    };
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("native.txt");
    std::fs::write(&file, "native document").unwrap();
    let (mut app, _) = make(root.path(), 1);
    let environment = app.state.rest.model.create_environment("Pending");
    app.state.open_rest_environment(&environment, None).unwrap();
    app.state
        .rest
        .sidebar
        .perform(
            Action::Rename(NodePath::default()),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    let modal_focus = app.state.focus.focused;
    until(&mut app, |app| app.state.rest.ready_open.is_some()).await;
    assert_eq!(app.state.focus.focused, modal_focus);
    assert!(app.state.editor.is_none());
    app.state.dispatch(&key(Key::Escape)).unwrap();
    assert_eq!(
        app.state.editor.as_ref().unwrap().document_id(),
        app.state.rest.environment_document(&environment).unwrap()
    );
    let other = app.state.rest.model.create_environment("Superseded");
    app.state.open_rest_environment(&other, None).unwrap();
    app.state
        .open(
            OpenRequest {
                path: file.clone(),
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
                == &DocumentOrigin::File(file.clone())
        })
    })
    .await;
    assert!(app.state.rest.environment_document(&other).is_none());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn invalid_saved_rest_data_is_retained_and_cannot_accept_edits_that_would_be_lost() {
    let root = tempfile::tempdir().unwrap();
    let original = json!({"version":77,"environments":"future format","x-future":{"keep":7}});
    let session = kode_workspace::session::ProjectSessionV1 {
        rest_api: original.clone(),
        ..Default::default()
    };
    std::fs::write(
        root.path().join(".kode-rs.json"),
        serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();
    let prepared = prepare_workspace(
        root.path(),
        WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::new(prepared).unwrap();
    assert!(!app.state.rest.persistence_enabled);
    app.state.dispatch(&alt7()).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Character('n'))).unwrap();
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&key(Key::Character('n'))).unwrap();
    assert!(!app.state.rest.sidebar.has_modal());
    assert!(app.state.rest.model.environments().is_empty());
    assert_eq!(app.state.rest.model.tree().len(), 1);
    app.flush_before_leave().await.unwrap();
    assert_eq!(app.state.store.load().unwrap().session.rest_api, original);
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn import_notices_remain_inspectable_after_further_imports_and_restart() {
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    for document in [
        json!({"openapi":"3.0.0","paths":{"/first":{"get":{"parameters":[{"$ref":"https://example.invalid/external"}]}}}}),
        json!({"openapi":"3.0.0","components":{"securitySchemes":{"oauth":{"type":"oauth2","flows":{}}}},"paths":{"/second":{"get":{"security":[{"oauth":[]}]}}}}),
    ] {
        let imported = kode_rest::import::Import::from_document(document).unwrap();
        imported
            .apply(&mut app.state.rest.model, &NodePath::default())
            .unwrap();
    }
    app.state.changed().unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let (mut app, _) = make(root.path(), 2);
    assert!(app.state.rest.sidebar.notices.is_empty());
    app.state.dispatch(&alt7()).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Character('!'))).unwrap();
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .rest
                .documents
                .get(&view.document_id())
                .is_some_and(|entry| {
                    entry.kind == kode_tui::panels::rest::state::DocumentKind::Notices
                })
        })
    })
    .await;
    let document = app
        .state
        .documents
        .document(app.state.editor.as_ref().unwrap().document_id())
        .unwrap();
    assert!(document.is_read_only());
    let text = document.snapshot().text();
    assert!(text.contains("external-reference"));
    assert!(text.contains("unsupported-security"));
    assert!(text.contains("collectionPath"));
    assert!(
        app.state
            .rest
            .model
            .tree()
            .iter()
            .filter(|node| node.kind == kode_rest::model::NodeKind::Request)
            .all(
                |node| app.state.rest.model.node(&node.path).unwrap()["x-kode-import-incomplete"]
                    == true
            )
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
