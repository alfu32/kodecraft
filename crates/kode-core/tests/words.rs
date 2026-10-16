use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, document::Document,
    editor::words::range_at,
};
use std::cell::Cell;

fn document(text: &str) -> Document {
    Document::new(
        DocumentId(8),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "words".into(),
            label: "Words".into(),
        },
        None,
        text,
    )
}

#[test]
fn every_scalar_in_named_unicode_words_selects_exact_bytes_and_excludes_line_ends() {
    for word in [
        "éclair",
        "can't",
        "foo_bar",
        "e\u{301}cole",
        "привет",
        "مرحبا",
        "32.3",
        "שלום",
    ] {
        let text = format!("prefix {word} suffix\r\n");
        let snapshot = document(&text).snapshot();
        let start = "prefix ".len();
        for (offset, _) in word.char_indices() {
            let range = range_at(&snapshot, ByteOffset(start + offset), || false).unwrap();
            assert_eq!(range.start.0, start, "{word:?}");
            assert_eq!(range.end.0, start + word.len(), "{word:?}");
            assert_eq!(snapshot.slice(range).unwrap(), word);
        }
        for offset in [text.len() - 2, text.len() - 1, text.len()] {
            assert!(
                range_at(&snapshot, ByteOffset(offset), || false)
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn whitespace_punctuation_emoji_cjk_empty_and_trailing_lines_have_explicit_segments() {
    for (text, at, selected) in [
        ("one  two", 3, "  "),
        ("one\ttwo", 3, "\t"),
        ("a::b", 1, ":"),
        ("(word)", 0, "("),
        ("(word)", 5, ")"),
        ("go 😀!", 3, "😀"),
        ("世界", 0, "世"),
        ("世界", 3, "界"),
        ("", 0, ""),
        ("x\n\n", 2, ""),
        ("x\n\n", 3, ""),
    ] {
        let snapshot = document(text).snapshot();
        let range = range_at(&snapshot, ByteOffset(at), || false).unwrap();
        assert_eq!(snapshot.slice(range).unwrap(), selected, "{text:?} at {at}");
    }
    assert!(range_at(&document("é").snapshot(), ByteOffset(1), || false).is_err());
}

#[test]
fn long_rope_chunk_word_cancels_and_selects_without_changing_the_document() {
    let word = "e\u{301}".repeat(400_000);
    let doc = document(&format!("prefix {word} suffix\nnext line"));
    let snapshot = doc.snapshot();
    let checks = Cell::new(0);
    assert!(matches!(
        range_at(&snapshot, ByteOffset(7), || {
            checks.set(checks.get() + 1);
            checks.get() > 3
        }),
        Err(DomainError::Cancelled)
    ));
    let range = range_at(&snapshot, ByteOffset(7 + 3000), || false).unwrap();
    assert_eq!((range.start.0, range.end.0), (7, 7 + word.len()));
    assert_eq!(doc.revision().0, 0);
    assert_eq!(doc.history_depth(), (0, 0));
    assert!(matches!(
        range_at(&snapshot, ByteOffset(0), || true),
        Err(DomainError::Cancelled)
    ));
}
