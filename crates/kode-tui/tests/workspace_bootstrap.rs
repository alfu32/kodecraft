use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, TextRange, WorkspaceGeneration,
    commands::EditKind, document::Document, editor::selection::CaretState,
};
use kode_platform::{files::NativeDocumentIo, jobs::CancellationToken};
use kode_tui::app::workspace::*;
use kode_workspace::{
    mime::{ImageCapabilities, MimeDetector},
    project::Project,
    session::{EditorStateV1, ProjectSessionV1, persistence::SessionStore},
};
use std::{fs, path::Path, sync::Arc};
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
#[test]
fn application_database_export_policy_rejects_credentials_before_capture_or_any_session_backup_write()
 {
    use serde_json::json;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let store = prepared.store;
    let identity = store.save(&ProjectSessionV1::default(), None).unwrap();
    let before = fs::read(store.path()).unwrap();
    let mut session = ProjectSessionV1::default();
    for database in [
        json!({"dataSources":[{"jdbcUrl":"jdbc:h2:mem:test;password=synthetic-value"}]}),
        json!({"dataSources":[{"properties":{"sslpassword":"synthetic-value"}}]}),
        json!({"futurePlugin":{"client_secret":"synthetic-value"}}),
    ] {
        session.database = database;
        let error = store.save(&session, Some(&identity)).unwrap_err();
        assert!(!error.to_string().contains("synthetic-value"));
        assert!(capture_session(session.clone(), &prepared.project, vec![], None).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), before);
        assert!(!root.join(".kode-rs.json.previous").exists());
    }
    session.database = json!({"dataSources":[{"id":"synthetic","credentialReference":"synthetic.password","credentialProperties":{"token":"synthetic.token"}}],"sessions":[],"future":{"value":42}});
    store.save(&session, Some(&identity)).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    assert_eq!(value["database"]["future"]["value"], 42);
    assert_eq!(
        value["database"]["dataSources"][0]["credentialReference"],
        "synthetic.password"
    );
}
#[test]
fn explicit_file_focus_overrides_session_and_corrupt_config_does_not_erase_source_data() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("old.rs"), "old").unwrap();
    fs::write(root.join("explicit.rs"), "explicit").unwrap();
    let store = SessionStore::new(root).unwrap();
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(root.join("old.rs")),
        Some("rust".into()),
        "old",
    );
    let state = EditorStateV1::capture(root, &document, &CaretState::default(), 0, None);
    let session = ProjectSessionV1 {
        active_origin: Some(state.origin.clone()),
        open_editors: vec![state],
        ..Default::default()
    };
    store.save(&session, None).unwrap();
    let workspace = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        Some(root.join("explicit.rs")),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        workspace.initial.as_ref().unwrap().path,
        root.join("explicit.rs")
    );
    assert!(workspace.initial.as_ref().unwrap().saved.is_none());
    let opened = prepare_open(
        root,
        workspace.initial.unwrap(),
        tag(),
        &MimeDetector::compiled(),
        &ImageCapabilities::default(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        opened.prepared.document.snapshot().text().as_ref(),
        "explicit"
    );
    fs::write(store.path(), "corrupt raw state").unwrap();
    let workspace = prepare_workspace(
        root,
        WorkspaceGeneration(2),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(workspace.session.warning.is_some());
    assert_eq!(
        fs::read_to_string(store.path()).unwrap(),
        "corrupt raw state"
    );
    assert!(
        store
            .save(
                &workspace.session.session,
                workspace.session.disk_identity.as_ref()
            )
            .is_err()
    );
}
#[test]
fn native_capture_clears_legacy_format_once_and_retains_clean_crlf_history_and_unknown_fields() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("code.rs"), "disk\r\n").unwrap();
    let imported = kode_workspace::session::legacy::import(br#"{"openEditors":[{"path":"code.rs","buffer":{"lines":["disk", ""],"cursor":{"line":0,"column":1},"undo":[{"lines":["older"],"cursor":{"line":0,"column":1}}],"dirty":false},"futureEditor":{"keep":true}}],"futureRoot":{"keep":true}}"#).unwrap();
    let opened = prepare_open(
        root,
        OpenRequest {
            path: root.join("code.rs"),
            saved: Some(Arc::new(imported.open_editors[0].clone())),
            navigation: None,
        },
        tag(),
        &MimeDetector::compiled(),
        &ImageCapabilities::default(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(opened.prepared.document.history_depth().0 > 0);
    let project = Project::new(root, WorkspaceGeneration(1)).unwrap();
    let captured = CapturedDocument {
        document: opened.prepared.document,
        caret: opened.caret,
        scroll_rows: 5,
        disk_identity: opened.prepared.disk_identity,
        metadata: opened.metadata,
    };
    let state = capture_session(imported, &project, vec![captured], None).unwrap();
    assert!(!state.open_editors[0].legacy_format);
    assert!(state.open_editors[0].extra.contains_key("legacyBuffer"));
    assert_eq!(state.extra["futureRoot"]["keep"], true);
    let restored = kode_workspace::session::restore::restore(
        root,
        &state.open_editors[0],
        tag(),
        &NativeDocumentIo,
    )
    .unwrap();
    assert!(!restored.stale);
    assert!(restored.document.history_depth().0 > 0);
    assert!(!restored.document.is_dirty());
    assert_eq!(restored.scroll_top, 5);
}
#[test]
fn worker_session_capture_merges_dirty_views_recents_and_unopened_recovery_states() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut doc = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(root.join("code.rs")),
        Some("rust".into()),
        "saved",
    );
    doc.apply_edit(
        doc.revision(),
        TextRange {
            start: ByteOffset(0),
            end: ByteOffset(0),
        },
        "dirty ",
        EditKind::Typing,
    )
    .unwrap();
    let unknown = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(root.join("missing.rs")),
        None,
        "recover this",
    );
    let unopened = EditorStateV1::capture(root, &unknown, &CaretState::default(), 0, None);
    let mut base = ProjectSessionV1 {
        open_editors: vec![unopened],
        ..Default::default()
    };
    base.rest_api = serde_json::json!({"future":{"keep":true}});
    base.recent_files
        .push(kode_workspace::session::RecentFileV1 {
            path: "code.rs".into(),
            last_opened_epoch_millis: 1,
            last_modified_millis: None,
            editor: None,
            dirty: false,
            viewer_type: "CODE".into(),
            extra: Default::default(),
        });
    let project = Project::new(root, WorkspaceGeneration(1)).unwrap();
    let origin = doc.origin().clone();
    let captured = CapturedDocument {
        document: doc,
        caret: CaretState::default(),
        scroll_rows: 0,
        disk_identity: None,
        metadata: None,
    };
    let session = capture_session(base, &project, vec![captured], Some(origin)).unwrap();
    assert_eq!(session.open_editors.len(), 2);
    assert!(session.recent_files[0].dirty);
    assert_eq!(session.open_editors[0].history.current.text, "recover this");
    assert_eq!(session.rest_api["future"]["keep"], true);
    assert_eq!(
        session.active_origin,
        Some(DocumentOrigin::File("code.rs".into()))
    );
    assert!(epoch_millis() > 0);
    assert!(kode_workspace::session::resolve_path(root, Path::new("")).is_err());
}
#[tokio::test]
async fn mapped_scheduler_keeps_real_worker_results_bounded_and_tagged() {
    use kode_platform::jobs::{MonotonicClock, Notifications, OwnerId, Scheduler};
    use kode_tui::app::workers::MappedScheduler;
    enum Output {
        Tree(kode_workspace::tree::TreeSnapshot),
    }
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("file.rs"), "text").unwrap();
    let project = Project::new(directory.path(), WorkspaceGeneration(1)).unwrap();
    let mut tree = kode_workspace::tree::FileTree::new(&project);
    let job = tree.begin_refresh(&project).unwrap();
    let tag = job.tag;
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    kode_workspace::tree::schedule_tree(
        &mut MappedScheduler::new(&mut scheduler, Output::Tree),
        OwnerId(1),
        job,
    )
    .unwrap();
    let completion = scheduler.next().await.unwrap();
    assert_eq!(completion.tag, tag);
    let Output::Tree(result) = completion.result.unwrap();
    assert!(tree.publish(tag, Ok(result)));
    assert_eq!(tree.flattened().len(), 1);
    assert!(scheduler.shutdown().await.is_empty());
}

