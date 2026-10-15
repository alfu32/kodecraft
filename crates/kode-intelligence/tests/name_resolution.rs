use kode_core::{
    DocumentId, DocumentOrigin, JobTag, RequestId, WorkspaceGeneration, document::Document,
};
use kode_intelligence::{
    frontend::{c::CSemanticAdapter, kotlin::KotlinSemanticAdapter},
    ids::*,
    model::*,
    resolver::{ResolutionReport, SemanticResolver},
    snapshot::SemanticSnapshot,
    store::{StoreLocation, StoreWorker},
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::registry::GrammarRegistry;
fn extract(path: &str, language: &str, text: &str) -> FileSemanticDelta {
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "resolution-test".into(),
            key: path.into(),
            label: path.into(),
        },
        Some(language.into()),
        text,
    );
    let registry = GrammarRegistry::compiled();
    let token = CancellationToken::new();
    if language == "c" {
        CSemanticAdapter::new(&registry)
            .unwrap()
            .extract(path, &document.snapshot(), WorkspaceGeneration(1), &token)
            .unwrap()
    } else {
        KotlinSemanticAdapter::new(&registry)
            .unwrap()
            .extract(path, &document.snapshot(), WorkspaceGeneration(1), &token)
            .unwrap()
    }
}
fn resolve(deltas: impl IntoIterator<Item = FileSemanticDelta>) -> ResolutionReport {
    SemanticResolver::resolve(deltas, &CancellationToken::new()).unwrap()
}
fn symbol<'a>(report: &'a ResolutionReport, name: &str) -> &'a SymbolRecord {
    report
        .project
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
}
fn target(report: &ResolutionReport, path: &str, offset: usize) -> Option<SymbolId> {
    report
        .project
        .occurrences
        .iter()
        .find(|o| o.file_id == SemanticIds::file(path) && o.range.start.0 == offset)
        .and_then(|o| o.resolved_symbol_id)
}
fn ty<'a>(report: &'a ResolutionReport, name: &str) -> Option<&'a TypeRef> {
    let symbol = symbol(report, name);
    symbol
        .declared_type_id
        .and_then(|id| report.project.types.get(&id))
        .map(|t| &t.r#ref)
}

#[test]
fn definitions_references_forward_functions_and_cross_file_types_use_identity_independent_of_input_order()
 {
    let definitions = extract(
        "Model.kt",
        "kotlin",
        "class Customer { val name: String = \"\" }\n",
    );
    let source = "fun before(): Customer = make()\nfun make(): Customer = Customer()\n";
    let consumer = extract("Use.kt", "kotlin", source);
    let first = resolve([consumer.clone(), definitions.clone()]);
    let second = resolve([definitions, consumer]);
    assert_eq!(first.project.symbols, second.project.symbols);
    assert_eq!(first.project.occurrences, second.project.occurrences);
    assert_eq!(first.project.relations, second.project.relations);
    let customer = symbol(&first, "Customer").id;
    let make = symbol(&first, "make").id;
    assert_eq!(
        target(&first, "Use.kt", source.find("make()").unwrap()),
        Some(make)
    );
    assert_eq!(ty(&first, "before"), Some(&TypeRef::named(customer)));
    assert_eq!(ty(&first, "make"), Some(&TypeRef::named(customer)));
    let snapshot = SemanticSnapshot::new(1, first.project).unwrap();
    assert_eq!(snapshot.references(customer).count(), 4);
    assert!(
        snapshot
            .dependencies(SemanticIds::file("Use.kt"))
            .any(|d| d.to_file_id == SemanticIds::file("Model.kt")
                && d.kind == FileDependencyKind::TypeReference)
    );
}

#[test]
fn lexical_shadowing_declaration_order_and_sibling_scopes_never_leak_through_global_fallback() {
    let source = "fun first() { val value = 1; println(value) }\nfun second() { println(value); val value = 2; println(value) }\nfun nested() { val value = 3; if (true) { val value = 4; println(value) }; println(value) }\n";
    let report = resolve([extract("Scopes.kt", "kotlin", source)]);
    let declarations: Vec<_> = report
        .project
        .symbols
        .iter()
        .filter(|s| s.name == "value")
        .collect();
    assert_eq!(declarations.len(), 4);
    let order = source
        .match_indices("println(value)")
        .map(|(i, _)| i + "println(".len())
        .collect::<Vec<_>>();
    assert_eq!(
        target(&report, "Scopes.kt", order[0]),
        Some(
            declarations
                .iter()
                .find(|s| s.name_range.start.0 == source.find("value = 1").unwrap())
                .unwrap()
                .id
        )
    );
    assert_eq!(target(&report, "Scopes.kt", order[1]), None);
    for (occurrence, definition) in [
        (order[2], "value = 2"),
        (order[3], "value = 4"),
        (order[4], "value = 3"),
    ] {
        assert_eq!(
            target(&report, "Scopes.kt", occurrence),
            Some(
                declarations
                    .iter()
                    .find(|s| s.name_range.start.0 == source.find(definition).unwrap())
                    .unwrap()
                    .id
            )
        );
    }
    assert!(
        report
            .unresolved
            .iter()
            .any(|u| u.name == "value" && u.range.start.0 == order[1])
    );
}

