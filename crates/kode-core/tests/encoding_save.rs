use kode_core::{ByteOffset, DocumentId, DocumentOrigin, Revision, TextRange};
use kode_core::{
    commands::EditKind,
    document::{
        Document,
        encoding::{LineEnding, decode, encode},
    },
};
use std::path::PathBuf;

#[test]
fn utf8_bom_newlines_and_no_final_newline_roundtrip_exactly() {
    for bytes in [
        include_bytes!("../../../fixtures/editor/empty.txt").as_slice(),
        include_bytes!("../../../fixtures/editor/no-final-newline.txt"),
        include_bytes!("../../../fixtures/editor/lf.txt"),
        include_bytes!("../../../fixtures/editor/crlf.txt"),
        include_bytes!("../../../fixtures/editor/bom.txt"),
        b"one\r\ntwo\n",
    ] {
        let decoded = decode(bytes).unwrap();
        assert_eq!(encode(&decoded.text, decoded.format), bytes);
    }
    assert_eq!(
        decode(b"a\r\nb\n").unwrap().format.line_ending,
        LineEnding::Mixed
    );
    assert!(decode(&[0xff, 0xfe, 0x00]).is_err());
}

#[test]
fn older_save_completion_keeps_newer_edits_dirty_and_never_rewinds_content() {
    let mut doc = Document::new(
        DocumentId(1),
        DocumentOrigin::File(PathBuf::from("synthetic.txt")),
        None,
        "original",
    );
    doc.apply_edit(
        Revision(0),
        TextRange::caret(ByteOffset(8)),
        "A",
        EditKind::Typing,
    )
    .unwrap();
    let saved = doc.snapshot();
    doc.apply_edit(
        Revision(1),
        TextRange::caret(ByteOffset(9)),
        "B",
        EditKind::Typing,
    )
    .unwrap();
    doc.acknowledge_save(&saved).unwrap();
    assert_eq!(doc.snapshot().text().as_ref(), "originalAB");
    assert!(doc.is_dirty());
    assert_eq!(doc.saved_revision(), Revision(1));
    let newest = doc.snapshot();
    doc.acknowledge_save(&newest).unwrap();
    assert!(!doc.is_dirty());
    assert!(doc.acknowledge_save(&saved).is_err());
}
