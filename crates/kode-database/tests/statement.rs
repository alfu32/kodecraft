use kode_database::statement::{self, Dialect};
#[test]
fn original_semicolons_inside_strings_and_comments_and_current_selection_cases() {
    let text =
        "select ';';\n\nselect 1; -- comment ;\n\nselect 'abc;def';\n\n/*\n ;\n*/\nselect 2;";
    assert_eq!(
        statement::statements(text, Dialect::Standard)
            .map(|span| span.sql)
            .collect::<Vec<_>>(),
        [
            "select ';'",
            "select 1",
            "-- comment ;\n\nselect 'abc;def'",
            "/*\n ;\n*/\nselect 2"
        ]
    );
    let text = "select 1;\nselect 'a;b';\nselect 3;";
    assert_eq!(
        statement::locate(text, 18, None, Dialect::Standard)
            .unwrap()
            .unwrap()
            .sql,
        "select 'a;b'"
    );
    let text = "select 1;\nselect 2;";
    assert_eq!(
        statement::locate(text, 0, Some(10..18), Dialect::Standard)
            .unwrap()
            .unwrap()
            .sql,
        "select 2"
    );
}
#[test]
fn actual_duplicate_selection_offsets_and_unicode_ranges_are_preserved_without_searching_for_text()
{
    let text = "select '界😀';\r\nselect '界😀';";
    let start = text.rfind("select").unwrap();
    let selected = start..text.len() - 1;
    let target = statement::locate(text, 0, Some(selected.clone()), Dialect::Standard)
        .unwrap()
        .unwrap();
    assert_eq!(target.range, selected);
    assert_eq!(target.sql, "select '界😀'");
    assert!(statement::locate(text, 9, None, Dialect::Standard).is_err());
}
#[test]
fn eof_after_a_terminator_never_falls_back_to_reexecuting_the_whole_script() {
    let text = "insert into t values(1); insert into t values(2);";
    let span = statement::locate(text, text.len(), None, Dialect::Standard)
        .unwrap()
        .unwrap();
    assert_eq!(span.sql, "insert into t values(2)");
    assert_ne!(span.range, 0..text.len());
    let comment = "select 1; -- only a comment ;";
    assert!(
        statement::locate(comment, comment.len(), None, Dialect::Standard)
            .unwrap()
            .is_none()
    );
    assert!(statement::script("/* ; */ -- only comment", Dialect::Standard).is_none());
}
#[test]
fn quoted_identifiers_dollar_bodies_nested_comments_and_dialect_escapes_are_atomic() {
    for text in [
        "select 'a'';b';select 2;",
        "select \"a\"\";b\";select 2;",
        "select `a``;b`;select 2;",
        "select [a]];b];select 2;",
        "select $$a;b$$;select 2;",
        "select $tag$a;b$tag$;select 2;",
        "select $界$a;b$界$;select 2;",
        "/* outer /* inner ; */ ; */ select 1;select 2;",
        "select E'a\\';b';select 2;",
    ] {
        let spans: Vec<_> = statement::statements(text, Dialect::Standard).collect();
        assert_eq!(spans.len(), 2, "{text}");
        assert_eq!(spans[1].sql, "select 2");
    }
    assert_eq!(
        statement::statements("select 'a\\';b';select 2;", Dialect::MySql).count(),
        2
    );
    assert!(statement::locate("select 'x'", 0, Some(8..100), Dialect::Standard).is_err());
}
#[test]
fn dollar_signs_in_identifiers_and_unicode_whitespace_do_not_merge_following_statements() {
    for (text, dialect) in [
        ("select foo$$;select 2;", Dialect::Standard),
        ("select 界$tag$;select 2;", Dialect::Standard),
        ("select $tag$;select 2;", Dialect::MySql),
    ] {
        let spans: Vec<_> = statement::statements(text, dialect).collect();
        assert_eq!(spans.len(), 2, "{text}");
        assert_eq!(spans[1].sql, "select 2");
    }
    assert!(statement::script("\u{a0}/* comment */\u{2003}", Dialect::Standard).is_none());
}
#[test]
fn mysql_default_quotes_hash_comments_dash_arithmetic_and_version_comments_keep_boundaries() {
    for text in [
        "select \"a\\\";b\";select 2;",
        "select 1--2;select 2;",
        "# comment ;\nselect 1;select 2;",
        "/* comment /* ignored */ select 1;select 2;",
    ] {
        let spans: Vec<_> = statement::statements(text, Dialect::MySql).collect();
        assert_eq!(spans.len(), 2, "{text}");
        assert_eq!(spans[1].sql, "select 2");
    }
    assert!(statement::script("# comment;", Dialect::MySql).is_none());
    assert!(statement::script("/*!40101 SET @a=1 */;", Dialect::MySql).is_some());
}