#[test]
fn package_import_aliases_wildcards_and_same_spelling_ambiguities_resolve_deterministically() {
    let alpha = extract("Alpha.kt", "kotlin", "package alpha\nclass Customer\n");
    let beta = extract("Beta.kt", "kotlin", "package beta\nclass Customer\n");
    let alias = "package app\nimport alpha.Customer as Person\nfun inspect(value: Person) {}\n";
    let report = resolve([
        beta.clone(),
        extract("Alias.kt", "kotlin", alias),
        alpha.clone(),
    ]);
    let alpha_id = report
        .project
        .symbols
        .iter()
        .find(|s| s.qualified_name.as_deref() == Some("alpha.Customer"))
        .unwrap()
        .id;
    assert_eq!(ty(&report, "value"), Some(&TypeRef::named(alpha_id)));
    assert_eq!(
        target(&report, "Alias.kt", alias.rfind("Person").unwrap()),
        Some(alpha_id)
    );
    assert!(
        report
            .project
            .dependencies
            .iter()
            .any(|d| d.from_file_id == SemanticIds::file("Alias.kt")
                && d.to_file_id == SemanticIds::file("Alpha.kt")
                && d.kind == FileDependencyKind::Import)
    );
    let wildcard = "package app\nimport alpha.*\nfun inspect(value: Customer) {}\n";
    let report = resolve([
        alpha.clone(),
        beta.clone(),
        extract("Wildcard.kt", "kotlin", wildcard),
    ]);
    assert_eq!(ty(&report, "value"), Some(&TypeRef::named(alpha_id)));
    for source in [
        "package app\nfun inspect(value: Customer) {}\n",
        "package app\nimport alpha.*\nimport beta.*\nfun inspect(value: Customer) {}\n",
    ] {
        let first = resolve([
            alpha.clone(),
            beta.clone(),
            extract("Ambiguous.kt", "kotlin", source),
        ]);
        let second = resolve([
            extract("Ambiguous.kt", "kotlin", source),
            beta.clone(),
            alpha.clone(),
        ]);
        assert_eq!(ty(&first, "value"), None);
        assert_eq!(first.project.occurrences, second.project.occurrences);
        assert_eq!(
            target(&first, "Ambiguous.kt", source.rfind("Customer").unwrap()),
            None
        );
    }
}

#[test]
fn inherited_visibility_this_super_and_unrelated_members_keep_correct_identity() {
    let source = "open class Base { val id: Long = 0; protected val inherited: Long = 0; private val hidden: Long = 0 }\nclass Child : Base() { private val secret: String = \"\"; fun inspect() { println(id); println(inherited); println(hidden); this.secret; super.id } }\nclass Other { val unrelated = 1 }\nfun external(child: Child) { child.secret; child.inherited; child.id; child.unrelated }\n";
    let report = resolve([extract("Visibility.kt", "kotlin", source)]);
    let child = symbol(&report, "Child").id;
    let base = symbol(&report, "Base").id;
    assert!(
        report
            .project
            .relations
            .iter()
            .any(|r| r.from == child && r.to == base && r.kind == RelationKind::Extends)
    );
    for (expression, name) in [
        ("println(id)", "id"),
        ("println(inherited)", "inherited"),
        ("this.secret", "secret"),
        ("super.id", "id"),
        ("child.id", "id"),
    ] {
        let start = source.find(expression).unwrap() + expression.find(name).unwrap();
        assert_eq!(
            target(&report, "Visibility.kt", start),
            Some(symbol(&report, name).id),
            "{expression}"
        );
    }
    for (expression, name) in [
        ("println(hidden)", "hidden"),
        ("child.secret", "secret"),
        ("child.inherited", "inherited"),
        ("child.unrelated", "unrelated"),
    ] {
        let start = source.find(expression).unwrap() + expression.find(name).unwrap();
        assert_eq!(
            target(&report, "Visibility.kt", start),
            None,
            "{expression}"
        );
    }
}

#[test]
fn private_top_level_types_do_not_cross_files_and_missing_names_remain_partial() {
    let report = resolve([
        extract(
            "Hidden.kt",
            "kotlin",
            "package demo\nprivate class Secret\n",
        ),
        extract(
            "Use.kt",
            "kotlin",
            "package demo\nfun use(secret: Secret, missing: Missing) { unknown() }\n",
        ),
    ]);
    assert_eq!(ty(&report, "secret"), None);
    assert_eq!(ty(&report, "missing"), None);
    assert!(report.unresolved.iter().any(|u| u.name == "Secret"));
    assert!(report.unresolved.iter().any(|u| u.name == "Missing"));
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        SemanticResolver::resolve([extract("Empty.kt", "kotlin", "")], &token),
        Err(kode_core::DomainError::Cancelled)
    ));
}

