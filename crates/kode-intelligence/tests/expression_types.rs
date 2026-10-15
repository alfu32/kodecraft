use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, WorkspaceGeneration,
    document::Document,
};
use kode_intelligence::{
    frontend::{kotlin::KotlinSemanticAdapter, kotlin_completion_context},
    ids::*,
    model::*,
    resolver::{BestEffortExpressionTypeResolver, SemanticResolver},
    snapshot::SemanticSnapshot,
    store::{StoreLocation, StoreWorker},
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::registry::GrammarRegistry;
use std::collections::BTreeSet;
fn delta(path: &str, text: &str) -> FileSemanticDelta {
    let text = text.replace("/*caret*/", "");
    let doc = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "expression-test".into(),
            key: path.into(),
            label: path.into(),
        },
        Some("kotlin".into()),
        &text,
    );
    KotlinSemanticAdapter::new(&GrammarRegistry::compiled())
        .unwrap()
        .extract(
            path,
            &doc.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap()
}
fn snapshot(text: &str) -> SemanticSnapshot {
    SemanticSnapshot::new(
        1,
        SemanticResolver::resolve([delta("Test.kt", text)], &CancellationToken::new())
            .unwrap()
            .project,
    )
    .unwrap()
}
fn symbol<'a>(snapshot: &'a SemanticSnapshot, name: &str) -> &'a SymbolRecord {
    snapshot
        .workspace_symbols()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
}
fn ty<'a>(snapshot: &'a SemanticSnapshot, name: &str) -> Option<&'a TypeRef> {
    let s = symbol(snapshot, name);
    s.declared_type_id
        .or(s.inferred_type_id)
        .and_then(|id| snapshot.type_record(id))
        .map(|t| &t.r#ref)
}
fn receiver_type(snapshot: &SemanticSnapshot, source: &str) -> TypeRef {
    let caret = source.find("/*caret*/").unwrap();
    let text = source.replace("/*caret*/", "");
    let context = kotlin_completion_context(&text, ByteOffset(caret)).unwrap();
    assert!(context.member_access);
    BestEffortExpressionTypeResolver::type_of(
        SemanticIds::file("Test.kt"),
        context.receiver_range.unwrap(),
        snapshot,
    )
}
fn names(snapshot: &SemanticSnapshot, source: &str) -> BTreeSet<String> {
    let caret = ByteOffset(source.find("/*caret*/").unwrap());
    snapshot
        .members(&receiver_type(snapshot, source))
        .into_iter()
        .filter(|s| snapshot.is_accessible(s, SemanticIds::file("Test.kt"), caret))
        .map(|s| s.name.clone())
        .collect()
}
const MODEL: &str = "class Address { val city: String = \"\" }\nclass Customer { val address: Address = Address(); val id: Long = 0; fun primaryAddress(): Address = Address() }\nfun getCustomer(): Customer = Customer()\n";

