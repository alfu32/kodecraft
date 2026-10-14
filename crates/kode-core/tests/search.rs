use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Selection,
    commands::EditKind,
    document::Document,
    editor::{
        search::{SearchReport, SearchResults, expand_replacement, scan},
        selection::CaretState,
    },
};

fn document(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "search".into(),
            label: "text".into(),
        },
        None,
        text,
    )
}
fn report(text: &str, pattern: &str) -> SearchReport {
    let report = scan(&document(text).snapshot(), pattern, || false).unwrap();
    assert!(report.error.is_none(), "{pattern}: {:?}", report.error);
    report
}
fn matches(text: &str, pattern: &str) -> Vec<String> {
    report(text, pattern)
        .matches
        .iter()
        .map(|matched| text[matched.range.start.0..matched.range.end.0].to_string())
        .collect()
}

#[test]
fn synthetic_java21_oracle_matches_raw_utf16_ranges() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/editor/search-java21.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let query = case["query"].as_str().unwrap();
        let report = report(text, query);
        let actual: Vec<Vec<usize>> = report
            .matches
            .iter()
            .map(|matched| {
                vec![
                    text[..matched.range.start.0].encode_utf16().count(),
                    text[..matched.range.end.0].encode_utf16().count(),
                ]
            })
            .collect();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            case["ranges_utf16"],
            "{query} in {text:?}"
        );
    }
}

#[test]
fn source_search_counts_cycles_captures_replacements_and_grouped_undo() {
    let mut document = document("abc123 xyz\nabc123");
    let mut caret = CaretState::default();
    let mut results = SearchResults::default();
    let pattern = "(abc)(123)";
    results.publish(
        scan(&document.snapshot(), pattern, || false).unwrap(),
        &document.snapshot(),
        pattern,
    );
    assert_eq!(results.report.as_ref().unwrap().matches.len(), 2);
    assert_eq!(results.active, None);
    results.next(&document.snapshot(), &mut caret).unwrap();
    assert_eq!(
        caret.selection,
        Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(6)
        }
    );
    results.next(&document.snapshot(), &mut caret).unwrap();
    assert_eq!(results.active, Some(1));
    results.next(&document.snapshot(), &mut caret).unwrap();
    let selected = caret.selection;
    assert_eq!(
        results
            .replace(&mut document, &mut caret, "$2-$1", true)
            .unwrap(),
        2
    );
    assert_eq!(
        document.snapshot().to_service_snapshot().text.as_ref(),
        "123-abc xyz\n123-abc"
    );
    assert_eq!(document.revision().0, 1);
    assert!(document.undo(&mut caret).unwrap());
    assert_eq!(
        document.snapshot().to_service_snapshot().text.as_ref(),
        "abc123 xyz\nabc123"
    );
    assert_eq!(caret.selection, selected);
}

#[test]
fn replacement_keeps_source_literal_dollars_backslashes_and_missing_group_rules() {
    let report = report("abc123", "(abc)(123)(z)?");
    assert_eq!(
        expand_replacement(
            r"$2-$1-$0-$3-$99-$$-${name}-\$1-$99999999999999999999999999",
            &report.matches[0],
            &report.text
        ),
        r"123-abc-abc123---$$-${name}-\abc-"
    );
    assert_eq!(
        expand_replacement("$١-$２-$𝟚", &report.matches[0], &report.text),
        "abc-123-$𝟚"
    );
}

#[test]
fn java_ascii_defaults_flags_quotes_and_unicode_scalars_are_explicit() {
    assert_eq!(matches("ab中_12𝟚", r"\w+"), ["ab", "_12"]);
    assert_eq!(matches("1𝟚", r"\d+"), ["1"]);
    assert_eq!(matches("1𝟚", r"(?U)\d+"), ["1𝟚"]);
    assert_eq!(matches("kKK", r"(?i)k"), ["k", "K"]);
    assert_eq!(matches("kKK", r"(?iu)k"), ["k", "K", "K"]);
    assert_eq!(matches("aA bB", r"(?i:a)b"), Vec::<String>::new());
    assert_eq!(matches("aAb aAB", r"(?i:aa)b"), ["aAb"]);
    assert_eq!(matches("aAzZ", r"(?i)[a-z]+"), ["aAzZ"]);
    assert_eq!(matches("x A.*B x", r"\QA.*B\E"), ["A.*B"]);
    assert_eq!(matches("😀A", r"\x{1F600}\u0041"), ["😀A"]);
    assert_eq!(matches("aé1", r"\p{Alpha}+"), ["a"]);
    assert_eq!(matches("aé1", r"\p{L}+"), ["aé"]);
}

