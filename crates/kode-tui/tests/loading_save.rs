use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Revision, TextRange, WorkspaceGeneration,
};
use kode_core::{
    commands::{EditKind, EditorCommand},
    document::Document,
    services::{DiskIdentity, LoadedDocument},
};
use kode_tui::{
    editor::loading::{DocumentLoader, SaveController, prepare},
    input::{
        bindings::editor_command,
        terminal::{Key, KeyInput, Modifiers},
    },
};
use std::path::PathBuf;

fn identity(content: &str) -> DiskIdentity {
    DiskIdentity {
        canonical_path: PathBuf::from("synthetic"),
        length: content.len() as u64,
        modified_nanos: Some(1),
        content_hash: content.into(),
    }
}

#[test]
fn late_loads_workspace_switch_missing_and_invalid_text_never_create_fake_empty_documents() {
    let mut loader = DocumentLoader::new(WorkspaceGeneration(1));
    let origin_a = DocumentOrigin::File(PathBuf::from("a"));
    let origin_b = DocumentOrigin::File(PathBuf::from("b"));
    let a = loader.begin(DocumentId(1), origin_a.clone());
    let b = loader.begin(DocumentId(2), origin_b.clone());
    let prepared_a = prepare(
        LoadedDocument {
            tag: a,
            origin: origin_a,
            bytes: b"A".to_vec(),
            disk_identity: None,
        },
        DocumentId(1),
        None,
    )
    .unwrap();
    assert!(!loader.complete(a, Ok(prepared_a)));
    let prepared_b = prepare(
        LoadedDocument {
            tag: b,
            origin: origin_b.clone(),
            bytes: b"B".to_vec(),
            disk_identity: None,
        },
        DocumentId(2),
        None,
    )
    .unwrap();
    assert!(loader.complete(b, Ok(prepared_b)));
    assert_eq!(
        loader
            .ready
            .as_ref()
            .unwrap()
            .document
            .snapshot()
            .text()
            .as_ref(),
        "B"
    );
    let missing = loader.begin(DocumentId(3), origin_b.clone());
    loader.complete(
        missing,
        Err(DomainError::MissingResource("missing file".into())),
    );
    assert!(loader.ready.is_none());
    assert!(loader.error.is_some());
    let invalid = loader.begin(DocumentId(4), origin_b.clone());
    let result = prepare(
        LoadedDocument {
            tag: invalid,
            origin: origin_b,
            bytes: vec![0xff],
            disk_identity: None,
        },
        DocumentId(4),
        None,
    );
    loader.complete(invalid, result);
    assert!(loader.error.is_some());
    assert!(loader.ready.is_none());
    let old = loader.begin(DocumentId(5), DocumentOrigin::File(PathBuf::from("old")));
    loader.switch_workspace();
    assert!(!loader.complete(old, Err(DomainError::Cancelled)));
    assert!(loader.error.is_none());
}

#[test]
fn saves_serialize_snapshots_update_disk_baselines_and_surface_failures() {
    let mut doc = Document::new(
        DocumentId(1),
        DocumentOrigin::File(PathBuf::from("synthetic")),
        None,
        "a",
    );
    let mut saves = SaveController::new(WorkspaceGeneration(1), Some(identity("a")));
    doc.apply_edit(
        Revision(0),
        TextRange::caret(ByteOffset(1)),
        "b",
        EditKind::Typing,
    )
    .unwrap();
    let first = saves.request(&doc).unwrap().unwrap();
    doc.apply_edit(
        Revision(1),
        TextRange::caret(ByteOffset(2)),
        "c",
        EditKind::Typing,
    )
    .unwrap();
    assert!(saves.request(&doc).unwrap().is_none());
    let second = saves
        .complete(first.tag, Ok(identity("ab")), &mut doc)
        .unwrap()
        .unwrap();
    assert!(doc.is_dirty());
    assert_eq!(doc.saved_revision(), Revision(1));
    assert_eq!(second.expected_disk.unwrap(), identity("ab"));
    assert_eq!(second.snapshot.text().as_ref(), "abc");
    saves
        .complete(second.tag, Ok(identity("abc")), &mut doc)
        .unwrap();
    assert!(!doc.is_dirty());
    assert!(!saves.is_saving());
    doc.apply_edit(
        Revision(2),
        TextRange::caret(ByteOffset(3)),
        "d",
        EditKind::Typing,
    )
    .unwrap();
    let failure = saves.request(&doc).unwrap().unwrap();
    saves
        .complete(
            failure.tag,
            Err(DomainError::SaveConflict(PathBuf::from("synthetic"))),
            &mut doc,
        )
        .unwrap();
    assert!(doc.is_dirty());
    assert!(saves.error.is_some());
}

