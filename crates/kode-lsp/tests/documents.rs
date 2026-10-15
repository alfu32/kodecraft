use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Revision, TextRange, commands::EditKind,
    document::Document,
};
use kode_lsp::documents::*;
use lsp_types::{
    Position, PositionEncodingKind, Range, SaveOptions, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
};
fn doc(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File("/tmp/synthetic source 😀.kt".into()),
        Some("kotlin".into()),
        text,
    )
}
fn mode(kind: TextDocumentSyncKind) -> SyncMode {
    SyncMode {
        open_close: true,
        change: kind,
        save: true,
        save_text: false,
    }
}
#[test]
fn negotiated_encodings_unicode_tabs_crlf_and_bare_cr_use_protocol_lines_without_changing_editor_bytes()
 {
    assert_eq!(
        PositionEncoding::negotiate(None).unwrap(),
        PositionEncoding::Utf16
    );
    assert_eq!(
        PositionEncoding::negotiate(Some(&PositionEncodingKind::UTF8)).unwrap(),
        PositionEncoding::Utf8
    );
    assert!(PositionEncoding::negotiate(Some(&PositionEncodingKind::new("unsupported"))).is_err());
    let document = doc("a\t😀é\r\nz\rlast");
    let snapshot = document.snapshot();
    assert_eq!(snapshot.len_lines(), 2);
    for (encoding, after_emoji, eol) in [
        (PositionEncoding::Utf8, 6, 8),
        (PositionEncoding::Utf16, 4, 5),
        (PositionEncoding::Utf32, 3, 4),
    ] {
        let mapper = PositionMapper::new(snapshot.clone(), encoding).unwrap();
        assert_eq!(
            mapper.position(ByteOffset(6)).unwrap(),
            Position::new(0, after_emoji)
        );
        assert_eq!(
            mapper.byte(Position::new(0, after_emoji)).unwrap(),
            ByteOffset(6)
        );
        assert_eq!(
            mapper.position(ByteOffset(9)).unwrap(),
            Position::new(0, eol),
            "cannot address inside CRLF"
        );
        assert_eq!(
            mapper.position(ByteOffset(12)).unwrap(),
            Position::new(2, 0)
        );
        assert_eq!(mapper.byte(Position::new(2, 999)).unwrap(), ByteOffset(16));
        assert!(mapper.byte(Position::new(999, 0)).is_err());
        assert_eq!(
            mapper
                .byte_range(Range::new(Position::new(1, 0), Position::new(2, 0)))
                .unwrap(),
            TextRange {
                start: ByteOffset(10),
                end: ByteOffset(12)
            }
        );
    }
    let mapper = PositionMapper::new(snapshot.clone(), PositionEncoding::Utf16).unwrap();
    assert_eq!(mapper.byte(Position::new(0, 3)).unwrap(), ByteOffset(2));
    let mapper = PositionMapper::new(snapshot.clone(), PositionEncoding::Utf8).unwrap();
    assert_eq!(mapper.byte(Position::new(0, 3)).unwrap(), ByteOffset(2));
    let uri = file_uri(std::path::Path::new("/tmp/a b#😀.kt")).unwrap();
    assert!(uri.as_str().contains("%20") && uri.as_str().contains("%23"));
    assert_eq!(
        uri_path(&uri).unwrap(),
        std::path::Path::new("/tmp/a b#😀.kt")
    );
    assert_eq!(snapshot.text().as_ref(), "a\t😀é\r\nz\rlast");
}
#[test]
fn document_open_incremental_save_close_order_and_revision_identity_are_explicit() {
    let mut document = doc("a\t😀é\r\nz\rlast");
    let mut tracker = DocumentTracker::default();
    let mode = mode(TextDocumentSyncKind::INCREMENTAL);
    let uri = file_uri(std::path::Path::new("/tmp/synthetic source 😀.kt")).unwrap();
    let old = document.snapshot();
    let open = tracker
        .synchronize(old.clone(), mode, PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].method, "textDocument/didOpen");
    assert_eq!(open[0].params["textDocument"]["version"], 1);
    assert_eq!(open[0].params["textDocument"]["text"], old.text().as_ref());
    assert!(
        tracker
            .synchronize(old.clone(), mode, PositionEncoding::Utf16)
            .unwrap()
            .is_empty()
    );
    document
        .apply_edit(
            document.revision(),
            TextRange {
                start: ByteOffset(2),
                end: ByteOffset(6),
            },
            "中",
            EditKind::Typing,
        )
        .unwrap();
    let changed = tracker
        .synchronize(document.snapshot(), mode, PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(changed[0].method, "textDocument/didChange");
    assert_eq!(changed[0].params["textDocument"]["version"], 2);
    assert_eq!(
        changed[0].params["contentChanges"][0],
        serde_json::json!({"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":4}},"text":"中"})
    );
    assert!(matches!(
        tracker.synchronize(old, mode, PositionEncoding::Utf16),
        Err(DomainError::StaleRevision { .. })
    ));
    let save = tracker
        .save(document.snapshot(), mode, PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(save.len(), 1);
    assert_eq!(save[0].method, "textDocument/didSave");
    assert!(save[0].params.get("text").is_none());
    assert_eq!(tracker.stamp(&uri).unwrap().revision, Revision(1));
    assert_eq!(tracker.stamp(&uri).unwrap().version, 2);
    let replacement = doc("a fresh identity");
    let reset = tracker
        .synchronize(replacement.snapshot(), mode, PositionEncoding::Utf16)
        .unwrap();
    assert_eq!(
        reset.iter().map(|n| n.method).collect::<Vec<_>>(),
        ["textDocument/didClose", "textDocument/didOpen"]
    );
    assert_eq!(tracker.stamp(&uri).unwrap().document, replacement.id());
    assert_eq!(tracker.stamp(&uri).unwrap().version, 1);
    assert_eq!(
        tracker.close(replacement.origin(), mode).unwrap()[0].method,
        "textDocument/didClose"
    );
    assert!(tracker.stamp(&uri).is_none());
    assert!(
        tracker
            .close(replacement.origin(), mode)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn incremental_newline_replacements_expand_across_crlf_instead_of_sending_an_unaddressable_half_break()
 {
    for (before, after, expected_start, expected_end, expected_text) in [
        (
            "a\r\nb",
            "a\nb",
            Position::new(0, 1),
            Position::new(1, 0),
            "\n",
        ),
        (
            "a\r\nb",
            "a\rb",
            Position::new(0, 1),
            Position::new(1, 0),
            "\r",
        ),
        (
            "a\nb",
            "a\r\nb",
            Position::new(0, 1),
            Position::new(0, 1),
            "\r",
        ),
        (
            "a\rb",
            "a\nb",
            Position::new(0, 1),
            Position::new(1, 0),
            "\n",
        ),
    ] {
        let mut document = doc(before);
        let old = document.snapshot();
        let mut tracker = DocumentTracker::default();
        let mode = mode(TextDocumentSyncKind::INCREMENTAL);
        tracker
            .synchronize(old.clone(), mode, PositionEncoding::Utf16)
            .unwrap();
        document
            .apply_edit(
                document.revision(),
                TextRange {
                    start: ByteOffset(0),
                    end: ByteOffset(before.len()),
                },
                after,
                EditKind::Typing,
            )
            .unwrap();
        let changes = tracker
            .synchronize(document.snapshot(), mode, PositionEncoding::Utf16)
            .unwrap();
        let change = &changes[0].params["contentChanges"][0];
        let range: Range = serde_json::from_value(change["range"].clone()).unwrap();
        assert_eq!(
            range,
            Range::new(expected_start, expected_end),
            "{before:?} → {after:?}"
        );
        assert_eq!(change["text"], expected_text);
        let mapper = PositionMapper::new(old, PositionEncoding::Utf16).unwrap();
        let bytes = mapper.byte_range(range).unwrap();
        let mut applied = before.to_owned();
        applied.replace_range(bytes.start.0..bytes.end.0, change["text"].as_str().unwrap());
        assert_eq!(applied, after);
    }
}
#[test]
fn full_sync_save_options_disabled_sync_virtual_origins_and_document_bounds_are_honest() {
    assert_eq!(language_id("tsx"), "typescriptreact");
    assert_eq!(language_id("jsx"), "javascriptreact");
    assert_eq!(language_id("bash"), "shellscript");
    let options = TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(TextDocumentSyncKind::FULL),
        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
            include_text: Some(true),
        })),
        ..Default::default()
    });
    let mode = SyncMode::negotiated(Some(&options));
    let mut document = doc("first");
    let mut tracker = DocumentTracker::default();
    tracker
        .synchronize(document.snapshot(), mode, PositionEncoding::Utf8)
        .unwrap();
    document
        .apply_edit(
            document.revision(),
            TextRange::caret(ByteOffset(5)),
            " second",
            EditKind::Typing,
        )
        .unwrap();
    let messages = tracker
        .save(document.snapshot(), mode, PositionEncoding::Utf8)
        .unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0].params["contentChanges"][0],
        serde_json::json!({"text":"first second"})
    );
    assert_eq!(messages[1].params["text"], "first second");
    assert!(
        DocumentTracker::default()
            .synchronize(
                document.snapshot(),
                SyncMode::negotiated(None),
                PositionEncoding::Utf16
            )
            .is_err()
    );
    let mut snapshot = document.snapshot();
    snapshot.origin = DocumentOrigin::Virtual {
        owner: "test".into(),
        key: "query".into(),
        label: "SQL".into(),
    };
    assert!(
        tracker
            .synchronize(snapshot, mode, PositionEncoding::Utf16)
            .is_err()
    );
    assert!(
        PositionMapper::new(
            doc(&"x".repeat(MAX_DOCUMENT_BYTES + 1)).snapshot(),
            PositionEncoding::Utf16
        )
        .is_err()
    );
    let no_change = SyncMode {
        open_close: true,
        change: TextDocumentSyncKind::NONE,
        save: false,
        save_text: false,
    };
    let mut tracker = DocumentTracker::default();
    tracker
        .synchronize(document.snapshot(), no_change, PositionEncoding::Utf16)
        .unwrap();
    document
        .apply_edit(
            document.revision(),
            TextRange::caret(ByteOffset(0)),
            "new ",
            EditKind::Typing,
        )
        .unwrap();
    assert!(
        tracker
            .synchronize(document.snapshot(), no_change, PositionEncoding::Utf16)
            .is_err()
    );
}
