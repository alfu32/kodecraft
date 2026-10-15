use kode_core::{ByteOffset, TextRange, editor::search_regex::SearchRegex};
#[test]
fn ascii_boundaries_with_lookaround_and_named_captures_keep_java_byte_ranges() {
    let pattern = SearchRegex::compile(r"(?<word>\bfoo)(?=\.)").unwrap();
    let text = "éfoo. xfoo. foo.";
    let mut matches = vec![];
    pattern
        .for_each_named(text, &["word", "absent"], |whole, groups| {
            assert_eq!(Some(whole), groups[0]);
            assert!(groups[1].is_none());
            matches.push(whole);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        matches,
        vec![
            TextRange::new(ByteOffset(2), ByteOffset(5)).unwrap(),
            TextRange::new(ByteOffset(13), ByteOffset(16)).unwrap()
        ]
    );
    let pattern = SearchRegex::compile(r"\Bfoo(?=\.)").unwrap();
    let mut starts = vec![];
    pattern
        .for_each(text, |range| {
            starts.push(range.start.0);
            Ok(())
        })
        .unwrap();
    assert_eq!(starts, vec![8]);
}