#[test]
fn binary_worker_load_is_bounded_cancellable_and_preserves_raw_non_utf8_bytes() {
    use kode_tui::app::workspace::{PreparedViewer, prepare_viewer};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let path = root.join("raw.bin");
    fs::write(&path, [0xff, 0xfe, 0, 0x80]).unwrap();
    let request = OpenRequest {
        path: path.clone(),
        saved: None,
        navigation: None,
    };
    let prepared = prepare_viewer(
        root,
        request.clone(),
        tag(),
        &MimeDetector::compiled(),
        &ImageCapabilities::default(),
        &CancellationToken::new(),
    )
    .unwrap();
    match prepared {
        PreparedViewer::Hex(prepared) => assert_eq!(prepared.bytes, [0xff, 0xfe, 0, 0x80]),
        _ => panic!("binary file routed to code"),
    }
    let misleading = root.join("invalid.rs");
    fs::write(&misleading, [0xff, 0xfe]).unwrap();
    match prepare_viewer(
        root,
        OpenRequest {
            path: misleading,
            saved: None,
            navigation: None,
        },
        tag(),
        &MimeDetector::compiled(),
        &ImageCapabilities::default(),
        &CancellationToken::new(),
    )
    .unwrap()
    {
        PreparedViewer::Hex(prepared) => assert!(prepared.warning.unwrap().contains("UTF-8 text")),
        _ => panic!("invalid text was not offered for byte inspection"),
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        prepare_viewer(
            root,
            request.clone(),
            tag(),
            &MimeDetector::compiled(),
            &ImageCapabilities::default(),
            &cancelled
        ),
        Err(kode_core::DomainError::Cancelled)
    ));
    fs::File::create(&path)
        .unwrap()
        .set_len(65 * 1024 * 1024)
        .unwrap();
    assert!(
        prepare_viewer(
            root,
            request,
            tag(),
            &MimeDetector::compiled(),
            &ImageCapabilities::default(),
            &CancellationToken::new()
        )
        .is_err()
    );
    fs::remove_file(&path).unwrap();
    assert!(
        prepare_viewer(
            root,
            OpenRequest {
                path,
                saved: None,
                navigation: None
            },
            tag(),
            &MimeDetector::compiled(),
            &ImageCapabilities::default(),
            &CancellationToken::new()
        )
        .is_err()
    );
}
