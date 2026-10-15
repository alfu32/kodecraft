use kode_core::{ByteOffset, DocumentId, DocumentOrigin, WorkspaceGeneration, document::Document};
use kode_intelligence::{
    completion::{providers::*, *},
    frontend::{legacy::DefinitionRegistry, registry::SemanticAdapterSession},
    ids::SemanticIds,
    model::*,
    resolver::SemanticResolver,
    snapshot::SemanticSnapshot,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{keywords::KeywordProvider, registry::GrammarRegistry};
use std::{collections::BTreeSet, sync::Arc};
const MODEL: &str = "class Address { val city: String = \"\" }\nclass Customer { val address: Address = Address(); val id: Long = 0; val name: String = \"\"; fun primaryAddress(): Address = Address(); fun save() {} }\nfun getCustomer(): Customer = Customer()\n";
fn prepare(fixtures: &[(&str, &str, &str)]) -> (SemanticSnapshot, CompletionContext, String) {
    let registry = GrammarRegistry::compiled();
    let definitions = DefinitionRegistry::compiled().unwrap();
    let keywords = KeywordProvider::compiled().unwrap();
    let mut deltas = vec![];
    let mut context = None;
    let mut current = String::new();
    for (path, language, text) in fixtures {
        let source = text.replace("/*caret*/", "");
        let doc = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "completion-test".into(),
                key: (*path).into(),
                label: (*path).into(),
            },
            Some((*language).into()),
            &source,
        );
        let report = SemanticAdapterSession::new(&registry, language)
            .extract(
                &definitions,
                &keywords,
                path,
                &doc.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new(),
            )
            .unwrap();
        deltas.push(report.delta);
        if let Some(offset) = text.find("/*caret*/") {
            context = Some(
                CompletionContext::at(
                    SemanticIds::file(path),
                    language,
                    &source,
                    ByteOffset(offset),
                )
                .unwrap(),
            );
            current = source;
        }
    }
    let snapshot = SemanticSnapshot::new(
        1,
        SemanticResolver::resolve(deltas, &CancellationToken::new())
            .unwrap()
            .project,
    )
    .unwrap();
    (snapshot, context.unwrap(), current)
}
fn engine() -> CompletionEngine {
    CompletionEngine::native(Arc::new(KeywordProvider::compiled().unwrap()))
}
fn candidates(
    snapshot: &SemanticSnapshot,
    context: &CompletionContext,
) -> Vec<CompletionCandidate> {
    engine()
        .complete(context, snapshot, 50, &CancellationToken::new())
        .unwrap()
}
fn labels(candidates: &[CompletionCandidate]) -> BTreeSet<&str> {
    candidates.iter().map(|c| c.label.as_str()).collect()
}
fn member_source(expression: &str) -> String {
    format!("{MODEL}\nfun inspect(customer: Customer) {{ {expression}./*caret*/\n}}\n")
}

#[test]
fn basic_cross_file_chained_properties_methods_and_function_call_receivers_are_precise() {
    for expression in [
        "customer.address",
        "customer.primaryAddress()",
        "getCustomer().address",
        "getCustomer().primaryAddress()",
    ] {
        let source = member_source(expression);
        let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
        let result = candidates(&snapshot, &context);
        assert_eq!(labels(&result), BTreeSet::from(["city"]), "{expression}");
        assert!(
            result
                .iter()
                .all(|c| c.member && c.symbol_id.is_some() && c.provider == ProviderKind::Member)
        );
    }
    let use_source = "fun inspect() { val customer = getCustomer(); customer./*caret*/\n}\n";
    let (snapshot, context, _) = prepare(&[
        ("Use.kt", "kotlin", use_source),
        ("Model.kt", "kotlin", MODEL),
    ]);
    assert_eq!(
        labels(&candidates(&snapshot, &context)),
        BTreeSet::from(["address", "id", "name", "primaryAddress", "save"])
    );
}

#[test]
fn cast_parentheses_assignment_union_and_nullable_generic_receivers_preserve_member_rules() {
    let cases = [
        (format!("{MODEL}\nfun inspect(value: Any) {{ (value as Customer)./*caret*/\n}}"), vec!["address", "id", "name", "primaryAddress", "save"]),
        (format!("{MODEL}\nfun inspect(original: Customer) {{ val copied = original; var assigned = null; assigned = copied; ((assigned))./*caret*/\n}}"), vec!["address", "id", "name", "primaryAddress", "save"]),
        ("class Box<T> { val size: Int = 0 }\nfun inspect(box: Box<String>?) { box?./*caret*/\n}".into(), vec!["size"]),
        ("class First { val shared: Int = 0; val onlyFirst: Int = 0 }\nclass Second { val shared: Int = 0; val onlySecond: Int = 0 }\nfun inspect() { var value = First(); value = Second(); value./*caret*/\n}".into(), vec!["shared"]),
    ];
    for (source, expected) in cases {
        let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
        let result = candidates(&snapshot, &context);
        assert_eq!(labels(&result), expected.into_iter().collect());
        if source.contains("class First") {
            assert!(
                result[0].symbol_id.is_none(),
                "union declarations have distinct identities"
            );
        }
    }
}

