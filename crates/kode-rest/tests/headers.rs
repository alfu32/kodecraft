use kode_rest::headers::{complete, duplicate_lines};
#[test]
fn header_completion_handles_hyphens_disabled_lines_duplicates_and_value_focus() {
    let text = "Accept: application/json\nContent-T\n# Authorization: disabled\nX-Request-ID: one";
    let cursor = text.find("Content-T").unwrap() + "Content-T".len();
    let result = complete(text, cursor).unwrap();
    assert_eq!(&text[result.prefix], "Content-T");
    assert_eq!(result.names, ["Content-Type"]);
    assert!(complete(text, "Accept: app".len()).is_none());
    let empty = complete(text, "Accept: application/json\n".len()).unwrap();
    assert!(!empty.names.contains(&"Accept"));
    assert!(empty.names.contains(&"Authorization"));
    assert!(!empty.names.contains(&"X-Request-ID"));
    let result = complete("# content-t", 11).unwrap();
    assert_eq!(result.names, ["Content-Type"]);
    assert_eq!(result.prefix, 2..11);
    assert!(complete("🚀", 1).is_none());
    assert!(complete(text, text.len() + 1).is_none());
}
#[test]
fn duplicate_header_warnings_match_the_source_earlier_occurrences_case_insensitively() {
    let text = "Accept: one\naccept: two\n# ACCEPT: disabled\nX-Unique: yes\nInvalid line\n: invalid\nAccept-Language: en";
    assert_eq!(duplicate_lines(text), [0, 1]);
    assert!(duplicate_lines("X-Test: a\nX-Other: b").is_empty());
}