#[test]
fn clipboard_shortcuts_support_ctrl_alt_case_and_leave_host_ctrl_enter_available() {
    for (ch, expected) in [
        ('C', EditorCommand::Copy),
        ('X', EditorCommand::Cut),
        ('V', EditorCommand::Paste),
    ] {
        for (ctrl, alt) in [(true, false), (false, true)] {
            assert_eq!(
                editor_command(KeyInput {
                    key: Key::Character(ch),
                    modifiers: Modifiers {
                        ctrl,
                        alt,
                        ..Modifiers::default()
                    },
                    repeat: false
                }),
                Some(expected.clone())
            );
        }
    }
    assert_eq!(
        editor_command(KeyInput {
            key: Key::Character('A'),
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::default()
            },
            repeat: false
        }),
        Some(EditorCommand::InsertText("A".into()))
    );
    assert_eq!(
        editor_command(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            repeat: false
        }),
        None
    );
}

#[tokio::test]
async fn real_file_load_and_save_run_through_bounded_workers_and_main_thread_controllers() {
    use kode_platform::{
        files::NativeDocumentIo,
        jobs::{MonotonicClock, Notifications, OwnerId, Scheduler},
    };
    use kode_tui::editor::loading::{PreparedDocument, schedule_load, schedule_save};
    use std::sync::Arc;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("worker.txt");
    std::fs::write(&path, b"\xef\xbb\xbfhello\r\nworld").unwrap();
    let notifications = Notifications::new(Arc::new(MonotonicClock::default()));
    let mut loads = Scheduler::<PreparedDocument>::new(notifications.clone());
    let mut loader = DocumentLoader::new(WorkspaceGeneration(1));
    let id = DocumentId::fresh().unwrap();
    let tag = loader.begin(id, DocumentOrigin::File(path.clone()));
    schedule_load(
        &mut loads,
        OwnerId(1),
        tag,
        id,
        DocumentOrigin::File(path.clone()),
        None,
        Arc::new(NativeDocumentIo),
    )
    .unwrap();
    let complete = loads.next().await.unwrap();
    assert!(loader.complete(complete.tag, complete.result));
    let mut prepared = loader.ready.take().unwrap();
    let end = ByteOffset(prepared.document.snapshot().len_bytes());
    prepared
        .document
        .apply_edit(Revision(0), TextRange::caret(end), "!", EditKind::Typing)
        .unwrap();
    let mut saves = SaveController::new(WorkspaceGeneration(1), prepared.disk_identity);
    let job = saves.request(&prepared.document).unwrap().unwrap();
    let mut workers = Scheduler::<DiskIdentity>::new(notifications);
    schedule_save(&mut workers, OwnerId(1), job, Arc::new(NativeDocumentIo)).unwrap();
    let complete = workers.next().await.unwrap();
    saves
        .complete(complete.tag, complete.result, &mut prepared.document)
        .unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"\xef\xbb\xbfhello\r\nworld!"
    );
    assert!(!prepared.document.is_dirty());
    assert!(loads.shutdown().await.is_empty());
    assert!(workers.shutdown().await.is_empty());
}
