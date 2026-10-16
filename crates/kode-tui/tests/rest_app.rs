use kode_core::{DocumentOrigin, WorkspaceGeneration, services::Clipboard};
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
fn click(x: u16, y: u16) -> InputEvent {
    InputEvent::Pointer(kode_tui::input::terminal::PointerInput {
        x,
        y,
        action: kode_tui::input::terminal::PointerAction::Down(crossterm::event::MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
async fn select_request_tab(app: &mut AppRuntime, tab: kode_tui::panels::rest::editor::form::Tab) {
    render(app);
    let rect = app
        .state
        .rest
        .form
        .as_ref()
        .unwrap()
        .geometry
        .tabs
        .iter()
        .find(|(item, _)| *item == tab)
        .unwrap()
        .1;
    app.state.dispatch(&click(rect.x, rect.y)).unwrap();
    until(app, |app| {
        app.state.rest.pending_open.is_none() && app.state.rest.ready_open.is_none()
    })
    .await;
}
async fn select_response_tab(
    app: &mut AppRuntime,
    tab: kode_tui::panels::rest::editor::response::Tab,
) {
    render(app);
    let rect = app
        .state
        .rest
        .response
        .tabs
        .iter()
        .find(|(item, _)| *item == tab)
        .unwrap()
        .1;
    app.state.dispatch(&click(rect.x, rect.y)).unwrap();
    until(app, |app| {
        app.state
            .rest
            .response
            .installed
            .as_ref()
            .is_some_and(|proof| proof.tab == tab)
            && app.state.rest.response.pending.is_none()
    })
    .await;
}
#[tokio::test]
async fn collection_runner_presents_live_progress_validation_and_rebased_results_and_never_opens_a_replacement_item()
 {
    use kode_tui::panels::rest::sidebar::Action;
    let (url, mut hits, release, worker) = server().await;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{"name":"Run UI"},"item":[
        {"name":"Scope","item":[{"name":"First","request":{"url":url}},{"name":"Invalid","request":{"url":"{{unresolved}}"}}]},
        {"name":"Outside","request":{"url":"{{must-not-run}}"}}
    ]})).unwrap();
    app.state.run_rest_selection(NodePath(vec![0])).unwrap();
    next_hit(&mut app, &mut hits).await;
    until(&mut app, |app| {
        app.state.rest.form.is_some() && !app.state.rest.runner.area.is_empty()
    })
    .await;
    assert!(app.state.rest.runner.visible);
    assert_eq!(
        app.state.rest.run.active_item.as_ref().unwrap().name,
        "First"
    );
    assert!(app.state.rest.run.outcomes.is_empty());
    let first = app.state.rest.run.keys[&NodePath(vec![0, 0])].clone();
    let invalid = app.state.rest.run.keys[&NodePath(vec![0, 1])].clone();
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
    assert_eq!(app.state.rest.run.scope, Some(NodePath(vec![1])));
    release.send(()).unwrap();
    until(&mut app, |app| {
        app.state.rest.run.summary.is_some() && app.state.rest.run.outcomes.len() == 2
    })
    .await;
    assert_eq!(app.state.rest.run.outcomes[0].item.id, first);
    assert_eq!(app.state.rest.run.outcomes[1].item.id, invalid);
    assert!(app.state.rest.run.outcomes[1].validation_error.is_some());
    app.state.dispatch(&key(Key::Function(7))).unwrap();
    assert_eq!(app.state.focus.focused, Some(app.state.rest.runner.id));
    app.state.dispatch(&key(Key::Down)).unwrap();
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|form| form.key == invalid)
    })
    .await;
    assert_eq!(
        app.state.rest.form.as_ref().unwrap().path,
        NodePath(vec![1, 1])
    );
    assert!(
        app.state
            .rest
            .form
            .as_ref()
            .unwrap()
            .message
            .contains("Unresolved")
    );
    assert!(app.state.rest.response.body.is_empty());
    app.state.dispatch(&key(Key::Function(7))).unwrap();
    render(&mut app);
    app.state.dispatch(&key(Key::Home)).unwrap();
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .response
            .installed
            .as_ref()
            .is_some_and(|proof| proof.key == first)
    })
    .await;
    assert_eq!(
        app.state.rest.form.as_ref().unwrap().path,
        NodePath(vec![1, 0])
    );
    assert_eq!(app.state.rest.run.responses[0].body.as_ref(), b"abcd");
    app.state
        .rest
        .sidebar
        .perform(
            Action::DeleteConfirmed(NodePath(vec![1, 0])),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    app.state.drain_ui().unwrap();
    assert!(app.state.rest.form.is_none());
    app.state.dispatch(&alt7()).unwrap();
    app.state.dispatch(&key(Key::Function(7))).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|form| form.path == NodePath(vec![1]))
            && app.state.focus.focused == Some(app.state.rest.runner.id)
    })
    .await;
    render(&mut app);
    app.state.dispatch(&key(Key::Home)).unwrap();
    app.state.dispatch(&key(Key::Enter)).unwrap();
    assert!(app.state.rest.runner.message.contains("removed"));
    assert_eq!(
        app.state.rest.form.as_ref().unwrap().path,
        NodePath(vec![1])
    );
    assert!(hits.try_recv().is_err());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    worker.abort();
}
fn click_label(app: &mut AppRuntime, label: &str) {
    let area = Rect::new(0, 0, 100, 34);
    let mut buffer = Buffer::empty(area);
    app.state
        .render(area, &mut buffer, &Theme::compiled())
        .unwrap();
    for y in 0..area.height {
        let text: String = (0..area.width).map(|x| buffer[(x, y)].symbol()).collect();
        if let Some(x) = text.find(label) {
            app.state.dispatch(&click(x as u16, y)).unwrap();
            return;
        }
    }
    panic!("button not visible: {label}");
}
#[tokio::test]
async fn live_raw_preview_keeps_url_typing_focus_and_late_buffer_attachment_waits_for_returned_focus()
 {
    use kode_rest::model::editing::Field;
    use kode_tui::panels::rest::editor::form::{Action, Tab};
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Preview","request":{"url":"https://example.test/original","body":{"mode":"raw","raw":"body"}}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, Tab::Raw).await;
    app.state
        .rest
        .form
        .as_mut()
        .unwrap()
        .perform(
            Action::Focus(Field::Url),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("https://example.test/changed".into()))
        .unwrap();
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .snapshot()
                .text()
                .contains("GET /changed ")
        })
    })
    .await;
    let form = app.state.rest.form.as_ref().unwrap();
    assert_eq!(app.state.focus.focused, Some(form.id));
    assert_eq!(
        form.fields[&Field::Url].value,
        "https://example.test/changed"
    );
    app.state
        .dispatch(&InputEvent::Paste("-again".into()))
        .unwrap();
    assert_eq!(
        app.state.rest.form.as_ref().unwrap().fields[&Field::Url].value,
        "https://example.test/changed-again"
    );
    // A body-open completion must wait while the user deliberately focuses the sidebar.
    render(&mut app);
    let rect = app
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
    app.state.dispatch(&click(rect.x, rect.y)).unwrap();
    let return_focus = app.state.focus.focused;
    app.state.focus_panel();
    let panel = app.state.focus.focused;
    until(&mut app, |app| app.state.rest.ready_open.is_some()).await;
    assert_eq!(app.state.focus.focused, panel);
    assert_ne!(
        app.state.editor.as_ref().unwrap().host.descriptor.role,
        kode_tui::editor::host::HostRole::RestBody
    );
    app.state.focus.focused = return_focus;
    until(&mut app, |app| {
        app.state.editor.as_ref().is_some_and(|view| {
            view.host.descriptor.role == kode_tui::editor::host::HostRole::RestBody
        })
    })
    .await;
    // Query-row dialogs update the visible URL after worker field refresh.
    select_request_tab(&mut app, Tab::Params).await;
    click_label(&mut app, "+ Query");
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("fresh".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("value".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state.rest.form.as_ref().unwrap().fields[&Field::Url]
            .value
            .contains("fresh=value")
    })
    .await;
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
#[tokio::test]
async fn websocket_raw_preview_ctrl_enter_is_receive_only_and_incremental_binary_close_feedback_keeps_bytes()
 {
    use futures_util::{SinkExt, StreamExt};
    use kode_tui::panels::rest::editor::form::Tab;
    use tokio_tungstenite::tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (release, gate) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(60), socket.next())
                .await
                .is_err(),
            "default WS must not send the configured body"
        );
        socket.send(Message::Text("first".into())).await.unwrap();
        gate.await.unwrap();
        socket
            .send(Message::Binary(vec![255].into()))
            .await
            .unwrap();
        socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Normal,
                reason: "synthetic close".into(),
            })))
            .await
            .unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), socket.next()).await;
    });
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Socket","request":{"url":url,"body":{"mode":"raw","raw":"must-not-send"}}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, Tab::Raw).await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("receive-only")
    );
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    until(&mut app, |app| {
        app.state.rest.response.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .snapshot()
                .text()
                .as_ref()
                == "first"
        })
    })
    .await;
    assert!(app.state.rest.run.responses.is_empty());
    assert_eq!(
        app.state.focus.focused,
        Some(app.state.editor.as_ref().unwrap().input.id)
    );
    release.send(()).unwrap();
    until(&mut app, |app| {
        app.state.rest.run.running.is_none()
            && app.state.rest.response.status.contains("close 1000")
    })
    .await;
    let response = &app.state.rest.run.responses[0];
    assert_eq!(response.body.as_ref(), b"first\xff");
    assert_eq!(response.status_code, Some(101));
    assert!(response.error.is_none());
    assert!(app.state.rest.response.status.contains("invalid UTF-8"));
    assert!(app.state.rest.response.status.contains("synthetic close"));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    worker.await.unwrap();
}
#[tokio::test]
async fn request_formatting_is_explicit_undoable_revision_guarded_and_root_send_never_batches() {
    use kode_tui::panels::rest::editor::form::Tab;
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    let raw = "{\"answer\":1}";
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"JSON","request":{"url":"https://example.test","body":{"mode":"raw","raw":raw,"options":{"raw":{"language":"json"}}}}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, Tab::Body).await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        raw
    );
    click_label(&mut app, "Format body");
    until(&mut app, |app| {
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains('\n')
    })
    .await;
    let editor = app.state.rest.form.as_ref().unwrap().geometry.editor;
    app.state
        .dispatch(&click(editor.x + 5, editor.y + 1))
        .unwrap();
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
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        raw
    );
    click_label(&mut app, "Format body");
    // Type before the worker is accepted; its older formatting result is stale.
    app.state.focus.focused = Some(app.state.editor.as_ref().unwrap().input.id);
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("unfinished {".into()))
        .unwrap();
    for _ in 0..20 {
        app.pump().unwrap();
        tokio::task::yield_now().await;
    }
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "unfinished {"
    );
    app.state.open_rest_node(NodePath::default()).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|form| form.path.is_root())
    })
    .await;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.rest.run.pending.is_none());
    assert!(app.state.rest.run.running.is_none());
    assert!(
        app.state
            .rest
            .form
            .as_ref()
            .unwrap()
            .message
            .contains("Select a request")
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn focused_headers_keep_arrow_keys_with_transaction_visible_and_response_tabs_share_readonly_editor()
 {
    use kode_tui::panels::rest::editor::{form::Tab as RequestTab, response::Tab};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: session=synthetic; Path=/\r\nContent-Length: 12\r\nConnection: close\r\n\r\n{\"answer\":1}").await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let (mut app, clipboard) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Call","request":{"url":url,"header":[{"key":"X-One","value":"1"},{"key":"X-Two","value":"2"}]}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, RequestTab::Headers).await;
    let header_id = app.state.editor.as_ref().unwrap().document_id();
    let header_focus = app.state.editor.as_ref().unwrap().input.id;
    app.state.run_rest_selection(NodePath(vec![0])).unwrap();
    until(&mut app, |app| {
        app.state.rest.response.installed.is_some() && app.state.rest.run.running.is_none()
    })
    .await;
    assert_eq!(app.state.focus.focused, Some(header_focus));
    let response = app.state.rest.run.responses[0].clone();
    assert_eq!(response.body.as_ref(), b"{\"answer\":1}");
    let response_id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    assert!(
        app.state
            .documents
            .document(response_id)
            .unwrap()
            .snapshot()
            .text()
            .contains('\n')
    );
    select_response_tab(&mut app, Tab::Transaction).await;
    assert_eq!(app.state.focus.focused, Some(header_focus));
    let response_caret = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .input
        .state
        .caret
        .clone();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::End,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    let before = app
        .state
        .editor
        .as_ref()
        .unwrap()
        .input
        .state
        .caret
        .cursor();
    app.state.dispatch(&key(Key::Up)).unwrap();
    assert!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret
            .cursor()
            < before
    );
    assert_eq!(
        app.state
            .rest
            .response
            .editor
            .as_ref()
            .unwrap()
            .input
            .state
            .caret,
        response_caret
    );
    assert_eq!(app.state.editor.as_ref().unwrap().document_id(), header_id);
    let response_id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    let text = app
        .state
        .documents
        .document(response_id)
        .unwrap()
        .snapshot()
        .text();
    assert!(
        text.contains("Configured request")
            && text.contains("Normalized initial request")
            && text.contains("Response")
    );
    let area = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .geometry(&app.state.documents, app.state.rest.response.body)
        .unwrap()
        .text;
    app.state.dispatch(&click(area.x, area.y)).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state.dispatch(&ctrl('c')).unwrap();
    until(&mut app, |_| {
        clipboard
            .read()
            .is_ok_and(|copied| copied.contains("Logical request/response"))
    })
    .await;
    assert!(
        app.state
            .dispatch(&InputEvent::Paste("cannot mutate response".into()))
            .is_err()
    );
    assert_eq!(
        app.state
            .documents
            .document(response_id)
            .unwrap()
            .snapshot()
            .text(),
        text
    );
    select_response_tab(&mut app, Tab::Body).await;
    render(&mut app);
    let pretty = app.state.rest.response.pretty_button;
    app.state.dispatch(&click(pretty.x, pretty.y)).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .response
            .installed
            .as_ref()
            .is_some_and(|proof| !proof.pretty)
    })
    .await;
    let id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "{\"answer\":1}"
    );
    assert_eq!(response.body.as_ref(), b"{\"answer\":1}");
    select_response_tab(&mut app, Tab::Raw).await;
    let id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("content-type: application/json")
    );
    select_response_tab(&mut app, Tab::Cookies).await;
    let id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    assert!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("session=synthetic")
    );
    let clear = app.state.rest.response.clear_button;
    app.state.dispatch(&click(clear.x, clear.y)).unwrap();
    assert!(app.state.rest.run.client.cookies().snapshot().is_empty());
    let viewer = app.state.geometry.viewer;
    let split_y = viewer.y + app.state.rest.response.splitter.size;
    app.state.dispatch(&click(viewer.x, split_y)).unwrap();
    app.state
        .dispatch(&InputEvent::Pointer(
            kode_tui::input::terminal::PointerInput {
                x: viewer.x,
                y: split_y - 2,
                action: kode_tui::input::terminal::PointerAction::Drag(
                    crossterm::event::MouseButton::Left,
                ),
                modifiers: Modifiers::default(),
            },
        ))
        .unwrap();
    render(&mut app);
    assert_eq!(
        app.state.rest.response.splitter.size,
        split_y - 2 - viewer.y
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn incremental_response_is_visible_before_completion_and_stays_with_original_owner_on_switch()
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("sse://{}", listener.local_addr().unwrap());
    let (release, gate) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            bytes.push(byte[0]);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 20\r\nConnection: close\r\n\r\ndata:abc\n\n").await.unwrap();
        gate.await.unwrap();
        socket.write_all(b"data:def\n\n").await.unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"First","request":{"url":url}},{"name":"Other","request":{"url":"https://example.invalid"}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    let owner = app.state.rest.form.as_ref().unwrap().key.clone();
    app.state.run_rest_selection(NodePath(vec![0])).unwrap();
    until(&mut app, |app| {
        app.state.rest.response.editor.as_ref().is_some_and(|view| {
            app.state
                .documents
                .document(view.document_id())
                .unwrap()
                .snapshot()
                .text()
                .contains("abc")
        })
    })
    .await;
    assert!(app.state.rest.run.responses.is_empty());
    assert!(app.state.rest.run.live.is_some());
    app.state.open_rest_node(NodePath(vec![1])).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|form| form.path == NodePath(vec![1]))
    })
    .await;
    render(&mut app);
    assert!(app.state.rest.response.body.is_empty());
    let focus = app.state.focus.focused;
    release.send(()).unwrap();
    until(&mut app, |app| app.state.rest.run.running.is_none()).await;
    assert_eq!(app.state.rest.run.responses[0].key.item_id, owner);
    assert_eq!(
        app.state.rest.run.responses[0].body.as_ref(),
        b"data:abc\n\ndata:def\n\n"
    );
    assert_eq!(app.state.focus.focused, focus);
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .response
            .installed
            .as_ref()
            .is_some_and(|proof| proof.key == owner)
    })
    .await;
    let id = app
        .state
        .rest
        .response
        .editor
        .as_ref()
        .unwrap()
        .document_id();
    assert_eq!(
        app.state
            .documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "data:abc\n\ndata:def\n\n"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    worker.abort();
}

