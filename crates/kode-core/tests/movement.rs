use kode_core::{ByteOffset, DocumentId, DocumentOrigin, Selection};
use kode_core::{
    commands::Movement,
    document::Document,
    editor::{movement::move_caret, selection::CaretState},
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

#[test]
fn left_right_cross_lines_crlf_and_unicode_without_splitting_scalars() {
    let doc = document("a😀\r\n中\t\x01");
    let snapshot = doc.snapshot();
    let mut caret = CaretState::default();
    let expected = [1, 5, 7, 10, 11, 12, 12];
    for expected in expected {
        move_caret(&snapshot, &mut caret, Movement::Right, false, None).unwrap();
        assert_eq!(caret.cursor(), ByteOffset(expected));
    }
    for expected in [11, 10, 7, 5, 1, 0, 0] {
        move_caret(&snapshot, &mut caret, Movement::Left, false, None).unwrap();
        assert_eq!(caret.cursor(), ByteOffset(expected));
    }
}

#[test]
fn logical_up_down_clamp_short_lines_without_retaining_a_hidden_preferred_column() {
    let snapshot = document("abcdef\nx\nabcdef").snapshot();
    let mut caret = CaretState::default();
    caret.move_to(&snapshot, ByteOffset(4), false).unwrap();
    move_caret(&snapshot, &mut caret, Movement::Down, false, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(8));
    move_caret(&snapshot, &mut caret, Movement::Down, false, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(10));
    move_caret(&snapshot, &mut caret, Movement::Down, false, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(snapshot.len_bytes()));
    move_caret(&snapshot, &mut caret, Movement::DocumentStart, false, None).unwrap();
    move_caret(&snapshot, &mut caret, Movement::Up, false, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(0));
}

#[test]
fn shift_movement_preserves_reverse_anchor_and_plain_movement_clears_selection() {
    let snapshot = document("abc\ndef").snapshot();
    let mut caret = CaretState::default();
    caret.move_to(&snapshot, ByteOffset(6), false).unwrap();
    for _ in 0..5 {
        move_caret(&snapshot, &mut caret, Movement::Left, true, None).unwrap();
    }
    assert_eq!(
        caret.selection,
        Selection {
            anchor: ByteOffset(6),
            head: ByteOffset(1)
        }
    );
    assert_eq!(caret.selected_text(&snapshot).unwrap(), "bc\nde");
    move_caret(&snapshot, &mut caret, Movement::LineStart, false, None).unwrap();
    assert!(caret.selection.is_empty());
    caret.select_all(&snapshot);
    assert_eq!(caret.selected_text(&snapshot).unwrap(), "abc\ndef");
    caret.clear_selection();
    assert_eq!(caret.cursor(), ByteOffset(7));
    let empty = document("").snapshot();
    caret.select_all(&empty);
    assert!(caret.selection.is_empty());
}

#[test]
fn word_jumps_keep_original_ascii_identifier_punctuation_and_whitespace_rules() {
    let snapshot = document("foo.bar  $baz_2\n\nnext").snapshot();
    let mut caret = CaretState::default();
    for expected in [3, 7, 15, 16, 17, 21] {
        move_caret(&snapshot, &mut caret, Movement::WordRight, true, None).unwrap();
        assert_eq!(caret.cursor(), ByteOffset(expected));
    }
    for expected in [17, 16, 9, 4, 0, 0] {
        move_caret(&snapshot, &mut caret, Movement::WordLeft, false, None).unwrap();
        assert_eq!(caret.cursor(), ByteOffset(expected));
    }
}

#[test]
fn home_end_page_targets_and_invalid_explicit_selections_are_safe() {
    let snapshot = document("one\ntwo").snapshot();
    let mut caret = CaretState::default();
    caret.move_to(&snapshot, ByteOffset(5), false).unwrap();
    move_caret(&snapshot, &mut caret, Movement::LineStart, true, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(4));
    move_caret(&snapshot, &mut caret, Movement::LineEnd, true, None).unwrap();
    assert_eq!(caret.cursor(), ByteOffset(7));
    assert!(move_caret(&snapshot, &mut caret, Movement::PageUp, false, None).is_err());
    move_caret(
        &snapshot,
        &mut caret,
        Movement::PageUp,
        false,
        Some(ByteOffset(1)),
    )
    .unwrap();
    assert_eq!(caret.cursor(), ByteOffset(1));
    assert!(
        caret
            .set_selection(
                &snapshot,
                Selection {
                    anchor: ByteOffset(99),
                    head: ByteOffset(0)
                }
            )
            .is_err()
    );
    assert_eq!(caret.cursor(), ByteOffset(1));
}
