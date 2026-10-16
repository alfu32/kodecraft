use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Revision, Selection,
    TextRange, WorkspaceGeneration,
    commands::EditKind,
    document::{Document, encoding::decode},
    editor::selection::CaretState,
};
use kode_platform::{
    files::NativeDocumentIo,
    jobs::{MonotonicClock, Notifications, OwnerId, Scheduler},
};
use kode_workspace::session::{
    legacy::{self, LegacyBuffer, LegacyPosition, LegacySnapshot},
    persistence::*,
    restore::{is_stale, restore},
    *,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn put(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn legacy_fixture() -> ProjectSessionV1 {
    legacy::import(include_bytes!("../../../fixtures/sessions/legacy-v6.json")).unwrap()
}
fn capture(root: &Path, text: &str) -> EditorStateV1 {
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(root.join("source.rs")),
        Some("rust".into()),
        text,
    );
    EditorStateV1::capture(root, &document, &CaretState::default(), 4, None)
}
#[test]
fn legacy_fixture_converts_utf16_against_each_history_snapshot_and_retains_unknown_fields() {
    let state = legacy_fixture();
    assert_eq!(state.source_roots, [PathBuf::from("src")]);
    assert_eq!(state.scan_exclusions, [PathBuf::from("src/generated")]);
    let editor = &state.open_editors[0];
    assert_eq!(editor.history.current.text, "a😀b\ndirty");
    assert_eq!(editor.history.current.selection.head, ByteOffset(5));
    assert_eq!(editor.history.current.selection.anchor, ByteOffset(1));
    assert_eq!(editor.history.undo.len(), 1);
    assert_eq!(editor.history.undo[0].selection.head, ByteOffset(5));
    assert!(editor.history.saved_baseline_known);
    assert_eq!(editor.history.saved_text, "a😀b");
    assert_eq!(state.extra["futureExtension"], json!({"keep":true}));
    let snapshot = |lines: Vec<&str>, column: i64, anchor: i64, dirty| LegacySnapshot {
        lines: lines.into_iter().map(str::to_owned).collect(),
        cursor: LegacyPosition { line: 0, column },
        anchor: Some(LegacyPosition {
            line: 0,
            column: anchor,
        }),
        dirty,
        ..Default::default()
    };
    let history = legacy::convert_buffer(&LegacyBuffer {
        current: snapshot(vec!["😀z"], 0, 2, true),
        undo: vec![snapshot(vec!["a😀z"], 3, 1, false)],
        redo: vec![snapshot(vec!["xx😀z"], 4, 2, true)],
    });
    assert!(history.current.selection.is_reverse());
    assert_eq!(history.current.selection.anchor, ByteOffset(4));
    assert_eq!(history.undo[0].selection.head, ByteOffset(5));
    assert_eq!(history.redo[0].selection.head, ByteOffset(6));
    let middle = legacy::convert_buffer(&LegacyBuffer {
        current: snapshot(vec!["😀"], 1, 99, true),
        ..Default::default()
    });
    assert_eq!(middle.current.selection.head, ByteOffset(0));
    assert_eq!(middle.current.selection.anchor, ByteOffset(4));
    assert!(!middle.saved_baseline_known);
    let mut doc = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File("missing".into()),
        None,
        "",
    );
    doc.import_history(middle, &mut Default::default()).unwrap();
    assert!(doc.is_dirty());
    let empty_dirty = legacy::convert_buffer(&LegacyBuffer {
        current: LegacySnapshot {
            dirty: true,
            ..Default::default()
        },
        ..Default::default()
    });
    doc.import_history(empty_dirty, &mut Default::default())
        .unwrap();
    assert!(doc.is_dirty());
    assert!(
        !doc.export_history(&CaretState::default())
            .saved_baseline_known
    );
    doc.acknowledge_save(&doc.snapshot()).unwrap();
    assert!(!doc.is_dirty());
}
#[test]
fn legacy_absent_defaults_empty_roots_feature_payloads_and_original_database_rest_suites_roundtrip()
{
    let value = json!({"database":{"dataSources":[{"id":"db-1","name":"local","driver":{"id":"h2","driverClass":"org.h2.Driver","providerExtra":true},"jdbcUrl":"jdbc:h2:mem:kode","credentialReference":"synthetic-ref"}],"sessions":[{"id":"db-1:1","dataSourceId":"db-1","title":"sql@local:1","buffer":"select 1;","catalog":"test","autoCommit":false}],"future":42},
        "restApi":{"collection":{"info":{"name":"Workspace API"},"item":[{"x-kode-headers":[{"key":"X-Test","value":"yes"}],"response":[{"originalRequest":{}}],"futureNumber":123456789012345678901234567890u128}],"futureDoc":{"preserve":true}},"environments":[{"id":"local","values":[{"key":"base","value":"line\\nvalue","secret":true,"enabled":false,"unknown":3}]}],"ui":{"selectedPath":"0/1","responsePanelHeight":12,"prettyPrintResponses":false},"activeEnvironmentId":"local"},
        "recentFiles":[{"path":"image.png","lastOpenedEpochMillis":50,"viewerType":"IMAGE"},{"path":"bytes.bin","viewerType":"HEX"}],"openEditors":[{"path":"empty.rs","futureEditor":{"keep":true}}],"futureRoot":"keep"});
    let imported = legacy::import(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(imported.source_roots.is_empty());
    assert_eq!(imported.recent_files[0].viewer_type, "IMAGE");
    assert_eq!(imported.recent_files[1].viewer_type, "HEX");
    assert_eq!(imported.open_editors[0].history.current.text, "");
    assert_eq!(imported.database, value["database"]);
    assert_eq!(imported.rest_api, value["restApi"]);
    assert_eq!(
        imported.open_editors[0].extra["futureEditor"],
        value["openEditors"][0]["futureEditor"]
    );
    let roundtrip: ProjectSessionV1 =
        serde_json::from_slice(&serde_json::to_vec(&imported).unwrap()).unwrap();
    assert_eq!(roundtrip.database, imported.database);
    assert_eq!(roundtrip.rest_api, imported.rest_api);
    assert_eq!(roundtrip.extra, imported.extra);
    assert!(legacy::import(br#"{"version":7}"#).is_err());
    assert!(legacy::import(br#"{"version":5}"#).is_err());
    assert!(legacy::import(b"not json").is_err());
}
#[test]
fn dirty_emoji_crlf_bom_history_cursor_scroll_grammar_and_virtual_buffer_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut document = Document::from_decoded(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(root.join("source.rs")),
        Some("rust".into()),
        decode(b"\xef\xbb\xbfabc\r\n").unwrap(),
    );
    document
        .apply_edit(
            Revision(0),
            TextRange {
                start: ByteOffset(1),
                end: ByteOffset(2),
            },
            "😀",
            EditKind::Typing,
        )
        .unwrap();
    let caret = CaretState {
        secondary: vec![],
        selection: Selection {
            anchor: ByteOffset(5),
            head: ByteOffset(1),
        },
    };
    let mut editor = EditorStateV1::capture(root, &document, &caret, 99, None);
    editor.grammar_language = Some("rust".into());
    editor.grammar_available = true;
    editor.mime = Some("text/rust".into());
    let editor: EditorStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&editor).unwrap()).unwrap();
    let restored = restore(root, &editor, tag(), &NativeDocumentIo).unwrap();
    assert!(restored.disk_error.is_some());
    assert!(restored.document.is_dirty());
    assert!(restored.document.format().utf8_bom);
    assert_eq!(restored.scroll_top, 99);
    assert_eq!(restored.caret.selection, caret.selection);
    assert_eq!(&*restored.document.snapshot().text(), "a😀c\r\n");
    let mut document = restored.document;
    let mut caret = restored.caret;
    document.undo(&mut caret).unwrap();
    assert_eq!(&*document.snapshot().text(), "abc\r\n");
    assert!(!document.is_dirty());
    document.redo(&mut caret).unwrap();
    assert!(document.is_dirty());
    let virtual_doc = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "sql".into(),
            key: "db:1".into(),
            label: "sql@local:1".into(),
        },
        Some("sql".into()),
        "select 1;",
    );
    let editor = EditorStateV1::capture(root, &virtual_doc, &CaretState::default(), 0, None);
    let restored = restore(root, &editor, tag(), &NativeDocumentIo).unwrap();
    assert!(restored.disk_error.is_none());
    assert_eq!(restored.document.origin(), virtual_doc.origin());
}
#[test]
fn newer_disk_mtime_keeps_dirty_recovery_and_reloads_with_clamped_utf16_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "source.rs", "x😀\nnew disk");
    let mut state = capture(root, "old😀\ntext");
    state.history.current.text = "a😀b\ndirty".into();
    state.history.current.selection = Selection {
        anchor: ByteOffset(5),
        head: ByteOffset(1),
    };
    state.history.current.secondary = vec![Selection {
        anchor: ByteOffset(12),
        head: ByteOffset(12),
    }];
    state.last_modified_millis = Some(1);
    let restored = restore(root, &state, tag(), &NativeDocumentIo).unwrap();
    assert!(restored.stale);
    assert!(restored.recovery.is_some());
    assert!(!restored.document.is_dirty());
    assert_eq!(&*restored.document.snapshot().text(), "x😀\nnew disk");
    assert_eq!(restored.caret.selection.anchor, ByteOffset(5));
    assert_eq!(restored.caret.selection.head, ByteOffset(1));
    assert_eq!(
        restored.caret.secondary,
        vec![Selection {
            anchor: ByteOffset(11),
            head: ByteOffset(11),
        }]
    );
    let store = SessionStore::new(root).unwrap();
    let recovered = store
        .recover_editor(restored.recovery.as_ref().unwrap())
        .unwrap();
    let loaded: EditorStateV1 = serde_json::from_slice(&fs::read(&recovered).unwrap()).unwrap();
    assert_eq!(loaded.history.current.text, "a😀b\ndirty");
    assert_eq!(
        loaded.history.current.secondary,
        state.history.current.secondary
    );
    assert_eq!(
        fs::read_to_string(root.join("source.rs")).unwrap(),
        "x😀\nnew disk"
    );
    assert_ne!(store.recover_editor(&loaded).unwrap(), recovered);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&recovered).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!is_stale(Some(1000), Some(2000)));
    assert!(is_stale(Some(1000), Some(2001)));
    assert!(!is_stale(None, Some(5000)));
    assert!(!is_stale(Some(1), None));
    assert_eq!(
        resolve_path(root, Path::new("src/../source.rs")).unwrap(),
        root.join("source.rs")
    );
    assert!(resolve_path(root, Path::new("")).is_err());
}
#[test]
fn store_import_is_idempotent_defaults_preserve_saved_empty_and_never_rewrite_legacy() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::create_dir(root.join("include")).unwrap();
    let store = SessionStore::new(root).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.source, SessionSource::New);
    assert_eq!(
        loaded.session.source_roots,
        [PathBuf::from("src"), PathBuf::from("include")]
    );
    let bytes = include_bytes!("../../../fixtures/sessions/legacy-v6.json");
    fs::write(root.join(".kode.json"), bytes).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.source, SessionSource::Legacy);
    assert_eq!(
        serde_json::to_value(store.load().unwrap().session).unwrap(),
        serde_json::to_value(&loaded.session).unwrap()
    );
    assert!(!store.path().exists());
    let identity = store
        .save(&loaded.session, loaded.disk_identity.as_ref())
        .unwrap();
    assert_eq!(fs::read(root.join(".kode.json")).unwrap(), bytes);
    let mut session = loaded.session;
    session.source_roots.clear();
    store.save(&session, Some(&identity)).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.source, SessionSource::Rust);
    assert!(loaded.session.source_roots.is_empty());
    assert_eq!(
        loaded.session.extra["futureExtension"],
        json!({"keep":true})
    );
    let previous: Value =
        serde_json::from_slice(&fs::read(root.join(".kode-rs.json.previous")).unwrap()).unwrap();
    assert_eq!(previous["sourceRoots"], json!(["src"]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(store.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn corrupt_and_future_sessions_remain_recoverable_and_external_changes_block_writes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let store = SessionStore::new(root).unwrap();
    let session = ProjectSessionV1::default();
    let identity = store.save(&session, None).unwrap();
    assert!(matches!(
        store.save(&session, None),
        Err(DomainError::SaveConflict(_))
    ));
    fs::write(store.path(), "{corrupt").unwrap();
    assert!(store.load().is_err());
    assert!(store.save(&session, Some(&identity)).is_err());
    assert_eq!(fs::read_to_string(store.path()).unwrap(), "{corrupt");
    fs::write(
        root.join(".kode-rs.json.previous"),
        serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();
    let recovered = store.load().unwrap();
    assert_eq!(recovered.source, SessionSource::Previous);
    assert!(recovered.warning.is_some());
    assert!(
        store
            .save(&recovered.session, recovered.disk_identity.as_ref())
            .is_err()
    );
    assert_eq!(fs::read_to_string(store.path()).unwrap(), "{corrupt");
    fs::write(store.path(), r#"{"version":99,"future":"must keep"}"#).unwrap();
    assert!(matches!(store.load(), Err(DomainError::Unsupported(_))));
    let future_identity = kode_platform::files::disk_identity(&store.path()).unwrap();
    assert!(store.save(&session, Some(&future_identity)).is_err());
    assert_eq!(
        fs::read_to_string(store.path()).unwrap(),
        r#"{"version":99,"future":"must keep"}"#
    );
}
#[tokio::test]
async fn debounce_older_completion_newer_state_failure_retry_and_shutdown_flush_are_real() {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path()).unwrap();
    let mut persistence = SessionPersistence::new(WorkspaceGeneration(1), store.load().unwrap());
    let mut session = persistence.session().clone();
    session.extra.insert("generation".into(), json!(1));
    persistence.changed(session.clone(), 100).unwrap();
    assert!(persistence.begin(3099, false).unwrap().is_none());
    let old = persistence.begin(3100, false).unwrap().unwrap();
    session.extra.insert("generation".into(), json!(2));
    persistence.changed(session.clone(), 3101).unwrap();
    assert!(persistence.begin(9999, true).unwrap().is_none());
    assert!(persistence.flush_shutdown(store.clone()).await.is_err());
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_write(&mut scheduler, OwnerId(5), store.clone(), old).unwrap();
    let completed = scheduler.next().await.unwrap();
    assert!(!persistence.complete(tag(), Err(DomainError::Cancelled)));
    assert!(persistence.complete(completed.tag, completed.result));
    assert!(persistence.is_dirty());
    assert_eq!(store.load().unwrap().session.extra["generation"], json!(1));
    let failed = persistence.begin(9999, true).unwrap().unwrap();
    assert!(persistence.submission_failed(failed.tag, DomainError::QueueFull));
    assert!(persistence.is_dirty());
    assert!(persistence.error.is_some());
    assert!(scheduler.shutdown().await.is_empty());
    persistence.flush_shutdown(store.clone()).await.unwrap();
    assert!(!persistence.is_dirty());
    assert_eq!(store.load().unwrap().session.extra["generation"], json!(2));
    // A real disk conflict at shutdown retains the newer in-memory state.
    session.extra.insert("generation".into(), json!(3));
    persistence.changed(session, 10000).unwrap();
    fs::write(store.path(), "external edit").unwrap();
    assert!(persistence.flush_shutdown(store.clone()).await.is_err());
    assert!(persistence.is_dirty());
    assert!(matches!(
        persistence.error,
        Some(DomainError::SaveConflict(_))
    ));
    assert_eq!(fs::read_to_string(store.path()).unwrap(), "external edit");
}
#[test]
fn recents_sort_cap_remove_and_active_view_preserve_viewer_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut session = ProjectSessionV1::default();
    for index in 0..25 {
        session.record_recent(RecentFileV1 {
            path: format!("file{index:02}").into(),
            last_opened_epoch_millis: index,
            last_modified_millis: None,
            editor: Some(capture(root, "text")),
            dirty: false,
            viewer_type: if index % 2 == 0 { "CODE" } else { "HEX" }.into(),
            extra: Default::default(),
        });
    }
    assert_eq!(session.recent_files.len(), 20);
    assert_eq!(session.recent_files[0].path, PathBuf::from("file00"));
    assert!(session.most_recent_editor().is_some());
    session.open_editors.push(capture(root, "active"));
    session.active_origin = Some(session.open_editors[0].origin.clone());
    assert_eq!(
        session.most_recent_editor().unwrap().history.current.text,
        "active"
    );
    session.remove_recent(root, Path::new("file00")).unwrap();
    assert_eq!(session.recent_files.len(), 19);
}
#[test]
fn rust_fixture_checksum_staleness_and_recovery_failure_never_replace_dirty_user_data() {
    let mut session: ProjectSessionV1 =
        serde_json::from_slice(include_bytes!("../../../fixtures/session/rust-v1.json")).unwrap();
    session.validate_version().unwrap();
    assert_eq!(
        session.open_editors[0].history.current.selection.head,
        ByteOffset(1)
    );
    assert_eq!(session.extra["futureRoot"], json!({"keep":true}));
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/example.kt", "different disk");
    let store = SessionStore::new(root).unwrap();
    let restored = kode_workspace::session::restore::restore_with_recovery(
        &store,
        &session.open_editors[0],
        tag(),
        &NativeDocumentIo,
    )
    .unwrap();
    assert!(restored.editor.stale);
    let recovery = restored.recovery_path.unwrap();
    assert!(recovery.exists());
    assert_eq!(
        &*restored.editor.document.snapshot().text(),
        "different disk"
    );
    assert_eq!(session.open_editors[0].history.current.text, "a😀b\ndirty");
    // Invalid byte carets fail without changing or deleting the raw session.
    session.open_editors[0].history.current.selection.head = ByteOffset(2);
    assert!(restore(root, &session.open_editors[0], tag(), &NativeDocumentIo).is_err());
    let legacy = legacy::import(br#"{"activeOrigin":"future legacy field","ignorePatterns":{"unknown":true},"openEditors":[{"path":"file","history":{"legacy":true},"legacyBuffer":{"future":true}}]}"#).unwrap();
    let roundtrip: ProjectSessionV1 =
        serde_json::from_slice(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(
        roundtrip.extra["legacySessionFields"]["activeOrigin"],
        "future legacy field"
    );
    assert_eq!(
        roundtrip.extra["legacySessionFields"]["ignorePatterns"],
        json!({"unknown":true})
    );
    assert_eq!(
        roundtrip.open_editors[0].extra["legacyEditorFields"]["history"],
        json!({"legacy":true})
    );
    assert_eq!(
        roundtrip.open_editors[0].extra["legacyEditorFields"]["legacyBuffer"],
        json!({"future":true})
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let blocked = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        put(blocked.path(), "src/example.kt", "different disk");
        symlink(outside.path(), blocked.path().join(".kode-rs")).unwrap();
        let session: ProjectSessionV1 =
            serde_json::from_slice(include_bytes!("../../../fixtures/session/rust-v1.json"))
                .unwrap();
        let store = SessionStore::new(blocked.path()).unwrap();
        assert!(
            kode_workspace::session::restore::restore_with_recovery(
                &store,
                &session.open_editors[0],
                tag(),
                &NativeDocumentIo
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
