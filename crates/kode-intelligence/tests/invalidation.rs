use kode_core::{
    DocumentId, DocumentOrigin, JobTag, RequestId, WorkspaceGeneration, document::Document,
};
use kode_intelligence::{
    frontend::{c::CSemanticAdapter, kotlin::KotlinSemanticAdapter},
    ids::*,
    index::{IncrementalSemanticIndex, SemanticCacheKey, dependencies::DependencyGraph},
    model::*,
    resolver::SemanticResolver,
    snapshot::SemanticSnapshot,
    store::*,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::registry::GrammarRegistry;
use std::{collections::BTreeSet, sync::Arc};

fn extract(path: &str, text: &str, version: u64) -> FileSemanticDelta {
    let language = if path.ends_with(".c") || path.ends_with(".h") {
        "c"
    } else {
        "kotlin"
    };
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "invalidation-test".into(),
            key: path.into(),
            label: path.into(),
        },
        Some(language.into()),
        text,
    );
    let registry = GrammarRegistry::compiled();
    let cancel = CancellationToken::new();
    let mut delta = if language == "c" {
        CSemanticAdapter::new(&registry)
            .unwrap()
            .extract(path, &document.snapshot(), WorkspaceGeneration(1), &cancel)
            .unwrap()
    } else {
        KotlinSemanticAdapter::new(&registry)
            .unwrap()
            .extract(path, &document.snapshot(), WorkspaceGeneration(1), &cancel)
            .unwrap()
    };
    delta.file.parse_version = version;
    delta.file.semantic_version = version;
    delta
}
fn apply(
    previous: &ResolvedSemanticProject,
    deltas: impl IntoIterator<Item = FileSemanticDelta>,
) -> kode_intelligence::index::IncrementalResolution {
    IncrementalSemanticIndex::prepare(previous, deltas, [], &CancellationToken::new()).unwrap()
}
fn initial(deltas: impl IntoIterator<Item = FileSemanticDelta>) -> ResolvedSemanticProject {
    apply(&ResolvedSemanticProject::default(), deltas)
        .report
        .project
}
fn symbol<'a>(project: &'a ResolvedSemanticProject, name: &str) -> &'a SymbolRecord {
    project.symbols.iter().find(|s| s.name == name).unwrap()
}
fn inferred<'a>(project: &'a ResolvedSemanticProject, name: &str) -> Option<&'a TypeRef> {
    symbol(project, name)
        .inferred_type_id
        .and_then(|id| project.types.get(&id))
        .map(|t| &t.r#ref)
}
const SERVICE: &str =
    "fun getCustomer(): Customer = Customer()\nfun test() { val customer = getCustomer() }\n";