#[tokio::test]
async fn c_tag_typedef_pointer_anonymous_and_same_spelling_cases_survive_native_store_reload() {
    let header = "typedef struct Foo { int id; } Foo;\ntypedef struct { int value; } Bar;\n";
    let source = "#include \"types.h\"\nvoid run(Foo *ptr) { struct Foo tagged; Foo aliased; Bar bar; tagged.id = aliased.id; ptr->id = 1; bar.value = 2; unknown.id = 0; }\n";
    let report = resolve([
        extract("use.c", "c", source),
        extract("types.h", "c", header),
    ]);
    let tag = report
        .project
        .symbols
        .iter()
        .find(|s| s.name == "Foo" && s.kind == SymbolKind::Struct)
        .unwrap()
        .id;
    let alias = report
        .project
        .symbols
        .iter()
        .find(|s| s.name == "Foo" && s.kind == SymbolKind::TypeAlias)
        .unwrap()
        .id;
    assert_ne!(tag, alias);
    assert!(
        report
            .project
            .relations
            .iter()
            .any(|r| r.from == alias && r.to == tag && r.kind == RelationKind::AliasOf)
    );
    let snapshot = SemanticSnapshot::new(1, report.project).unwrap();
    for (name, field) in [
        ("ptr", "id"),
        ("tagged", "id"),
        ("aliased", "id"),
        ("bar", "value"),
    ] {
        let variable = snapshot
            .workspace_symbols()
            .find(|s| s.name == name)
            .unwrap();
        let ty = &snapshot
            .type_record(variable.declared_type_id.unwrap())
            .unwrap()
            .r#ref;
        assert_eq!(
            snapshot
                .members(ty)
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            [field],
            "{name}"
        );
    }
    let id = snapshot
        .workspace_symbols()
        .find(|s| s.name == "id")
        .unwrap()
        .id;
    assert_eq!(
        snapshot
            .references(id)
            .filter(|o| o.kind == OccurrenceKind::MemberReference)
            .count(),
        3
    );
    let unknown = source.find("unknown.id").unwrap() + "unknown.".len();
    assert_eq!(
        snapshot
            .occurrences(SemanticIds::file("use.c"))
            .find(|o| o.range.start.0 == unknown)
            .unwrap()
            .resolved_symbol_id,
        None
    );
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
    assert_eq!(
        reloaded
            .references(id)
            .filter(|o| o.kind == OccurrenceKind::MemberReference)
            .count(),
        3
    );
    worker.shutdown().await.unwrap();
}

#[test]
fn declared_nullable_generic_function_types_and_alias_cycles_terminate_with_partial_facts() {
    let source = "class Box<T> { val size: Int = 0 }\nfun inspect(box: Box<String>?, callback: (Int, String) -> Box<String>?) {}\ntypealias Left = Right\ntypealias Right = Left\n";
    let report = resolve([extract("Types.kt", "kotlin", source)]);
    assert!(
        matches!(ty(&report,"box"),Some(TypeRef::Nullable {inner}) if matches!(inner.as_ref(),TypeRef::Generic {..}))
    );
    assert!(
        matches!(ty(&report,"callback"),Some(TypeRef::Function {parameters,returns:Some(_)}) if parameters.len()==2)
    );
    let left = symbol(&report, "Left").id;
    let snapshot = SemanticSnapshot::new(1, report.project).unwrap();
    assert!(snapshot.members(&TypeRef::named(left)).is_empty());
}

#[test]
fn local_call_shadowing_and_snapshot_global_candidates_do_not_escape_their_scope() {
    let source = "fun load() {}\nfun first() { class Local\n val load = 1; load() }\nfun second() { /*caret*/ }\n";
    let report = resolve([extract("Locals.kt", "kotlin", source)]);
    let local = report
        .project
        .symbols
        .iter()
        .find(|s| s.name == "load" && s.kind == SymbolKind::Variable)
        .unwrap()
        .id;
    assert_eq!(
        target(&report, "Locals.kt", source.rfind("load()").unwrap()),
        Some(local)
    );
    let snapshot = SemanticSnapshot::new(1, report.project).unwrap();
    let visible = snapshot.visible_symbols(
        SemanticIds::file("Locals.kt"),
        kode_core::ByteOffset(source.find("/*caret*/").unwrap()),
    );
    assert!(!visible.iter().any(|s| s.name == "Local"));
}

#[test]
fn c_ordinary_and_tag_namespaces_nested_alias_types_and_enum_names_follow_context() {
    let source = "typedef struct Inner { int id; } Alias;\ntypedef struct Outer { Alias item; } Outer;\nenum Color { RED, BLUE };\nvoid run(void) { Alias before; int Alias; Alias invalid; struct Inner tagged; int color = RED; }\n";
    let report = resolve([extract("Context.c", "c", source)]);
    let alias = report
        .project
        .symbols
        .iter()
        .find(|s| s.name == "Alias" && s.kind == SymbolKind::TypeAlias)
        .unwrap()
        .id;
    assert_eq!(ty(&report, "before"), Some(&TypeRef::named(alias)));
    assert_eq!(ty(&report, "invalid"), None);
    assert_eq!(
        target(&report, "Context.c", source.find("Alias item").unwrap()),
        Some(alias)
    );
    assert_eq!(
        target(&report, "Context.c", source.rfind("RED").unwrap()),
        Some(symbol(&report, "RED").id)
    );
}
