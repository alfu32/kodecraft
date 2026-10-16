use kode_platform::jobs::CancellationToken;
use kode_rest::format::{language_for_content_type, pretty_print};
use std::borrow::Cow;
fn pretty<'a>(content: &'a str, kind: &str) -> Cow<'a, str> {
    pretty_print(content, Some(kind), true, &CancellationToken::new())
}
#[test]
fn original_content_types_and_pretty_toggle_leave_invalid_and_raw_content_unchanged() {
    for (kind, language) in [
        ("application/json; charset=utf-8", "json"),
        (" Application/Problem+JSON ", "json"),
        ("text/javascript", "javascript"),
        ("application/problem+xml", "xml"),
        ("application/xhtml+xml", "html"),
        ("text/yaml", "yaml"),
        ("application/graphql", "graphql"),
    ] {
        assert_eq!(language_for_content_type(Some(kind)), Some(language));
    }
    assert_eq!(
        language_for_content_type(Some("application/octet-stream")),
        None
    );
    assert_eq!(language_for_content_type(None), None);
    let raw = "{\"ok\":true}";
    assert!(pretty(raw, "application/json").contains('\n'));
    assert!(matches!(
        pretty_print(
            raw,
            Some("application/json"),
            false,
            &CancellationToken::new()
        ),
        Cow::Borrowed(_)
    ));
    for (invalid, kind) in [
        ("{bad", "application/json"),
        ("{\"x\":1} trailing", "application/json"),
        ("<root><unclosed></root>", "application/xml"),
        ("const x = {;", "text/javascript"),
    ] {
        assert_eq!(pretty(invalid, kind), invalid);
    }
    assert_eq!(raw, "{\"ok\":true}");
}
#[test]
fn json_formatting_preserves_duplicates_exact_numbers_order_unicode_and_escape_spelling() {
    let raw = r#" {"x":123456789012345678901234567890,"x":1.23456789012345678901234567890,"z":[{},[],"🚀 \u0061 \\" ]} "#;
    let text = pretty(raw, "application/problem+json");
    assert!(text.contains('\n'));
    assert_eq!(text.matches("\"x\"").count(), 2);
    assert!(text.contains("123456789012345678901234567890"));
    assert!(text.contains("1.23456789012345678901234567890"));
    assert!(text.contains(r#""🚀 \u0061 \\""#));
    assert!(text.contains("{}"));
    assert!(text.contains("[]"));
    assert_eq!(pretty(&text, "application/json"), text);
}
#[test]
fn xml_indents_element_only_content_and_keeps_attributes_cdata_entities_and_mixed_text() {
    let raw = "<?xml version='1.0'?><root attr='a>b'><child x='&amp;'>ok</child><!--comment--><empty/></root>";
    let text = pretty(raw, "application/xml");
    assert_eq!(
        text,
        "<?xml version='1.0'?><root attr='a>b'>\n  <child x='&amp;'>ok</child>\n  <!--comment-->\n  <empty/>\n</root>"
    );
    for raw in [
        "<p>Hello <b>world</b>!</p>",
        "<r> <a/></r>",
        "<r xml:space='preserve'> <a><b/></a> </r>",
        "<r><![CDATA[ \n ]]><a/></r>",
        "<!DOCTYPE r [<!ENTITY value 'untouched'>]><r>&value;</r>",
    ] {
        assert_eq!(pretty(raw, "application/xml"), raw);
    }
}
#[test]
fn html_handles_void_tags_but_preserves_mixed_content_scripts_styles_and_preformatted_text() {
    let raw =
        "<!doctype html><html><head><title>Title</title></head><body><br><p>Text</p></body></html>";
    let text = pretty(raw, "text/html");
    assert!(text.contains("\n  <head>"), "{text}");
    assert!(text.contains("<br>"));
    assert!(!text.contains("</br>"));
    assert_eq!(pretty(&text, "text/html"), text);
    for raw in [
        "<p>Hello <b>world</b>!</p>",
        "<pre> \n <b> spaced </b> </pre>",
        "<script>const text='<p>';\n</script>",
        "<style>a>b { color:red; }</style>",
    ] {
        assert_eq!(pretty(raw, "text/html"), raw);
    }
}
#[test]
fn javascript_keeps_strings_comments_regex_templates_and_automatic_semicolon_boundaries() {
    let raw = r#"function run(){const pattern=/[{};,]/g;const text="{;},";const template=`${x};{}`;return {a:1,b:2};}"#;
    let text = pretty(raw, "text/javascript");
    assert!(text.contains('\n'), "{text}");
    for literal in [r#"/[{};,]/g"#, r#""{;},""#, r#"`${x};{}`"#] {
        assert!(text.contains(literal), "{text}");
    }
    let asi = "function run(){return\n{x:1}\n}";
    let text = pretty(asi, "text/javascript");
    assert!(text.contains("return\n"));
    let comments = "function run(){// {};\nreturn 1; /* {} */}";
    let text = pretty(comments, "text/javascript");
    assert!(text.contains("// {};\n"));
    assert!(text.contains("/* {} */"));
}
#[test]
fn yaml_scalars_limits_and_cancellation_never_rewrite_raw_data() {
    assert_eq!(pretty("key: value   \n", "text/yaml"), "key: value \n");
    assert_eq!(
        pretty("key: value   \r\nnext: 1  \r\n", "application/yaml"),
        "key: value\r\nnext: 1\r\n"
    );
    for raw in [
        "key: |\n  text  \n",
        "key: >\n  text  \n",
        "key: 'text  '  \n",
        "x: [unclosed  \n",
    ] {
        assert_eq!(pretty(raw, "text/yaml"), raw);
    }
    let raw = "{\"ok\":true}";
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        pretty_print(raw, Some("application/json"), true, &cancel),
        raw
    );
    let oversized = format!("\"{}\"", "a".repeat(10 * 1024 * 1024));
    assert!(matches!(
        pretty(&oversized, "application/json"),
        Cow::Borrowed(_)
    ));
    let deep = format!("{}0{}", "[".repeat(97), "]".repeat(97));
    assert!(matches!(
        pretty(&deep, "application/json"),
        Cow::Borrowed(_)
    ));
}
