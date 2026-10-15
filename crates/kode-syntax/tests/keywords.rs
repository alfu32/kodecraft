use kode_core::LineIndex;
use kode_syntax::keywords::KeywordProvider;
#[test]
fn original_keyword_completion_markup_and_css_cases_keep_byte_ranges_and_colors() {
    let provider = KeywordProvider::compiled().unwrap();
    for keyword in ["trait", "impl", "Self", "macro"] {
        assert!(provider.keywords("rust").iter().any(|word| word == keyword));
    }
    let line = "function App() { return <Button className=\"primary\" /> }";
    let tokens = provider
        .tokens_for_lines(LineIndex(0), &[line], "jsx", || false)
        .unwrap();
    for (text, class) in [
        ("function", "keyword"),
        ("Button", "tag"),
        ("className", "css.class"),
        ("\"primary\"", "string"),
    ] {
        assert!(
            tokens
                .iter()
                .any(|token| token.text == text && token.class == class),
            "{text}: {tokens:?}"
        );
    }
    let xml = "😀<rest access-permission=\"app.access\">";
    let tokens = provider
        .tokens_for_lines(LineIndex(5), &[xml], "xml", || false)
        .unwrap();
    for (text, class) in [
        ("rest", "tag"),
        ("access-permission", "attribute"),
        ("\"app.access\"", "string"),
    ] {
        let token = tokens
            .iter()
            .find(|token| token.text == text && token.class == class)
            .unwrap();
        assert_eq!(token.line, LineIndex(5));
        assert_eq!(&xml[token.line_range.start.0..token.line_range.end.0], text);
    }
    let css = provider
        .tokens_for_lines(LineIndex(0), &[".myBox { color: red; }"], "css", || false)
        .unwrap();
    assert!(
        css.iter()
            .any(|token| token.text == ".myBox" && token.class == "css.class")
    );
    assert!(
        css.iter()
            .any(|token| token.text.contains("color") && token.class == "keyword")
    );
}
#[test]
fn all_shipped_patterns_compile_and_cplus_systemd_aliases_have_real_tokens() {
    let provider = KeywordProvider::compiled().unwrap();
    assert!(
        provider.diagnostics.is_empty(),
        "{:?}",
        provider.diagnostics
    );
    assert!(provider.languages().len() > 200);
    assert!(
        !provider
            .tokens_for_lines(
                LineIndex(0),
                &["class Example { public: return; }"],
                "c++",
                || false
            )
            .unwrap()
            .is_empty()
    );
    assert!(
        !provider
            .tokens_for_lines(
                LineIndex(0),
                &["pub fn example() { return; }"],
                "c+",
                || false
            )
            .unwrap()
            .is_empty()
    );
    assert!(
        !provider
            .tokens_for_lines(
                LineIndex(0),
                &["[Service]", "ExecStart=/bin/true"],
                "systemd",
                || false
            )
            .unwrap()
            .is_empty()
    );
}
#[test]
fn invalid_patterns_report_source_line_and_color_errors_never_panic() {
    let provider = KeywordProvider::from_resources(
        "broken=(\nokay=\\b(if|else)\\b\nmissing delimiter\n",
        "keyword=#中aaa\ndefault=#123456",
    )
    .unwrap();
    assert_eq!(provider.diagnostics.len(), 2);
    assert_eq!(provider.diagnostics[0].line, 1);
    assert_eq!(provider.color("keyword"), [0x12, 0x34, 0x56]);
    let tokens = provider
        .tokens_for_lines(LineIndex(0), &["if hello else"], "okay", || false)
        .unwrap();
    assert_eq!(
        tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>(),
        ["if", "else"]
    );
    assert!(
        provider
            .tokens_for_lines(LineIndex(0), &["if"], "okay", || true)
            .is_err()
    );
}
