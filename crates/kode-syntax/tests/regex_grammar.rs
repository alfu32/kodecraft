use indexmap::IndexMap;
use kode_core::LineIndex;
use kode_syntax::regex_grammar::{RegexGrammarDefinition, RegexProvider, load_directory};
#[test]
fn original_ordered_qualifier_and_word_boundary_cases_preserve_all_source_punctuation() {
    let definition = RegexGrammarDefinition {
        language: "demo".into(),
        extensions: vec![],
        tokens: IndexMap::from([
            ("keyword.control".into(), r"\b(if|else)\b".into()),
            ("identifier".into(), "[A-Za-z_][A-Za-z0-9_]*".into()),
        ]),
    };
    let provider = RegexProvider::new([definition]).unwrap();
    assert!(
        provider.diagnostics.is_empty(),
        "{:?}",
        provider.diagnostics
    );
    let tokens = provider
        .tokens_for_lines(LineIndex(0), &["if foo else"], "demo", || false)
        .unwrap();
    assert_eq!(
        tokens
            .iter()
            .map(|token| token.class.as_str())
            .collect::<Vec<_>>(),
        ["keyword.control", "identifier", "keyword.control"]
    );
    assert_eq!(
        tokens
            .iter()
            .map(|token| token.line_range.start.0)
            .collect::<Vec<_>>(),
        [0, 3, 7]
    );
    assert_eq!(
        tokens
            .iter()
            .map(|token| token.line_range.end.0)
            .collect::<Vec<_>>(),
        [2, 6, 11]
    );
    let source = "a+b[3].";
    let punctuation = provider
        .tokens_for_lines(LineIndex(0), &[source], "text", || false)
        .unwrap();
    assert_eq!(
        punctuation
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>(),
        ["+", "[", "]", "."]
    );
    for token in punctuation {
        assert_eq!(
            &source[token.line_range.start.0..token.line_range.end.0],
            token.text
        );
    }
}
#[test]
fn original_directory_extension_and_line_number_case_loads_real_json_and_rejects_escape() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("index.json"),
        r#"{"languages":["templang","../outside"]}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("templang.json"),r#"{"language":"templang","extensions":[".tmp"],"tokens":{"number":"\\d+","word":"[a-z]+"},"futureField":"retained by owner's file"}"#).unwrap();
    let loaded = load_directory(root.path()).unwrap();
    assert_eq!(loaded.definitions.len(), 1);
    assert_eq!(loaded.diagnostics.len(), 1);
    let provider = RegexProvider::new(loaded.definitions).unwrap();
    assert_eq!(provider.language_for_extension(".TMP"), Some("templang"));
    let tokens = provider
        .tokens_for_lines(LineIndex(5), &["abc 123"], "templang", || false)
        .unwrap();
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0].line, LineIndex(5));
    assert_eq!(tokens[0].class, "word");
    assert_eq!(tokens[0].text, "abc");
    assert_eq!(tokens[1].text, "123");
}