#[tokio::test]
async fn native_request_buffers_keep_identity_history_search_and_caret_across_request_switches() {
    use kode_tui::panels::rest::editor::form::{BufferKind, Tab};
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{"name":"Editors"},"item":[
        {"name":"First","request":{"url":"https://example.test","header":[{"key":"X-Keep","value":" old ","x-extra":7}],"body":{"mode":"raw","raw":"original"}}},
        {"name":"Second","request":{"url":"https://other.test"}}
    ]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    let owner = app.state.rest.form.as_ref().unwrap().key.clone();
    select_request_tab(&mut app, Tab::Headers).await;
    let headers = app.state.editor.as_ref().unwrap().document_id();
    assert_eq!(
        app.state.rest.buffers[&(owner.clone(), BufferKind::Headers)],
        headers
    );
    // Merely opening headers must preserve structured metadata and whitespace.
    app.pump().unwrap();
    assert_eq!(
        app.state
            .rest
            .model
            .collection()
            .pointer("/item/0/request/header/0/value")
            .unwrap(),
        " old "
    );
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("X-Keep: edited\nX-Extra: value".into()))
        .unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .model
            .collection()
            .pointer("/item/0/request/header/0/value")
            == Some(&json!("edited"))
    })
    .await;
    assert_eq!(
        app.state
            .rest
            .model
            .collection()
            .pointer("/item/0/request/header/0/x-extra")
            .unwrap(),
        7
    );
    let caret = app.state.editor.as_ref().unwrap().input.state.caret.clone();
    app.state.open_rest_node(NodePath(vec![1])).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .form
            .as_ref()
            .is_some_and(|form| form.path == NodePath(vec![1]))
    })
    .await;
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| {
        app.state
            .editor
            .as_ref()
            .is_some_and(|view| view.document_id() == headers)
    })
    .await;
    assert_eq!(app.state.editor.as_ref().unwrap().input.state.caret, caret);
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
    assert!(
        app.state
            .documents
            .document(headers)
            .unwrap()
            .snapshot()
            .text()
            .contains(" old ")
    );
    select_request_tab(&mut app, Tab::Body).await;
    let body = app.state.editor.as_ref().unwrap().document_id();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("raw bytes\n".into()))
        .unwrap();
    select_request_tab(&mut app, Tab::Headers).await;
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("Content-Type: application/json".into()))
        .unwrap();
    select_request_tab(&mut app, Tab::Body).await;
    assert_eq!(app.state.editor.as_ref().unwrap().document_id(), body);
    assert_eq!(
        app.state
            .editor
            .as_ref()
            .unwrap()
            .host
            .descriptor
            .language
            .as_deref(),
        Some("json")
    );
    assert_eq!(
        app.state
            .documents
            .document(body)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "raw bytes\n"
    );
    // Search owns Ctrl+Enter; it must never trigger REST execution.
    app.state.dispatch(&ctrl('f')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("raw".into()))
        .unwrap();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    assert!(app.state.rest.run.pending.is_none());
    assert_eq!(
        app.state
            .documents
            .document(body)
            .unwrap()
            .snapshot()
            .text()
            .as_ref(),
        "raw bytes\n"
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn unfinished_request_drafts_survive_immediate_quit_and_native_recents_do_not_override_rest_restore()
 {
    use kode_tui::panels::rest::editor::form::Tab;
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("native.txt");
    std::fs::write(&file, "native").unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state
        .open(
            kode_tui::app::workspace::OpenRequest {
                path: file.clone(),
                saved: None,
                navigation: None,
            },
            kode_tui::app::state::OpenDestination::Main,
        )
        .unwrap();
    until(&mut app, |app| app.state.pending_open.is_none()).await;
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Draft","request":{"url":"https://example.test","header":[{"key":"X-Valid","value":"retained"}]}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, Tab::Headers).await;
    app.state.dispatch(&ctrl('a')).unwrap();
    let draft = "# owned comment\nunfinished header";
    app.state
        .dispatch(&InputEvent::Paste(draft.into()))
        .unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    let (mut restored, _) = make(root.path(), 2);
    until(&mut restored, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut restored, Tab::Headers).await;
    let id = restored.state.editor.as_ref().unwrap().document_id();
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
            .collection()
            .pointer("/item/0/request/header/0/value")
            .unwrap(),
        "retained"
    );
    assert!(
        !restored
            .state
            .rest
            .model
            .collection()
            .to_string()
            .contains("unfinished header")
    );
    restored.flush_before_leave().await.unwrap();
    restored.quiesce().await.unwrap();
    // An explicit startup file still takes precedence over the saved REST view.
    let prepared = prepare_workspace(
        root.path(),
        WorkspaceGeneration(3),
        Some(file),
        &CancellationToken::new(),
    )
    .unwrap();
    let mut explicit = AppRuntime::new(prepared).unwrap();
    until(&mut explicit, |app| app.state.pending_open.is_none()).await;
    assert!(explicit.state.rest.form.is_none());
    explicit.flush_before_leave().await.unwrap();
    explicit.quiesce().await.unwrap();
}