#[test]
fn chained_properties_methods_and_function_calls_keep_receiver_result_types() {
    for expression in [
        "customer.address",
        "customer.primaryAddress()",
        "getCustomer().address",
    ] {
        let source = format!(
            "{MODEL}fun test() {{ val customer = getCustomer(); {expression}./*caret*/ }}\n"
        );
        let snapshot = snapshot(&source);
        assert_eq!(
            names(&snapshot, &source),
            BTreeSet::from(["city".into()]),
            "{expression}"
        );
    }
}
#[test]
fn parenthesized_and_cast_receivers_use_declared_target_shape() {
    for expression in [
        "(value as Customer)",
        "((value as Customer))",
        "(getCustomer())",
    ] {
        let source = format!("{MODEL}fun test(value: Any) {{ {expression}./*caret*/ }}\n");
        let snapshot = snapshot(&source);
        assert!(
            names(&snapshot, &source).contains("id"),
            "{expression}: {:?}",
            receiver_type(&snapshot, &source)
        );
    }
}
#[test]
fn references_assignments_and_navigation_initializers_propagate_without_last_assignment_bias() {
    let source = format!(
        "{MODEL}fun test(original: Customer) {{ val copied = original; var assigned = null; assigned = getCustomer(); val address = copied.address; assigned./*caret*/ }}\n"
    );
    let snapshot = snapshot(&source);
    let customer = symbol(&snapshot, "Customer").id;
    assert_eq!(ty(&snapshot, "copied"), Some(&TypeRef::named(customer)));
    assert_eq!(ty(&snapshot, "assigned"), Some(&TypeRef::named(customer)));
    assert_eq!(
        ty(&snapshot, "address"),
        Some(&TypeRef::named(symbol(&snapshot, "Address").id))
    );
    assert!(names(&snapshot, &source).contains("id"));
    let mut chain = format!("{MODEL}fun chain() {{ val item0 = getCustomer(); ");
    for index in 1..100 {
        chain.push_str(&format!("val item{index} = item{}; ", index - 1));
    }
    chain.push_str("item99./*caret*/ }\n");
    let chain_snapshot = self::snapshot(&chain);
    assert_eq!(
        ty(&chain_snapshot, "item99"),
        Some(&TypeRef::named(symbol(&chain_snapshot, "Customer").id))
    );
    assert!(names(&chain_snapshot, &chain).contains("id"));
}
#[test]
fn conflicting_assignments_form_order_independent_unions_and_intersect_members() {
    let definitions = "class First { val shared: Long = 0; val onlyFirst: Long = 0 }\nclass Second { val shared: Long = 0; val onlySecond: Long = 0 }\n";
    for (first, second) in [("First", "Second"), ("Second", "First")] {
        let source = format!(
            "{definitions}fun test() {{ var value = {first}(); value = {second}(); value./*caret*/ }}\n"
        );
        let snapshot = snapshot(&source);
        assert!(
            matches!(ty(&snapshot,"value"),Some(TypeRef::Union {alternatives}) if alternatives.len()==2)
        );
        assert_eq!(names(&snapshot, &source), BTreeSet::from(["shared".into()]));
    }
}
#[test]
fn primitive_literals_and_unknown_receivers_keep_honest_partial_answers() {
    let source = "class Known { val coincidental: Long = 0 }\nfun test(unknown: Any) { val count = 1; val title = \"Kode\"; unknown.coincidental }\n";
    let snapshot = snapshot(source);
    assert_eq!(ty(&snapshot, "count"), Some(&TypeRef::primitive("Int")));
    assert_eq!(ty(&snapshot, "title"), Some(&TypeRef::primitive("String")));
    assert_eq!(
        snapshot
            .occurrences(SemanticIds::file("Test.kt"))
            .find(|o| o.kind == OccurrenceKind::MemberReference && o.text == "coincidental")
            .unwrap()
            .resolved_symbol_id,
        None
    );
}
#[test]
fn nullable_generic_parameters_and_explicit_constructor_arguments_preserve_shape() {
    let source = "class Box<T> { val size: Int = 0 }\nfun inspect(box: Box<String>?) { val created = Box<String>(); box?./*caret*/ }\n";
    let snapshot = snapshot(source);
    assert!(
        matches!(ty(&snapshot,"box"),Some(TypeRef::Nullable {inner}) if matches!(inner.as_ref(),TypeRef::Generic {arguments,..} if arguments==&[TypeRef::primitive("String")]))
    );
    assert!(
        matches!(ty(&snapshot,"created"),Some(TypeRef::Generic {arguments,..}) if arguments==&[TypeRef::primitive("String")])
    );
    assert_eq!(names(&snapshot, source), BTreeSet::from(["size".into()]));
}
#[test]
fn this_super_inherited_members_and_visibility_match_native_scope_context() {
    let base = "open class Entity { val id: Long = 0; protected val inherited: Long = 0; private val hiddenBase: Long = 0 }\nclass Customer : Entity() { val name: String = \"\"; private val secret: String = \"\"; fun inspect() { RECEIVER./*caret*/ } }\n";
    let source = base.replace("RECEIVER", "this");
    let snapshot = snapshot(&source);
    assert_eq!(
        names(&snapshot, &source),
        BTreeSet::from(["id", "inherited", "name", "secret", "inspect"].map(String::from))
    );
    let source = base.replace("RECEIVER", "super");
    let snapshot = self::snapshot(&source);
    assert_eq!(
        names(&snapshot, &source),
        BTreeSet::from(["id", "inherited"].map(String::from))
    );
}
#[test]
fn call_arguments_do_not_replace_the_outer_return_value_or_invent_unknown_types() {
    for expression in [
        "factory(1)",
        "factory(other.city)",
        "factory(other.city).primaryAddress()",
    ] {
        let source = format!(
            "{MODEL}fun factory(value: Any): Customer = Customer()\nfun test(other: Address) {{ {expression}./*caret*/ }}\n"
        );
        let snapshot = snapshot(&source);
        let expected = if expression.ends_with("primaryAddress()") {
            "city"
        } else {
            "id"
        };
        assert!(
            names(&snapshot, &source).contains(expected),
            "{expression}: {:?}",
            receiver_type(&snapshot, &source)
        );
    }
    let source = "fun test() { unknown(1)./*caret*/ }\n";
    let snapshot = snapshot(source);
    assert_eq!(receiver_type(&snapshot, source), TypeRef::Unknown);
}
#[tokio::test]
async fn persisted_expression_facts_reconstruct_receiver_types_and_public_bindings() {
    let source = format!(
        "{MODEL}val exported = getCustomer().address\nfun test() {{ getCustomer().primaryAddress()./*caret*/ }}\n"
    );
    let snapshot = snapshot(&source);
    let expected = names(&snapshot, &source);
    let directory = tempfile::tempdir().unwrap();
    let location = StoreLocation::Project(directory.path().into());
    let worker = StoreWorker::start(location.clone()).await.unwrap();
    let tag = JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    };
    worker
        .client()
        .commit(tag, snapshot, None, CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap();
    worker.shutdown().await.unwrap();
    let worker = StoreWorker::start(location).await.unwrap();
    let reloaded = worker.client().snapshot().unwrap();
    assert_eq!(names(&reloaded, &source), expected);
    assert_eq!(
        ty(&reloaded, "exported"),
        Some(&TypeRef::named(symbol(&reloaded, "Address").id))
    );
    worker.shutdown().await.unwrap();
}

