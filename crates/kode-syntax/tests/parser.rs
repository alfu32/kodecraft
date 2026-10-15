use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, TextRange, WorkspaceGeneration, commands::EditKind,
    document::Document,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{parser::ParserSession, registry::GrammarRegistry};
fn document(language: &str, text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: language.into(),
            label: language.into(),
        },
        Some(language.into()),
        text,
    )
}
#[test]
fn kotlin_and_c_parse_real_unicode_fields_and_incremental_edits_equal_fresh_trees() {
    let registry = GrammarRegistry::compiled();
    let token = CancellationToken::new();
    for (language, initial) in [
        (
            "kotlin",
            "class Café(val name: String) {\n fun greet(): String {\n return name\n }\n}\n",
        ),
        (
            "c",
            "/* 😀 */\r\nstruct Customer { int value; };\r\nint use(void) { return 2; }\r\n",
        ),
    ] {
        let grammar = registry.get(language).unwrap();
        let mut session = ParserSession::new(grammar.clone()).unwrap();
        let mut doc = document(language, initial);
        let before = session
            .parse(&doc.snapshot(), WorkspaceGeneration(1), &token)
            .unwrap();
        assert!(
            !before.has_errors(),
            "{language}: {}",
            before.root_node().to_sexp()
        );
        for (from, to) in [
            ("Café", "Cafè"),
            ("😀", "😁"),
            ("name", "title"),
            ("return", "/* 😀 */ return"),
            ("2", "300"),
        ] {
            let source = doc.snapshot().text();
            let Some(start) = source.find(from) else {
                continue;
            };
            doc.apply_edit(
                doc.revision(),
                TextRange::new(ByteOffset(start), ByteOffset(start + from.len())).unwrap(),
                to,
                EditKind::Typing,
            )
            .unwrap();
            let parsed = session
                .parse(&doc.snapshot(), WorkspaceGeneration(1), &token)
                .unwrap();
            let fresh = ParserSession::new(grammar.clone())
                .unwrap()
                .parse(&doc.snapshot(), WorkspaceGeneration(1), &token)
                .unwrap();
            assert_eq!(parsed.root_node().to_sexp(), fresh.root_node().to_sexp());
            assert_eq!(parsed.lexical(), fresh.lexical());
            assert_eq!(parsed.diagnostics(), fresh.diagnostics());
            assert_eq!(before.source(), initial);
            assert_eq!(parsed.source(), doc.snapshot().text().as_ref());
            for span in parsed.lexical() {
                span.range.validate(parsed.source()).unwrap();
            }
        }
    }
}

#[test]
fn cancellation_and_time_budgets_preserve_old_trees_and_allow_later_recovery() {
    use std::time::Duration;
    let registry = GrammarRegistry::compiled();
    let grammar = registry.get("c").unwrap();
    let mut session = ParserSession::new(grammar)
        .unwrap()
        .with_budget(Duration::ZERO);
    let doc = document("c", "int value = 2;\n");
    assert!(
        session
            .parse(
                &doc.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new()
            )
            .is_err()
    );
    let mut session = session.with_budget(Duration::from_secs(2));
    let before = session
        .parse(
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        session.parse(&doc.snapshot(), WorkspaceGeneration(1), &cancel),
        Err(kode_core::DomainError::Cancelled)
    ));
    let next = session
        .parse(
            &doc.snapshot(),
            WorkspaceGeneration(2),
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(next.generation(), WorkspaceGeneration(2));
    assert_eq!(before.generation(), WorkspaceGeneration(1));
    assert_eq!(before.source(), next.source());
}
#[tokio::test]
async fn missing_grammar_has_real_keyword_fallback_status_through_bounded_workers() {
    use kode_platform::jobs::{MonotonicClock, Notifications, OwnerId, Scheduler};
    use kode_syntax::{
        keywords::KeywordProvider,
        model::GrammarMode,
        service::{SyntaxSession, schedule_highlight, tag_for},
    };
    use std::sync::{Arc, Mutex};
    let doc = document("kotlin", "fun example() { return; }\n");
    let tag = tag_for(&doc.snapshot(), WorkspaceGeneration(1)).unwrap();
    let session = Arc::new(Mutex::new(SyntaxSession::new(
        "kotlin",
        &GrammarRegistry::empty(),
        Arc::new(KeywordProvider::compiled().unwrap()),
    )));
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_highlight(&mut scheduler, OwnerId(1), tag, session, doc.snapshot()).unwrap();
    let completion = scheduler.next().await.unwrap();
    assert_eq!(completion.tag, tag);
    let report = completion.result.unwrap();
    assert_eq!(report.status.mode, GrammarMode::Keywords);
    assert!(report.status.message.unwrap().contains("unavailable"));
    assert!(report.parsed.is_none());
    assert!(!report.spans.is_empty());
    scheduler.shutdown().await;
}

#[test]
fn every_enabled_grammar_matches_its_pinned_abi_node_fields_and_unicode_byte_fixture() {
    use std::collections::BTreeSet;
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/languages/parsers/node-fields.json"
    ))
    .unwrap();
    let registry = GrammarRegistry::compiled();
    assert_eq!(
        registry.entries().count(),
        fixture["bindings"].as_array().unwrap().len()
    );
    for binding in fixture["bindings"].as_array().unwrap() {
        let id = binding["grammar"]["language"].as_str().unwrap();
        let grammar = registry.get(id).unwrap();
        assert_eq!(
            serde_json::to_value(grammar.metadata()).unwrap(),
            binding["grammar"]
        );
        let source = binding["source"].as_str().unwrap();
        let doc = document(id, source);
        let parsed = ParserSession::new(grammar)
            .unwrap()
            .parse(
                &doc.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new(),
            )
            .unwrap();
        assert!(
            !parsed.has_errors(),
            "{id}: {}",
            parsed.root_node().to_sexp()
        );
        assert_eq!(parsed.root_node().kind(), binding["root"].as_str().unwrap());
        let mut actual = BTreeSet::new();
        let mut cursor = parsed.tree().walk();
        loop {
            let node = cursor.node();
            actual.insert((
                node.kind().to_owned(),
                node.parent().map(|parent| parent.kind().to_owned()),
                cursor.field_name().map(str::to_owned),
                node.start_byte(),
                node.end_byte(),
            ));
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break;
                }
            }
            if cursor.node().id() == parsed.root_node().id() {
                break;
            }
        }
        for node in binding["nodes"].as_array().unwrap() {
            let record = (
                node["kind"].as_str().unwrap().to_owned(),
                node["parent"].as_str().map(str::to_owned),
                node["field"].as_str().map(str::to_owned),
                node["range"]["start"].as_u64().unwrap() as usize,
                node["range"]["end"].as_u64().unwrap() as usize,
            );
            assert!(
                actual.contains(&record),
                "{id}: missing node/field {record:?}"
            );
            assert_eq!(&source[record.3..record.4], node["text"].as_str().unwrap());
        }
    }
}