#[tokio::test]
async fn send_captures_unpublished_header_and_body_ropes_and_rejects_invalid_headers_before_io() {
    use kode_tui::panels::rest::editor::form::Tab;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/captured", listener.local_addr().unwrap());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                bytes.push(byte[0]);
            }
            let headers = String::from_utf8(bytes).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_string)
                })
                .unwrap()
                .parse()
                .unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            tx.send((headers, body)).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        }
    });
    let root = tempfile::tempdir().unwrap();
    let (mut app, _) = make(root.path(), 1);
    app.state.rest.model.replace_collection(json!({"info":{},"item":[{"name":"Capture","request":{"method":"POST","url":url,"body":{"mode":"raw","raw":"old"}}}]})).unwrap();
    app.state.open_rest_node(NodePath(vec![0])).unwrap();
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, Tab::Headers).await;
    app.state
        .dispatch(&InputEvent::Paste("unfinished".into()))
        .unwrap();
    let enter = InputEvent::Key(KeyInput {
        key: Key::Enter,
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    app.state.dispatch(&enter).unwrap();
    until(&mut app, |app| app.state.rest.run.pending.is_none()).await;
    assert!(app.state.rest.run.running.is_none());
    assert!(rx.try_recv().is_err());
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("X-Current: exact".into()))
        .unwrap();
    select_request_tab(&mut app, Tab::Body).await;
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("new raw bytes\n".into()))
        .unwrap();
    app.state.dispatch(&enter).unwrap();
    // Further metadata edits during preparation must not invalidate/retarget the captured send.
    app.state
        .rest
        .model
        .rename(&NodePath(vec![0]), "Renamed after Send")
        .unwrap();
    until(&mut app, |app| !app.state.rest.run.responses.is_empty()).await;
    let (headers, body) = rx.try_recv().unwrap();
    assert!(headers.to_ascii_lowercase().contains("x-current: exact"));
    assert_eq!(body, b"new raw bytes\n");
    let key = app.state.rest.form.as_ref().unwrap().key.clone();
    assert_eq!(app.state.rest.run.responses[0].key.item_id, key);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    server.abort();
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
    until(&mut app, |app| app.state.rest.form.is_some()).await;
    select_request_tab(&mut app, kode_tui::panels::rest::editor::form::Tab::Raw).await;
    let id = app.state.editor.as_ref().unwrap().document_id();
    assert!(app.state.documents.document(id).unwrap().is_read_only());
    let snapshot = app.state.documents.document(id).unwrap().snapshot().text();
    assert!(snapshot.contains("GET /"));
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
        .add_request(&NodePath::default(), "Inserted")
        .unwrap();
    app.state
        .rest
        .run
        .tree_changed(kode_tui::panels::rest::sidebar::TreeChange::Inserted(
            NodePath(vec![1]),
        ));
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