fn model() -> ResolvedSemanticProject {
    initial([
        extract("Customer.kt", "class Customer { val id: Long = 0 }\n", 1),
        extract("Service.kt", SERVICE, 1),
        extract("Unrelated.kt", "class Unrelated\n", 1),
    ])
}
fn assert_fresh_equivalent(project: &ResolvedSemanticProject) {
    let fresh = SemanticResolver::resolve(
        project.deltas.values().map(|d| d.as_ref().clone()),
        &CancellationToken::new(),
    )
    .unwrap()
    .project;
    assert_eq!(project.symbols, fresh.symbols);
    assert_eq!(project.occurrences, fresh.occurrences);
    assert_eq!(project.relations, fresh.relations);
    assert_eq!(project.expression_types, fresh.expression_types);
    let edges = |p: &ResolvedSemanticProject| {
        p.dependencies
            .iter()
            .map(|d| (d.from_file_id, d.to_file_id, d.kind))
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(edges(project), edges(&fresh));
}

#[test]
fn implementation_only_edit_retains_dependent_facts_relations_dependencies_and_generation() {
    let previous = model();
    let customer = SemanticIds::file("Customer.kt");
    let service = SemanticIds::file("Service.kt");
    assert!(
        previous
            .dependencies
            .iter()
            .any(|d| d.from_file_id == service
                && d.to_file_id == customer
                && d.kind == FileDependencyKind::TypeReference)
    );
    let update = apply(
        &previous,
        [extract(
            "Customer.kt",
            "class Customer { val id: Long = 1 }\n",
            2,
        )],
    );
    let next = &update.report.project;
    assert_eq!(
        update.invalidation.affected_files,
        BTreeSet::from([customer])
    );
    assert!(update.invalidation.surface_changed_files.is_empty());
    assert_eq!(next.resolution_generations[&customer], 2);
    assert_eq!(next.resolution_generations[&service], 1);
    assert!(Arc::ptr_eq(
        &next.deltas[&service],
        &previous.deltas[&service]
    ));
    assert_eq!(
        SemanticCacheKey::for_file(next, service),
        SemanticCacheKey::for_file(&previous, service)
    );
    assert_eq!(
        next.dependencies
            .iter()
            .filter(|d| d.from_file_id == service)
            .collect::<Vec<_>>(),
        previous
            .dependencies
            .iter()
            .filter(|d| d.from_file_id == service)
            .collect::<Vec<_>>()
    );
    assert_eq!(inferred(next, "customer"), inferred(&previous, "customer"));
    assert_fresh_equivalent(next);
}

#[test]
fn exported_surface_edit_re_resolves_transitive_dependents_without_changing_document_versions() {
    let previous = initial([
        extract("Base.kt", "open class Base { val id: Long = 0 }\n", 1),
        extract("Middle.kt", "open class Middle : Base()\n", 1),
        extract("Leaf.kt", "class Leaf : Middle()\n", 1),
        extract("Other.kt", "class Other\n", 1),
    ]);
    let next = apply(
        &previous,
        [extract(
            "Base.kt",
            "open class Base { val id: String = \"\" }\n",
            2,
        )],
    );
    for path in ["Base.kt", "Middle.kt", "Leaf.kt"] {
        let file = SemanticIds::file(path);
        assert_eq!(
            next.report.project.resolution_generations[&file], 2,
            "{path}"
        );
        assert!(next.invalidation.affected_files.contains(&file));
    }
    let leaf = SemanticIds::file("Leaf.kt");
    assert_eq!(next.report.project.deltas[&leaf].file.parse_version, 1);
    assert_ne!(
        SemanticCacheKey::for_file(&previous, leaf),
        SemanticCacheKey::for_file(&next.report.project, leaf)
    );
    assert_eq!(
        next.report.project.resolution_generations[&SemanticIds::file("Other.kt")],
        1
    );
    assert_fresh_equivalent(&next.report.project);
}

#[test]
fn new_definition_wakes_previously_unresolved_files_and_their_dependents() {
    let previous = initial([
        extract("Service.kt", SERVICE, 1),
        extract(
            "Caller.kt",
            "fun caller() { val fromService = getCustomer() }\n",
            1,
        ),
    ]);
    assert!(inferred(&previous, "customer").is_none());
    let update = apply(
        &previous,
        [extract(
            "Customer.kt",
            "class Customer { val id: Long = 0 }\n",
            1,
        )],
    );
    let next = &update.report.project;
    assert_eq!(
        inferred(next, "customer"),
        Some(&TypeRef::named(symbol(next, "Customer").id))
    );
    assert_eq!(inferred(next, "fromService"), inferred(next, "customer"));
    for path in ["Service.kt", "Caller.kt"] {
        assert_eq!(next.resolution_generations[&SemanticIds::file(path)], 2);
    }
    assert_fresh_equivalent(next);
}

#[test]
fn renamed_or_removed_definition_clears_obsolete_types_relations_and_occurrence_identities() {
    let previous = model();
    let old_id = symbol(&previous, "Customer").id;
    let renamed = apply(&previous, [extract("Customer.kt", "class Client\n", 2)]);
    assert!(inferred(&renamed.report.project, "customer").is_none());
    assert!(
        symbol(&renamed.report.project, "getCustomer")
            .declared_type_id
            .is_none()
    );
    assert!(
        renamed
            .report
            .project
            .occurrences
            .iter()
            .filter(|o| o.text == "Customer")
            .all(|o| o.resolved_symbol_id.is_none())
    );
    assert!(
        !renamed
            .report
            .project
            .relations
            .iter()
            .any(|r| r.from == old_id || r.to == old_id)
    );
    assert_fresh_equivalent(&renamed.report.project);
    let removed = IncrementalSemanticIndex::prepare(
        &previous,
        [],
        [SemanticIds::file("Customer.kt")],
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(inferred(&removed.report.project, "customer").is_none());
    assert!(
        !removed
            .report
            .project
            .deltas
            .contains_key(&SemanticIds::file("Customer.kt"))
    );
    assert_eq!(
        removed.invalidation.removed_files,
        BTreeSet::from([SemanticIds::file("Customer.kt")])
    );
    assert_fresh_equivalent(&removed.report.project);
    // Physical path rename is one atomic remove + insert, with new stable IDs.
    let moved = IncrementalSemanticIndex::prepare(
        &previous,
        [extract(
            "Renamed.kt",
            "class Customer { val id: Long = 0 }\n",
            1,
        )],
        [SemanticIds::file("Customer.kt")],
        &CancellationToken::new(),
    )
    .unwrap();
    assert_ne!(symbol(&moved.report.project, "Customer").id, old_id);
    assert_eq!(
        inferred(&moved.report.project, "customer"),
        Some(&TypeRef::named(
            symbol(&moved.report.project, "Customer").id
        ))
    );
    assert_fresh_equivalent(&moved.report.project);
}

#[test]
fn dependency_closure_and_incremental_updates_terminate_for_cycles() {
    let edges = [(1, 2), (2, 1), (3, 2)].map(|(from, to)| FileDependencyRecord {
        from_file_id: FileId(from),
        to_file_id: FileId(to),
        kind: FileDependencyKind::TypeReference,
        generation: 1,
    });
    let graph = DependencyGraph::new(&edges);
    assert_eq!(
        graph.dependent_closure([FileId(1)]),
        BTreeSet::from([FileId(1), FileId(2), FileId(3)])
    );
    assert_eq!(
        graph.dependencies_of(FileId(1)).collect::<Vec<_>>(),
        vec![FileId(2)]
    );
    let previous = initial([
        extract("A.kt", "class A : B()\n", 1),
        extract("B.kt", "class B : A()\n", 1),
    ]);
    let next = apply(
        &previous,
        [extract("A.kt", "class A : B() { val id: Int = 0 }\n", 2)],
    );
    assert_eq!(
        next.report
            .project
            .resolution_generations
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![2, 2]
    );
    assert_fresh_equivalent(&next.report.project);
}

#[test]
fn duplicate_new_export_invalidates_old_binding_instead_of_keeping_arbitrary_identity() {
    let previous = initial([
        extract("A.kt", "class Shared\n", 1),
        extract("Use.kt", "fun use(shared: Shared) {}\n", 1),
    ]);
    assert!(symbol(&previous, "shared").declared_type_id.is_some());
    let next = apply(&previous, [extract("B.kt", "class Shared\n", 1)]);
    assert!(
        symbol(&next.report.project, "shared")
            .declared_type_id
            .is_none()
    );
    assert_eq!(
        next.report.project.resolution_generations[&SemanticIds::file("Use.kt")],
        2
    );
    assert_fresh_equivalent(&next.report.project);
}

#[test]
fn noops_metadata_only_edits_and_failures_do_not_advance_semantic_generations_or_mutate_previous() {
    let previous = model();
    let delta = previous.deltas[&SemanticIds::file("Customer.kt")]
        .as_ref()
        .clone();
    let next = apply(&previous, [delta.clone()]);
    assert!(next.invalidation.affected_files.is_empty());
    assert_eq!(
        next.report.project.resolution_generations,
        previous.resolution_generations
    );
    let mut metadata = delta.clone();
    metadata.file.parse_version = 900;
    let next = apply(&previous, [metadata]);
    assert!(next.invalidation.affected_files.is_empty());
    assert_eq!(
        next.report.project.resolution_generations,
        previous.resolution_generations
    );
    assert_eq!(
        next.report.project.deltas[&delta.file.id]
            .file
            .parse_version,
        900
    );
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        IncrementalSemanticIndex::prepare(&previous, [delta.clone()], [], &token),
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(
        IncrementalSemanticIndex::prepare(
            &previous,
            [delta.clone()],
            [delta.file.id],
            &CancellationToken::new()
        )
        .is_err()
    );
    let mut conflicting = delta.clone();
    conflicting.file.content_hash.push('a');
    assert!(
        IncrementalSemanticIndex::prepare(
            &previous,
            [delta.clone(), conflicting],
            [],
            &CancellationToken::new()
        )
        .is_err()
    );
    let mut exhausted = previous.clone();
    exhausted
        .resolution_generations
        .insert(delta.file.id, u64::MAX);
    assert!(
        IncrementalSemanticIndex::prepare(
            &exhausted,
            [extract("Customer.kt", "class Changed\n", 2)],
            [],
            &CancellationToken::new()
        )
        .is_err()
    );
    assert_eq!(previous.deltas[&delta.file.id].file.parse_version, 1);
    assert_fresh_equivalent(&previous);
}

fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
async fn commit(
    client: &StoreClient,
    revision: u64,
    project: ResolvedSemanticProject,
) -> CommitReceipt {
    client
        .commit(
            tag(),
            SemanticSnapshot::new(revision, project).unwrap(),
            None,
            CancellationToken::new(),
        )
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value
}
#[tokio::test]
async fn implementation_only_sqlite_commit_retains_dependent_relations_and_generations_after_restart()
 {
    let directory = tempfile::tempdir().unwrap();
    let location = StoreLocation::Project(directory.path().into());
    let worker = StoreWorker::start(location.clone()).await.unwrap();
    let client = worker.client();
    let previous = model();
    commit(&client, 1, previous.clone()).await;
    let held = client.snapshot().unwrap();
    let update = apply(
        &previous,
        [extract(
            "Customer.kt",
            "class Customer { val id: Long = 1 }\n",
            2,
        )],
    );
    // Preparation has no publication side effect.
    assert_eq!(client.snapshot().unwrap().version(), 1);
    let receipt = commit(&client, 2, update.report.project).await;
    assert_eq!(
        receipt.changed_files,
        vec![SemanticIds::file("Customer.kt")]
    );
    assert_eq!(
        held.resolution_generation(SemanticIds::file("Customer.kt")),
        1
    );
    worker.shutdown().await.unwrap();
    let worker = StoreWorker::start(location).await.unwrap();
    let client = worker.client();
    let reloaded = client.snapshot().unwrap();
    assert_eq!(
        reloaded.resolution_generation(SemanticIds::file("Service.kt")),
        1
    );
    assert_eq!(
        inferred(reloaded.project(), "customer"),
        inferred(&previous, "customer")
    );
    let update = apply(
        reloaded.project(),
        [extract("Customer.kt", "class Client\n", 3)],
    );
    commit(&client, 3, update.report.project).await;
    assert_eq!(
        client
            .snapshot()
            .unwrap()
            .resolution_generation(SemanticIds::file("Service.kt")),
        2
    );
    assert!(inferred(client.snapshot().unwrap().project(), "customer").is_none());
    worker.shutdown().await.unwrap();
}

#[test]
fn c_moved_and_changed_fields_refresh_cross_file_pointer_and_alias_receivers() {
    let previous = initial([
        extract("types.h", "typedef struct Foo { int id; } Foo;\n", 1),
        extract(
            "use.c",
            "#include \"types.h\"\nvoid use(Foo *ptr) { ptr->id = 1; }\n",
            1,
        ),
    ]);
    let old_field = symbol(&previous, "id").id;
    let next = apply(
        &previous,
        [extract(
            "types.h",
            "\ntypedef struct Foo { long id; } Foo;\n",
            2,
        )],
    );
    let field = symbol(&next.report.project, "id");
    assert_ne!(field.id, old_field);
    assert!(
        next.report
            .project
            .occurrences
            .iter()
            .any(|o| o.file_id == SemanticIds::file("use.c")
                && o.text == "id"
                && o.resolved_symbol_id == Some(field.id))
    );
    assert_eq!(
        next.report.project.resolution_generations[&SemanticIds::file("use.c")],
        2
    );
    assert_fresh_equivalent(&next.report.project);
    let next = apply(
        &next.report.project,
        [extract(
            "types.h",
            "typedef struct Foo { int value; } Foo;\n",
            3,
        )],
    );
    assert!(
        next.report
            .project
            .occurrences
            .iter()
            .filter(|o| o.text == "id")
            .all(|o| o.resolved_symbol_id.is_none())
    );
    assert_fresh_equivalent(&next.report.project);
}
