use kode_core::{
    DocumentId, DocumentOrigin, DomainError, WorkspaceGeneration, commands::EditorCommand,
    document::Document,
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_rest::model::{EnvironmentValue, NodePath, Workspace};
use kode_tui::{
    editor::host::{EditorDocuments, EditorWork, ExecutionAction},
    focus::FocusState,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers},
    panels::rest::editor::{
        buffers::{self, Input, Override},
        form::BufferKind,
    },
};
use serde_json::json;
use std::{path::Path, sync::Arc};
const AREA: ratatui::layout::Rect = ratatui::layout::Rect::new(0, 0, 80, 24);
fn model() -> Workspace {
    let mut model = Workspace::default();
    model.replace_collection(json!({"info":{},"item":[
        {"name":"Call","request":{"method":"POST","url":"https://example.test/api","header":[{"key":"X-Test","value":"one"},{"key":"X-Test","value":"two","disabled":true}],"body":{"mode":"raw","raw":"{\"ok\":true}","options":{"raw":{"language":"json"}}}}},
        {"name":"Other","request":{"url":"https://other.example.test/"}}
    ]})).unwrap();
    model
}
fn input(model: &Workspace, kind: BufferKind, root: &Path) -> Input {
    Input {
        runtime_tokens: None,
        collection: model.export_collection(),
        path: NodePath(vec![0]),
        kind,
        key: "item-identity".into(),
        root: root.into(),
        environment: None,
        environment_text: None,
        environment_draft: None,
        overrides: Vec::new(),
        draft: None,
        drafts: Default::default(),
    }
}
fn snapshot(text: &str) -> kode_core::document::snapshot::TextSnapshot {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "fixture".into(),
            key: "buffer".into(),
            label: "synthetic".into(),
        },
        None,
        text,
    )
    .snapshot()
}
fn ctrl(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    })
}
#[tokio::test]
async fn worker_prepared_body_installs_the_shared_editor_with_raw_bytes_syntax_and_send_ownership()
{
    let model = model();
    let original = model.export_collection();
    let work = input(&model, BufferKind::RawBody, Path::new("."));
    let prepared =
        tokio::task::spawn_blocking(move || buffers::prepare(work, &CancellationToken::new()))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        prepared.document.snapshot().text().as_ref(),
        "{\"ok\":true}"
    );
    assert_eq!(prepared.descriptor.grammar, "json");
    assert_eq!(
        prepared.descriptor.mime.as_deref(),
        Some("application/json")
    );
    let mut documents = EditorDocuments::default();
    let mut view = prepared
        .install(
            &mut documents,
            WorkspaceGeneration(1),
            Arc::new(ClipboardService::internal()),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
    let mut focus = FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    };
    view.command(EditorCommand::SelectAll, &mut documents, AREA)
        .unwrap();
    view.dispatch(
        &InputEvent::Paste("typed request".into()),
        &mut documents,
        &mut focus,
        AREA,
    )
    .unwrap();
    view.dispatch(&ctrl(Key::Enter), &mut documents, &mut focus, AREA)
        .unwrap();
    let work = view.take_work();
    let execution = work
        .iter()
        .find_map(|work| {
            if let EditorWork::Execute(execution) = work {
                Some(execution)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(execution.action, ExecutionAction::SendRequest);
    assert_eq!(execution.snapshot.text().as_ref(), "typed request");
    let capture = Override {
        path: NodePath(vec![0]),
        kind: BufferKind::RawBody,
        snapshot: documents.document(view.document_id()).unwrap().snapshot(),
    };
    let mut captured = Workspace::from_state(model.project_state()).unwrap();
    buffers::apply_override(&mut captured, &capture, &CancellationToken::new()).unwrap();
    assert_eq!(
        captured.node(&NodePath(vec![0])).unwrap()["request"]["body"]["raw"],
        "typed request"
    );
    assert_eq!(model.collection(), &*original);
}
#[test]
fn prepared_headers_keep_completion_warnings_and_search_owns_ctrl_enter() {
    let model = model();
    let prepared = buffers::prepare(
        input(&model, BufferKind::Headers, Path::new(".")),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(prepared.warnings, [kode_core::LineIndex(0)]);
    let mut documents = EditorDocuments::default();
    let mut view = prepared
        .install(
            &mut documents,
            WorkspaceGeneration(1),
            Arc::new(ClipboardService::internal()),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
    assert!(view.host.external.is_some());
    assert!(!view.host.descriptor.auto_show_external);
    let mut focus = FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    };
    view.take_work();
    view.dispatch(&ctrl(Key::Character('f')), &mut documents, &mut focus, AREA)
        .unwrap();
    view.dispatch(
        &InputEvent::Paste("one".into()),
        &mut documents,
        &mut focus,
        AREA,
    )
    .unwrap();
    view.dispatch(&ctrl(Key::Enter), &mut documents, &mut focus, AREA)
        .unwrap();
    assert!(view.search.visible);
    assert!(
        !view
            .take_work()
            .iter()
            .any(|work| matches!(work, EditorWork::Execute(_)))
    );
}
#[test]
fn raw_preview_captures_the_latest_environment_and_body_and_ignores_unrelated_invalid_buffers() {
    let mut model = model();
    let path = NodePath(vec![0]);
    model.set_node_field(&path,"request",Some(json!({"method":"POST","url":"https://{{host}}/api?flag","body":{"mode":"raw","raw":"old body"}}))).unwrap();
    let id = model.create_environment("Selected");
    model.set_environment_values(
        &id,
        vec![EnvironmentValue {
            key: "host".into(),
            value: "old.example.test".into(),
            ..Default::default()
        }],
    );
    let mut work = input(&model, BufferKind::RawRequest, Path::new("."));
    work.environment = model.environment(&id).cloned();
    work.environment_text = Some(snapshot("host=current.example.test"));
    work.overrides = vec![
        Override {
            path: path.clone(),
            kind: BufferKind::RawBody,
            snapshot: snapshot("latest body"),
        },
        Override {
            path: NodePath(vec![1]),
            kind: BufferKind::Headers,
            snapshot: snapshot("unfinished unrelated header"),
        },
    ];
    let prepared = buffers::prepare(work, &CancellationToken::new()).unwrap();
    let text = prepared.document.snapshot().text();
    assert!(text.contains("POST /api?flag HTTP/1.1"));
    assert!(text.contains("Host: current.example.test"));
    assert!(text.contains("latest body"));
    assert!(!text.contains("old body"));
    let mut documents = EditorDocuments::default();
    let mut view = prepared
        .install(
            &mut documents,
            WorkspaceGeneration(1),
            Arc::new(ClipboardService::internal()),
            Arc::new(MonotonicClock::default()),
        )
        .unwrap();
    view.host.descriptor.read_only = false;
    let mut focus = FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    };
    assert!(matches!(
        view.dispatch(
            &InputEvent::Paste("forbidden".into()),
            &mut documents,
            &mut focus,
            AREA
        ),
        Err(DomainError::ReadOnly)
    ));
    let mut work = input(&model, BufferKind::RawRequest, Path::new("."));
    work.overrides.push(Override {
        path,
        kind: BufferKind::Headers,
        snapshot: snapshot("unfinished selected header"),
    });
    assert!(
        buffers::prepare(work, &CancellationToken::new())
            .err()
            .unwrap()
            .to_string()
            .contains("needs Name: Value")
    );
}
#[test]
fn draft_and_graphql_variables_are_verbatim_and_missing_files_cancellation_and_folder_roles_are_explicit()
 {
    let mut model = model();
    let path = NodePath(vec![0]);
    model.set_node_field(&path,"request",Some(json!({"url":"https://example.test","body":{"mode":"graphql","graphql":{"query":"query { item }","variables":"{unfinished"}}}))).unwrap();
    let prepared = buffers::prepare(
        input(&model, BufferKind::GraphqlVariables, Path::new(".")),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(prepared.document.snapshot().text().as_ref(), "{unfinished");
    assert_eq!(prepared.descriptor.grammar, "json");
    let mut work = input(&model, BufferKind::Headers, Path::new("."));
    work.draft = Some(Arc::from("# comment\nunfinished header"));
    let prepared = buffers::prepare(work, &CancellationToken::new()).unwrap();
    assert_eq!(
        prepared.document.snapshot().text().as_ref(),
        "# comment\nunfinished header"
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        buffers::prepare(
            input(&model, BufferKind::RawBody, Path::new(".")),
            &cancelled
        ),
        Err(DomainError::Cancelled)
    ));
    let directory = tempfile::tempdir().unwrap();
    model.set_node_field(&path,"request",Some(json!({"url":"https://example.test","body":{"mode":"file","file":{"src":"not-present.bin"}}}))).unwrap();
    assert!(
        buffers::prepare(
            input(&model, BufferKind::RawRequest, directory.path()),
            &CancellationToken::new()
        )
        .is_err()
    );
    let mut work = input(&model, BufferKind::RawBody, Path::new("."));
    work.path = NodePath::default();
    assert!(buffers::prepare(work, &CancellationToken::new()).is_err());
}

#[test]
fn unopened_environment_drafts_are_captured_for_raw_preview() {
    let mut model = model();
    model
        .set_node_field(
            &NodePath(vec![0]),
            "request",
            Some(json!({"url":"https://{{host}}/"})),
        )
        .unwrap();
    let id = model.create_environment("Draft environment");
    model.set_environment_values(
        &id,
        vec![EnvironmentValue {
            key: "host".into(),
            value: json!("old.example.test"),
            ..Default::default()
        }],
    );
    let mut work = input(&model, BufferKind::RawRequest, Path::new("."));
    work.environment = model.active_environment().cloned();
    work.environment_draft = Some(Arc::from("host=draft.example.test"));
    let prepared = buffers::prepare(work, &CancellationToken::new()).unwrap();
    assert!(
        prepared
            .document
            .snapshot()
            .text()
            .contains("Host: draft.example.test")
    );
}

#[test]
fn oauth_acquisition_uses_the_same_unopened_environment_draft_as_send_and_preview() {
    use kode_tui::panels::rest::{editor::jobs::ModelCapture, oauth::Prepare};
    let mut model = model();
    let path = NodePath(vec![0]);
    model.set_node_field(&path,"request",Some(json!({"url":"https://example.test/","auth":{"type":"oauth2","oauth2":[
        {"key":"clientId","value":"{{client}}"},{"key":"authUrl","value":"https://identity.example.test/authorize"},{"key":"accessTokenUrl","value":"https://identity.example.test/token"}
    ]}}))).unwrap();
    let id = model.create_environment("Selected");
    model.set_environment_values(
        &id,
        vec![EnvironmentValue {
            key: "client".into(),
            value: json!("old-client"),
            ..Default::default()
        }],
    );
    let prepared = Prepare {
        tag: kode_core::JobTag {
            workspace: WorkspaceGeneration(1),
            request: kode_core::RequestId::fresh().unwrap(),
            document: None,
        },
        key: "owner".into(),
        path,
        capture: ModelCapture {
            project: model.project_state(),
            environment_text: None,
            environment_draft: Some(Arc::from("client=draft-client")),
            root: "/synthetic".into(),
        },
    }
    .prepare()
    .unwrap();
    assert_eq!(prepared.client_id, "draft-client");
}