#[test]
fn java_line_terminators_multiline_dotall_and_final_end_anchors_match() {
    let text = "a\r\nb\rC\u{85}D\u{2028}E\u{2029}F\n";
    assert_eq!(matches(text, "."), ["a", "b", "C", "D", "E", "F"]);
    assert_eq!(matches(text, "(?m)^.$"), ["a", "b", "C", "D", "E", "F"]);
    assert_eq!(matches("abc\r\n", "abc$"), ["abc"]);
    assert_eq!(matches("abc\r\nnext", "abc$"), Vec::<String>::new());
    assert_eq!(matches("a\r\nb", "(?s)a.b"), Vec::<String>::new());
    assert_eq!(matches("a\r\nb", "(?s)a..b"), ["a\r\nb"]);
    assert_eq!(matches("a\rb\n", "(?d)."), ["a", "\r", "b"]);
    assert_eq!(matches("a\r\nb", r"\R"), ["\r\n"]);
}

#[test]
fn lookarounds_backreferences_multiline_ranges_and_zero_width_patterns_progress() {
    assert_eq!(matches("one two oneone", r"(?<=one)one"), ["one"]);
    assert_eq!(matches("abab abac", r"(ab)\1"), ["abab"]);
    assert_eq!(matches("foo bar foo", "foo(?= bar)"), ["foo"]);
    assert_eq!(matches("a\nb", r"a\nb"), ["a\nb"]);
    for pattern in ["^", "$", "(?=a)", "a*", "(?:)"] {
        let report = report("bbb", pattern);
        assert!(report.matches.is_empty());
    }
    assert_eq!(matches("ab", "a*"), ["a"]);
}

#[test]
fn invalid_unsupported_cancelled_stale_and_readonly_search_never_mutates() {
    for pattern in ["[", r"\X", r"\p{javaWhitespace}", "(?x)a", r"\uD800"] {
        let scan = scan(&document("text").snapshot(), pattern, || false).unwrap();
        assert!(scan.error.is_some(), "{pattern}");
        assert!(scan.matches.is_empty());
    }
    assert!(matches!(
        scan(&document("text").snapshot(), "t", || true),
        Err(DomainError::Cancelled)
    ));
    let mut document = document("one one");
    let mut caret = CaretState::default();
    let old = scan(&document.snapshot(), "one", || false).unwrap();
    let mut results = SearchResults::default();
    results.publish(old.clone(), &document.snapshot(), "one");
    document.set_read_only(true);
    assert!(matches!(
        results.replace(&mut document, &mut caret, "1", true),
        Err(DomainError::ReadOnly)
    ));
    document.set_read_only(false);
    document
        .apply_edit(
            document.revision(),
            old.matches[0].range,
            "1",
            EditKind::Typing,
        )
        .unwrap();
    assert!(!results.publish(old, &document.snapshot(), "one"));
    assert!(
        results
            .next(&document.snapshot(), &mut caret)
            .unwrap()
            .is_none()
    );
    assert!(
        results
            .replace(&mut document, &mut caret, "1", true)
            .is_err()
    );
    let revision = document.revision();
    results.publish(
        scan(&document.snapshot(), "one", || false).unwrap(),
        &document.snapshot(),
        "one",
    );
    assert_eq!(
        results
            .replace(&mut document, &mut caret, "one", true)
            .unwrap(),
        1
    );
    assert_eq!(document.revision(), revision); // identical replacement makes no false history
}
