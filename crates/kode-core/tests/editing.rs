use kode_core::{ByteOffset, DocumentId, DocumentOrigin, Revision, Selection, TextRange};
use kode_core::{
    commands::EditKind,
    document::Document,
    editor::{
        edit::{TextEdit, delete, insert, newline},
        history::{HISTORY_LIMIT, HistoryStateV1, PersistedBuffer},
        selection::CaretState,
    },
};

fn document(text: &str) -> Document {
    Document::new(
        DocumentId(1),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "buffer".into(),
            label: "Test".into(),
        },
        None,
        text,
    )
}
fn end(doc: &Document) -> CaretState {
    let at = ByteOffset(doc.snapshot().len_bytes());
    CaretState {
        secondary: vec![],
        selection: Selection {
            anchor: at,
            head: at,
        },
    }
}

#[test]
fn reverse_multiline_paste_is_one_transaction_and_restores_exact_selection() {
    let mut doc = document("abc\ndef");
    let mut caret = CaretState {
        secondary: vec![],
        selection: Selection {
            anchor: ByteOffset(6),
            head: ByteOffset(1),
        },
    };
    insert(&mut doc, &mut caret, "X\nY", EditKind::Paste).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "aX\nYf");
    assert_eq!(caret.cursor(), ByteOffset(4));
    assert_eq!(doc.history_depth(), (1, 0));
    doc.undo(&mut caret).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "abc\ndef");
    assert_eq!(
        caret.selection,
        Selection {
            anchor: ByteOffset(6),
            head: ByteOffset(1)
        }
    );
    assert!(!doc.is_dirty());
    doc.redo(&mut caret).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "aX\nYf");
    assert_eq!(caret.cursor(), ByteOffset(4));
    assert!(doc.is_dirty());
    assert_eq!(doc.revision(), Revision(3));
}

#[test]
fn deletion_joins_lf_crlf_and_unicode_and_empty_edges_make_no_history() {
    for text in ["abc\ndef", "abc\r\ndef"] {
        let mut doc = document(text);
        let snapshot = doc.snapshot();
        let at = snapshot.line_start(kode_core::LineIndex(1)).unwrap();
        let mut caret = CaretState {
            secondary: vec![],
            selection: Selection {
                anchor: at,
                head: at,
            },
        };
        delete(&mut doc, &mut caret, true).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), "abcdef");
        assert_eq!(caret.cursor(), ByteOffset(3));
        doc.undo(&mut caret).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), text);
        caret
            .move_to(&doc.snapshot(), ByteOffset(3), false)
            .unwrap();
        delete(&mut doc, &mut caret, false).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), "abcdef");
    }
    let mut doc = document("a😀");
    let mut caret = end(&doc);
    delete(&mut doc, &mut caret, true).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "a");
    assert_eq!(caret.cursor(), ByteOffset(1));
    let mut doc = document("");
    let mut caret = CaretState::default();
    assert!(delete(&mut doc, &mut caret, true).unwrap().is_none());
    assert!(delete(&mut doc, &mut caret, false).unwrap().is_none());
    assert!(!doc.undo(&mut caret).unwrap());
    assert!(!doc.redo(&mut caret).unwrap());
    assert_eq!(doc.revision(), Revision(0));
    assert_eq!(doc.history_depth(), (0, 0));
    assert!(!doc.is_dirty());
}

#[test]
fn newline_copies_current_leading_spaces_tabs_and_file_break_style() {
    for (text, expected) in [
        ("\t  hello", "\t  hello\n\t  "),
        ("first\r\n  last", "first\r\n  last\r\n  "),
    ] {
        let mut doc = document(text);
        let mut caret = end(&doc);
        newline(&mut doc, &mut caret).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), expected);
        assert_eq!(doc.history_depth(), (1, 0));
        doc.undo(&mut caret).unwrap();
        assert_eq!(doc.snapshot().text().as_ref(), text);
    }
}

#[test]
fn replace_all_groups_disjoint_original_ranges_and_rejects_overlap_atomically() {
    let mut doc = document("a a a");
    let mut caret = CaretState::default();
    let edits = [0, 2, 4]
        .into_iter()
        .map(|start| TextEdit {
            range: TextRange::new(ByteOffset(start), ByteOffset(start + 1)).unwrap(),
            inserted: "BC".into(),
        })
        .collect();
    let outcome = doc
        .apply_transaction(
            Revision(0),
            edits,
            EditKind::ReplaceAll,
            caret.selection,
            None,
        )
        .unwrap()
        .unwrap();
    caret.selection = outcome.selection;
    assert_eq!(doc.snapshot().text().as_ref(), "BC BC BC");
    assert_eq!(outcome.deltas.len(), 3);
    assert_eq!(doc.revision(), Revision(1));
    assert_eq!(doc.history_depth(), (1, 0));
    assert_eq!(caret.cursor(), ByteOffset(8));
    doc.undo(&mut caret).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "a a a");
    let revision = doc.revision();
    let depth = doc.history_depth();
    assert!(
        doc.apply_transaction(
            revision,
            vec![
                TextEdit {
                    range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
                    inserted: "x".into()
                },
                TextEdit {
                    range: TextRange::new(ByteOffset(2), ByteOffset(5)).unwrap(),
                    inserted: "y".into()
                }
            ],
            EditKind::ReplaceAll,
            caret.selection,
            None
        )
        .is_err()
    );
    assert_eq!(doc.snapshot().text().as_ref(), "a a a");
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.history_depth(), depth);
}

