use kode_core::{
    ByteOffset, CharOffset, DocumentId, DocumentOrigin, LineIndex, Revision, TextRange, Utf16Column,
};
use kode_core::{
    commands::EditKind,
    document::{Document, TextPosition, Utf16Position},
};
use std::{path::PathBuf, sync::Arc};

fn document(text: &str) -> Document {
    Document::new(
        DocumentId(1),
        DocumentOrigin::File(PathBuf::from("synthetic.txt")),
        None,
        text,
    )
}

#[test]
fn empty_trailing_lines_and_raw_crlf_have_exact_source_content() {
    for (text, lines) in [
        ("", 1),
        ("one", 1),
        ("one\n", 2),
        ("one\n\n", 3),
        ("one\r\ntwo\r\n", 3),
        ("a\rb\u{2028}c", 1),
    ] {
        let snapshot = document(text).snapshot();
        assert_eq!(snapshot.text().as_ref(), text);
        assert_eq!(snapshot.len_lines(), lines);
    }
    let snapshot = document("one\r\ntwo").snapshot();
    assert_eq!(snapshot.line_content(LineIndex(0)).unwrap(), "one");
    assert_eq!(snapshot.line_end(LineIndex(0)).unwrap(), ByteOffset(3));
    assert_eq!(snapshot.line_start(LineIndex(1)).unwrap(), ByteOffset(5));
}

#[test]
fn every_scalar_boundary_roundtrips_bytes_scalars_lines_and_utf16() {
    for text in [
        "ASCII\n",
        "a\t\x01😀中e\u{301}\nlast",
        "a\r\nb\r\n",
        "",
        "a\rb\u{2028}c",
    ] {
        let snapshot = document(text).snapshot();
        for byte in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let byte = ByteOffset(byte);
            let scalar = snapshot.byte_to_char(byte).unwrap();
            assert_eq!(snapshot.char_to_byte(scalar).unwrap(), byte);
            let position = snapshot.byte_to_position(byte).unwrap();
            assert_eq!(snapshot.position_to_byte(position).unwrap(), byte);
            let utf16 = snapshot.byte_to_utf16(byte).unwrap();
            assert_eq!(snapshot.utf16_to_byte_clamped(utf16), byte);
        }
    }
}

#[test]
fn legacy_surrogate_interiors_and_out_of_range_positions_clamp_safely() {
    let snapshot = document("a😀b\n").snapshot();
    assert_eq!(
        snapshot.utf16_to_byte_clamped(Utf16Position {
            line: LineIndex(0),
            column: Utf16Column(2)
        }),
        ByteOffset(1)
    );
    assert_eq!(
        snapshot.utf16_to_byte_clamped(Utf16Position {
            line: LineIndex(0),
            column: Utf16Column(3)
        }),
        ByteOffset(5)
    );
    assert_eq!(snapshot.clamp_byte(ByteOffset(3)), ByteOffset(1));
    assert_eq!(snapshot.clamp_byte(ByteOffset(100)), ByteOffset(7));
    assert_eq!(
        snapshot.utf16_to_byte_clamped(Utf16Position {
            line: LineIndex(100),
            column: Utf16Column(100)
        }),
        ByteOffset(7)
    );
    assert!(
        snapshot
            .position_to_byte(TextPosition {
                line: LineIndex(99),
                column: CharOffset(0)
            })
            .is_err()
    );
    assert!(snapshot.line_end(LineIndex(99)).is_err());
}

#[test]
fn invalid_stale_readonly_and_noop_edits_do_not_mutate_or_advance_revision() {
    let mut doc = document("a😀b");
    let invalid = TextRange {
        start: ByteOffset(2),
        end: ByteOffset(5),
    };
    assert!(
        doc.apply_edit(Revision(0), invalid, "x", EditKind::Typing)
            .is_err()
    );
    assert_eq!(doc.revision(), Revision(0));
    assert!(
        doc.apply_edit(
            Revision(9),
            TextRange::caret(ByteOffset(0)),
            "x",
            EditKind::Typing
        )
        .is_err()
    );
    assert_eq!(
        doc.apply_edit(
            Revision(0),
            TextRange::new(ByteOffset(1), ByteOffset(5)).unwrap(),
            "😀",
            EditKind::Typing
        )
        .unwrap(),
        None
    );
    doc.set_read_only(true);
    assert!(
        doc.apply_edit(
            Revision(0),
            TextRange::caret(ByteOffset(0)),
            "x",
            EditKind::Typing
        )
        .is_err()
    );
    assert!(!doc.is_dirty());
}

#[test]
fn snapshots_are_immutable_share_flattening_and_supply_slices_without_flattening() {
    let mut doc = document("a😀b\nsecond");
    let before = doc.snapshot();
    let shared = doc.snapshot();
    assert!(Arc::ptr_eq(&before.text(), &shared.text()));
    doc.apply_edit(
        Revision(0),
        TextRange::new(ByteOffset(1), ByteOffset(5)).unwrap(),
        "X",
        EditKind::Programmatic,
    )
    .unwrap();
    assert_eq!(before.text().as_ref(), "a😀b\nsecond");
    assert_eq!(doc.snapshot().text().as_ref(), "aXb\nsecond");
    assert!(doc.is_dirty());
    let snapshot = doc.snapshot();
    assert_eq!(
        snapshot
            .slice(TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap())
            .unwrap(),
        "aXb"
    );
    assert_eq!(snapshot.chunks().collect::<String>(), "aXb\nsecond");
    assert_eq!(before.to_service_snapshot().revision, Revision(0));
    assert!(
        before
            .slice(TextRange {
                start: ByteOffset(2),
                end: ByteOffset(5)
            })
            .is_err()
    );
}

#[test]
fn large_and_single_long_line_fixtures_use_chunked_access() {
    for text in [
        include_str!("../../../fixtures/editor/long-line.txt"),
        include_str!("../../../fixtures/editor/many-lines.txt"),
    ] {
        let snapshot = document(text).snapshot();
        assert_eq!(snapshot.len_bytes(), text.len());
        assert!(snapshot.chunks().count() > 1);
        assert_eq!(
            snapshot
                .char_to_byte(CharOffset(snapshot.len_chars()))
                .unwrap(),
            ByteOffset(text.len())
        );
    }
}