#[test]
fn this_super_and_external_receivers_filter_visibility_and_unknown_receivers_return_empty() {
    let model = "open class Entity { val id: Int = 0; protected val inherited: Int = 0; private val hiddenBase: Int = 0 }\nclass Customer : Entity() { val name: String = \"\"; private val secret: String = \"\"; fun inspect() { REPLACE\n} }\n";
    for (expression, expected) in [
        ("this", vec!["id", "inherited", "name", "secret", "inspect"]),
        ("super", vec!["id", "inherited"]),
    ] {
        let source = model.replace("REPLACE", &format!("{expression}./*caret*/"));
        let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
        assert_eq!(
            labels(&candidates(&snapshot, &context)),
            expected.into_iter().collect()
        );
    }
    let source = format!(
        "{}\nfun external(customer: Customer) {{ customer./*caret*/\n}}",
        model.replace("REPLACE", "")
    );
    let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
    assert_eq!(
        labels(&candidates(&snapshot, &context)),
        BTreeSet::from(["id", "name", "inspect"])
    );
    for receiver in ["unknown", "missingCall(1)", "unknown.property"] {
        let source = format!("{MODEL}\nfun external(unknown: Any) {{ {receiver}./*caret*/\n}}");
        let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
        assert!(context.member_access);
        assert!(candidates(&snapshot, &context).is_empty(), "{receiver}");
    }
}

#[test]
fn c_pointer_and_typedef_receivers_complete_fields_without_unrelated_workspace_members() {
    let source = "#include \"types.h\"\nvoid use(Customer *ptr) { ptr->/*caret*/; }\n";
    let header = "typedef struct Customer { int id; char *name; } Customer;\nstruct Other { int unrelated; };\n";
    let (snapshot, context, _) = prepare(&[("Use.c", "c", source), ("types.h", "c", header)]);
    assert!(context.member_access);
    assert_eq!(
        labels(&candidates(&snapshot, &context)),
        BTreeSet::from(["id", "name"])
    );
}

#[test]
fn scope_precedence_declaration_order_and_workspace_candidates_do_not_leak_sibling_locals() {
    let source = "val name: Int = 0\nfun separate() { val siblingOnly = 1 }\nfun namedGlobal() {}\nfun inspect(argument: Int) { val name = 1; na/*caret*/; val nameLater = 2 }\n";
    let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", source)]);
    let result = candidates(&snapshot, &context);
    assert_eq!(result[0].label, "name");
    assert_eq!(result[0].provider, ProviderKind::Local);
    assert_eq!(
        snapshot.symbol(result[0].symbol_id.unwrap()).unwrap().kind,
        SymbolKind::Variable
    );
    assert!(
        !result
            .iter()
            .any(|c| matches!(c.label.as_str(), "siblingOnly" | "nameLater"))
    );
    assert!(
        result
            .iter()
            .any(|c| c.label == "namedGlobal" && c.provider == ProviderKind::Workspace)
    );
    let source = source.replace("na/*caret*/", "/*caret*/");
    let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", &source)]);
    let result = candidates(&snapshot, &context);
    assert!(result.iter().any(|c| c.label == "argument"));
    assert!(
        !result
            .iter()
            .any(|c| matches!(c.label.as_str(), "siblingOnly" | "nameLater"))
    );
}

#[test]
fn qualified_imports_keywords_snippets_and_empty_prefix_have_explicit_insertion_context() {
    let source = "package use\nimport demo.Cu/*caret*/\n";
    let (snapshot, context, text) = prepare(&[
        ("Use.kt", "kotlin", source),
        ("Customer.kt", "kotlin", "package demo\nclass Customer\n"),
        ("Other.kt", "kotlin", "package other\nclass Customer\n"),
    ]);
    assert!(context.import_context);
    assert!(!context.member_access);
    let result = candidates(&snapshot, &context);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].label, "Customer");
    assert_eq!(result[0].insert_text, "demo.Customer");
    let item = result[0].item(&context);
    item.replacement_range.validate(&text).unwrap();
    let mut inserted = text.clone();
    inserted.replace_range(
        item.replacement_range.start.0..item.replacement_range.end.0,
        &item.insert_text,
    );
    assert_eq!(inserted, "package use\nimport demo.Customer\n");
    let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", "/*caret*/")]);
    let result = candidates(&snapshot, &context);
    assert!(result.iter().any(|c| c.label == "fun"));
    assert_eq!(result.iter().filter(|c| c.label == "fun").count(), 1);
    let mut snippets = vec![];
    SnippetCompletionProvider
        .candidates(&context, &snapshot, &mut |c| {
            snippets.push(c);
            Ok(())
        })
        .unwrap();
    assert_eq!(labels(&snippets), BTreeSet::from(["fun", "when"]));
    assert!(snippets.iter().all(|c| c.insert_text == c.label));
}