#[test]
fn noops_do_not_clear_redo_and_new_edit_does() {
    let mut doc = document("abc");
    let mut caret = end(&doc);
    insert(&mut doc, &mut caret, "1", EditKind::Typing).unwrap();
    doc.undo(&mut caret).unwrap();
    let revision = doc.revision();
    assert!(
        insert(&mut doc, &mut caret, "", EditKind::Typing)
            .unwrap()
            .is_none()
    );
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.history_depth(), (0, 1));
    insert(&mut doc, &mut caret, "A", EditKind::Typing).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "abcA");
    assert!(!doc.redo(&mut caret).unwrap());
}

#[test]
fn history_json_roundtrip_retains_dirty_text_undo_redo_and_caret() {
    let mut doc = document("seed");
    let mut caret = end(&doc);
    insert(&mut doc, &mut caret, "A", EditKind::Typing).unwrap();
    insert(&mut doc, &mut caret, "B", EditKind::Typing).unwrap();
    doc.undo(&mut caret).unwrap();
    let state: HistoryStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&doc.export_history(&caret)).unwrap()).unwrap();
    let mut restored = document("");
    let mut restored_caret = CaretState::default();
    let report = restored.import_history(state, &mut restored_caret).unwrap();
    assert_eq!(report.discarded_undo, 0);
    assert_eq!(report.discarded_redo, 0);
    assert_eq!(restored.snapshot().text().as_ref(), "seedA");
    assert_eq!(restored_caret, caret);
    assert!(restored.is_dirty());
    restored.redo(&mut restored_caret).unwrap();
    assert_eq!(restored.snapshot().text().as_ref(), "seedAB");
    restored.undo(&mut restored_caret).unwrap();
    restored.undo(&mut restored_caret).unwrap();
    assert_eq!(restored.snapshot().text().as_ref(), "seed");
    assert!(!restored.is_dirty());
    assert_eq!(restored_caret.cursor(), ByteOffset(4));
}

#[test]
fn history_limits_are_explicit_and_corrupt_imports_leave_the_document_intact() {
    let mut doc = document("");
    let mut caret = CaretState::default();
    let mut discarded = 0;
    for _ in 0..HISTORY_LIMIT + 5 {
        discarded += insert(&mut doc, &mut caret, "x", EditKind::Typing)
            .unwrap()
            .unwrap()
            .history_discarded;
    }
    assert_eq!(discarded, 5);
    assert_eq!(doc.history_depth(), (HISTORY_LIMIT, 0));
    let state = doc.export_history(&caret);
    assert_eq!(state.undo.len(), 30);
    assert_eq!(state.omitted_undo, 175);
    let mut bad = state.clone();
    bad.current = PersistedBuffer {
        secondary: vec![],
        text: "a😀b".into(),
        selection: Selection {
            anchor: ByteOffset(2),
            head: ByteOffset(5),
        },
    };
    let before = doc.snapshot().text();
    assert!(doc.import_history(bad, &mut caret).is_err());
    assert_eq!(doc.snapshot().text(), before);
    let mut excess = state.clone();
    excess.undo = vec![
        PersistedBuffer {
            secondary: vec![],
            text: "old".into(),
            selection: Selection::default()
        };
        HISTORY_LIMIT + 3
    ];
    let report = doc.import_history(excess, &mut caret).unwrap();
    assert_eq!(report.discarded_undo, 3);
    assert_eq!(doc.history_depth().0, HISTORY_LIMIT);
    let mut unsupported = state;
    unsupported.version = 2;
    assert!(doc.import_history(unsupported, &mut caret).is_err());
}

#[test]
fn readonly_guards_cover_history_programmatic_replacement_and_paste() {
    let mut doc = document("abc");
    let mut caret = end(&doc);
    insert(&mut doc, &mut caret, "X", EditKind::Typing).unwrap();
    let state = doc.export_history(&caret);
    doc.set_read_only(true);
    let before = doc.snapshot().text();
    let revision = doc.revision();
    let depth = doc.history_depth();
    assert!(insert(&mut doc, &mut caret, "paste", EditKind::Paste).is_err());
    assert!(delete(&mut doc, &mut caret, true).is_err());
    assert!(doc.undo(&mut caret).is_err());
    assert!(doc.redo(&mut caret).is_err());
    assert!(doc.import_history(state, &mut caret).is_err());
    assert!(
        doc.apply_edit(
            revision,
            TextRange::caret(ByteOffset(0)),
            "p",
            EditKind::Programmatic
        )
        .is_err()
    );
    assert_eq!(doc.snapshot().text(), before);
    assert_eq!(doc.revision(), revision);
    assert_eq!(doc.history_depth(), depth);
}

#[test]
fn redo_restores_the_cursor_present_when_undo_was_requested() {
    let mut doc = document("abc");
    let mut caret = end(&doc);
    insert(&mut doc, &mut caret, "X", EditKind::Typing).unwrap();
    caret
        .move_to(&doc.snapshot(), ByteOffset(0), false)
        .unwrap();
    doc.undo(&mut caret).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(3));
    doc.redo(&mut caret).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(0));
    assert_eq!(doc.snapshot().text().as_ref(), "abcX");
}