#[test]
fn callable_values_work_while_noncallable_shadows_do_not_infer_a_constructor() {
    let source =
        format!("{MODEL}fun test(callback: (Int) -> Customer) {{ callback(1)./*caret*/ }}\n");
    let snapshot = snapshot(&source);
    assert!(names(&snapshot, &source).contains("id"));
    let source = "class Factory { val field: Int = 0 }\nfun test(Factory: Int) { val value = Factory(); value./*caret*/ }\n";
    let snapshot = self::snapshot(source);
    assert_eq!(ty(&snapshot, "value"), None);
    assert_eq!(receiver_type(&snapshot, source), TypeRef::Unknown);
    assert!(names(&snapshot, source).is_empty());
}

#[test]
fn union_member_identity_cycles_and_untyped_exported_bindings_remain_deterministic() {
    let source = "class First { val shared: Long = 0 }\nclass Second { val shared: Long = 0 }\nfun test() { var value = First(); value = Second(); val result = value.shared; var a = b; var b = a }\n";
    let snapshot = snapshot(source);
    assert_eq!(ty(&snapshot, "result"), Some(&TypeRef::primitive("Long")));
    assert!(
        snapshot
            .occurrences(SemanticIds::file("Test.kt"))
            .filter(|o| o.kind == OccurrenceKind::MemberReference && o.text == "shared")
            .all(|o| o.resolved_symbol_id.is_none())
    );
    assert_eq!(ty(&snapshot, "a"), None);
    assert_eq!(ty(&snapshot, "b"), None);
    let first = delta("Surface.kt", "class Box<T>\nval value = Box<String>()\n");
    let second = delta("Surface.kt", "class Box<T>\nval value = Box<Int>()\n");
    assert_ne!(first.exported_surface_hash, second.exported_surface_hash);
    assert_eq!(
        delta("Surface.kt", "val value = 1\n").exported_surface_hash,
        delta("Surface.kt", "val value = 2\n").exported_surface_hash
    );
}

#[test]
fn inference_union_limits_retain_partial_facts_without_selecting_arbitrary_alternatives() {
    let mut source = (0..65)
        .map(|index| format!("class Type{index}\n"))
        .collect::<String>();
    source.push_str("fun test() { var value = Type0(); ");
    for index in 1..65 {
        source.push_str(&format!("value = Type{index}(); "));
    }
    source.push_str("}\n");
    let report =
        SemanticResolver::resolve([delta("Limit.kt", &source)], &CancellationToken::new()).unwrap();
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("64 alternatives"))
    );
    let snapshot = SemanticSnapshot::new(1, report.project).unwrap();
    assert_eq!(ty(&snapshot, "value"), Some(&TypeRef::Unknown));
    assert!(
        snapshot
            .workspace_symbols()
            .any(|symbol| symbol.name == "Type64")
    );
}

#[test]
fn c_qualified_pointer_casts_and_calls_preserve_receiver_types_without_typing_void_members() {
    use kode_intelligence::frontend::c::{CSemanticAdapter, completion_context};
    let source = "struct Foo { int id; };\ntypedef struct Foo Foo;\nFoo *create(int count);\nvoid use(void *opaque) { ((const struct Foo *)opaque)->id; create(1)->id; opaque->id; }\n";
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "c-expression-test".into(),
            key: "Test.c".into(),
            label: "Test.c".into(),
        },
        Some("c".into()),
        source,
    );
    let facts = CSemanticAdapter::new(&GrammarRegistry::compiled())
        .unwrap()
        .extract(
            "Test.c",
            &document.snapshot(),
            WorkspaceGeneration(1),
            &CancellationToken::new(),
        )
        .unwrap();
    let snapshot = SemanticSnapshot::new(
        1,
        SemanticResolver::resolve([facts], &CancellationToken::new())
            .unwrap()
            .project,
    )
    .unwrap();
    let file = SemanticIds::file("Test.c");
    for expression in ["((const struct Foo *)opaque)->", "create(1)->"] {
        let caret = source.find(expression).unwrap() + expression.len();
        let context = completion_context(source, ByteOffset(caret)).unwrap();
        let ty = BestEffortExpressionTypeResolver::type_of(
            file,
            context.receiver_range.unwrap(),
            &snapshot,
        );
        assert_eq!(
            snapshot
                .members(&ty)
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["id"]
        );
    }
    let member = source.find("opaque->id").unwrap() + "opaque->".len();
    assert_eq!(
        snapshot
            .occurrences(file)
            .find(|o| o.range.start.0 == member)
            .unwrap()
            .resolved_symbol_id,
        None
    );
}