#[test]
fn replacement_ranges_preserve_operators_receivers_suffix_and_unicode_source() {
    for operator in [".", "?."] {
        let source = format!(
            "class Box {{ val café: Int = 0 }}\nfun inspect(box: Box?) {{ box{operator}ca/*caret*/ + 12\n}}"
        );
        let (snapshot, context, text) = prepare(&[("Use.kt", "kotlin", &source)]);
        let result = candidates(&snapshot, &context);
        assert_eq!(labels(&result), BTreeSet::from(["café"]));
        let item = result[0].item(&context);
        assert_eq!(
            &text[item.replacement_range.start.0..item.replacement_range.end.0],
            "ca"
        );
        let mut inserted = text.clone();
        inserted.replace_range(
            item.replacement_range.start.0..item.replacement_range.end.0,
            &item.insert_text,
        );
        assert!(inserted.contains(&format!("box{operator}café + 12")));
    }
    let context = CompletionContext::at(
        SemanticIds::file("Use.ts"),
        "ts",
        "const $value = $va",
        ByteOffset(18),
    )
    .unwrap();
    assert_eq!(context.prefix, "$va");
    assert_eq!(context.replacement_range.start, ByteOffset(15));
    assert!(
        CompletionContext::at(SemanticIds::file("Use.kt"), "kotlin", "é", ByteOffset(1)).is_err()
    );
}

fn provider_set(reverse: bool) -> Vec<Box<dyn CompletionProvider>> {
    let mut providers: Vec<Box<dyn CompletionProvider>> = vec![
        Box::new(LocalScopeCompletionProvider),
        Box::new(TypeCompletionProvider),
        Box::new(WorkspaceSymbolCompletionProvider),
        Box::new(KeywordCompletionProvider(Arc::new(
            KeywordProvider::compiled().unwrap(),
        ))),
        Box::new(SnippetCompletionProvider),
    ];
    if reverse {
        providers.reverse();
    }
    providers
}
#[test]
fn ranking_deduplication_and_symbol_ties_are_stable_under_provider_and_file_input_order_changes() {
    let fixtures = [
        (
            "Use.kt",
            "kotlin",
            "fun inspect() { val customer = 1; cu/*caret*/ }\n",
        ),
        ("A.kt", "kotlin", "package a\nclass Customer\n"),
        ("B.kt", "kotlin", "package b\nclass Customer\n"),
    ];
    let (snapshot, context, _) = prepare(&fixtures);
    let forward = CompletionEngine::new(provider_set(false), Box::new(HeuristicCompletionRanker))
        .complete(&context, &snapshot, 50, &CancellationToken::new())
        .unwrap();
    let reverse = CompletionEngine::new(provider_set(true), Box::new(HeuristicCompletionRanker))
        .complete(&context, &snapshot, 50, &CancellationToken::new())
        .unwrap();
    assert_eq!(forward, reverse);
    let mut fixtures = fixtures;
    fixtures.reverse();
    let (snapshot, context, _) = prepare(&fixtures);
    assert_eq!(forward, candidates(&snapshot, &context));
    assert_eq!(forward[0].label, "customer");
    assert_eq!(forward.iter().filter(|c| c.label == "Customer").count(), 2);
    let mut candidate = forward[0].clone();
    candidate.label = "customer".into();
    candidate.scope_distance = Some(0);
    candidate.same_file_count = 1;
    candidate.workspace_frequency = 1;
    let score = HeuristicCompletionRanker.score(&context, &candidate);
    assert!((score.total() - 0.595).abs() < 0.00001);
    assert_eq!(score.optional, 0.0);
    candidate.workspace_frequency = usize::MAX;
    assert_eq!(
        HeuristicCompletionRanker
            .score(&context, &candidate)
            .frequency,
        1.0
    );
}

#[test]
fn limits_cancellation_stale_source_and_invalid_optional_ranker_are_explicit() {
    let (snapshot, context, _) = prepare(&[("Use.kt", "kotlin", "fun f() { /*caret*/ }\n")]);
    assert_eq!(
        engine()
            .complete(&context, &snapshot, 1, &CancellationToken::new())
            .unwrap()
            .len(),
        1
    );
    assert!(
        engine()
            .complete(&context, &snapshot, 0, &CancellationToken::new())
            .unwrap()
            .is_empty()
    );
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        engine().complete(&context, &snapshot, 50, &token),
        Err(kode_core::DomainError::Cancelled)
    ));
    let mut stale = context.clone();
    stale.content_hash.push('x');
    assert!(
        engine()
            .complete(&stale, &snapshot, 50, &CancellationToken::new())
            .is_err()
    );
    struct Invalid;
    impl CompletionRanker for Invalid {
        fn score(&self, _: &CompletionContext, _: &CompletionCandidate) -> CompletionScore {
            CompletionScore {
                optional: f32::NAN,
                ..CompletionScore::default()
            }
        }
    }
    assert!(
        CompletionEngine::new(provider_set(false), Box::new(Invalid))
            .complete(&context, &snapshot, 50, &CancellationToken::new())
            .is_err()
    );
}
