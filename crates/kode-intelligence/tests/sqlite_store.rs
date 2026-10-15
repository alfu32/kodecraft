use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, LineIndex, RequestId, TextRange, Utf16Column,
    WorkspaceGeneration, document::Document,
};
use kode_intelligence::{
    frontend::kotlin::KotlinSemanticAdapter, ids::*, model::*, snapshot::SemanticSnapshot, store::*,
};
use kode_platform::jobs::CancellationToken;
use kode_syntax::registry::GrammarRegistry;
use std::{fs, sync::Arc};
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(7),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn delta(path: &str, text: &str) -> FileSemanticDelta {
    let document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "store-test".into(),
            key: path.into(),
            label: path.into(),
        },
        Some("kotlin".into()),
        text,
    );
    KotlinSemanticAdapter::new(&GrammarRegistry::compiled())
        .unwrap()
        .extract(
            path,
            &document.snapshot(),
            WorkspaceGeneration(7),
            &CancellationToken::new(),
        )
        .unwrap()
}
fn snapshot(version: u64, deltas: impl IntoIterator<Item = FileSemanticDelta>) -> SemanticSnapshot {
    SemanticSnapshot::new(
        version,
        ResolvedSemanticProject::from_deltas(deltas).unwrap(),
    )
    .unwrap()
}
async fn commit(client: &StoreClient, snapshot: SemanticSnapshot) -> CommitReceipt {
    client
        .commit(tag(), snapshot, None, CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value
}
fn compatibility(path: &str) -> CompatibilityFile {
    let syntax = SyntaxMetadata {
        language: Some("kotlin".into()),
        parent: Some("class_declaration".into()),
        kind: Some("identifier".into()),
        is_named: Some(false),
        field_names: Some("name,type".into()),
        hierarchy_kind: Some("source_file/class_declaration/identifier".into()),
    };
    let range = TextRange::new(ByteOffset(6), ByteOffset(12)).unwrap();
    CompatibilityFile {
        path: path.into(),
        definitions: vec![CompatibilityDefinition {
            name: "Stored".into(),
            kind: CompatibilitySymbolKind::Class,
            range_bytes: range,
            container: Some("demo".into()),
            parent_key: Some("stable-parent".into()),
            line: Some(LineIndex(0)),
            start_utf16_column: Some(Utf16Column(6)),
            language: Some("kotlin".into()),
            syntax: syntax.clone(),
        }],
        usages: vec![CompatibilityUsage {
            name: "Stored".into(),
            range_bytes: range,
            line: LineIndex(0),
            start_utf16_column: Utf16Column(6),
            end_utf16_column: Utf16Column(12),
            declaration: true,
            container: Some("demo".into()),
            parent_key: Some("stable-parent".into()),
            syntax,
        }],
    }
}

#[tokio::test]
async fn disk_reload_preserves_facts_all_type_shapes_generations_and_compatibility_without_java() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join(".kode/db")).unwrap();
    fs::write(root.join(".kode/db/keep.mv.db"), b"legacy reference").unwrap();
    let worker = StoreWorker::start(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    assert_eq!(worker.status, CacheStatus::Fresh);
    let client = worker.client();
    let mut first = delta("Stored.kt", "class Stored { val value: Long = 0 }\n");
    first.file.parse_version = u64::MAX;
    first.file.semantic_version = u64::MAX;
    let second = delta("Use.kt", "fun use(stored: Stored) {}\n");
    let mut project = ResolvedSemanticProject::from_deltas([first, second]).unwrap();
    let owner = project
        .symbols
        .iter()
        .find(|s| s.name == "Stored")
        .unwrap()
        .id;
    let member = project
        .symbols
        .iter()
        .find(|s| s.name == "value")
        .unwrap()
        .id;
    let types = [
        TypeRef::named(owner),
        TypeRef::primitive("Long"),
        TypeRef::Nullable {
            inner: Box::new(TypeRef::Generic {
                base: Box::new(TypeRef::named(owner)),
                arguments: vec![TypeRef::primitive("String")],
            }),
        },
        TypeRef::Union {
            alternatives: vec![TypeRef::named(owner), TypeRef::Unknown],
        },
        TypeRef::Function {
            parameters: vec![TypeRef::primitive("Int")],
            returns: Some(Box::new(TypeRef::named(owner))),
        },
        TypeRef::Unknown,
    ];
    for ty in &types {
        let id = SemanticIds::type_ref(ty);
        project.types.insert(
            id,
            TypeRecord {
                id,
                r#ref: ty.clone(),
                confidence: Confidence::inferred(0.87).unwrap(),
                knowledge_level: TypeKnowledgeLevel::CrossFileResolved,
            },
        );
    }
    project
        .symbols
        .iter_mut()
        .find(|s| s.id == member)
        .unwrap()
        .declared_type_id = Some(SemanticIds::type_ref(&TypeRef::primitive("Long")));
    let from = SemanticIds::file("Use.kt");
    let to = SemanticIds::file("Stored.kt");
    project.dependencies.push(FileDependencyRecord {
        from_file_id: from,
        to_file_id: to,
        kind: FileDependencyKind::TypeReference,
        generation: u64::MAX,
    });
    project.resolution_generations.insert(from, u64::MAX);
    let prepared = SemanticSnapshot::new(7, project).unwrap();
    let legacy = compatibility("Stored.kt");
    let reply = client
        .commit(
            tag(),
            prepared.clone(),
            Some(vec![legacy.clone()]),
            CancellationToken::new(),
        )
        .unwrap()
        .resolve()
        .await
        .unwrap();
    assert_eq!(reply.tag.workspace, WorkspaceGeneration(7));
    assert_eq!(reply.value.revision, 7);
    let held = client.snapshot().unwrap();
    worker.shutdown().await.unwrap();
    assert_eq!(
        fs::read(root.join(".kode/db/keep.mv.db")).unwrap(),
        b"legacy reference"
    );
    assert!(root.join(".kode-rs/index.sqlite").is_file());
    let worker = StoreWorker::start(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    assert_eq!(worker.status, CacheStatus::Ready);
    let client = worker.client();
    let loaded = client
        .load(tag(), CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value;
    assert_eq!(loaded.snapshot.version(), 7);
    assert_eq!(loaded.snapshot.project().types, prepared.project().types);
    assert_eq!(
        loaded.snapshot.project().dependencies,
        prepared.project().dependencies
    );
    assert_eq!(loaded.snapshot.resolution_generation(from), u64::MAX);
    assert_eq!(loaded.compatibility, vec![legacy]);
    assert_eq!(
        loaded
            .snapshot
            .file(SemanticIds::file("Stored.kt"))
            .unwrap()
            .parse_version,
        u64::MAX
    );
    assert_eq!(
        loaded
            .snapshot
            .members(&TypeRef::named(owner))
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["value"]
    );
    assert!(std::ptr::eq(
        loaded.snapshot.project(),
        client.snapshot().unwrap().project()
    ));
    assert_eq!(held.version(), 7);
    let receipt = commit(&client, loaded.snapshot.clone()).await;
    assert!(receipt.reused);
    assert!(std::ptr::eq(
        loaded.snapshot.project(),
        client.snapshot().unwrap().project()
    ));
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn duplicate_symbols_expressions_and_compatibility_records_are_canonical_before_persistence()
{
    let worker = StoreWorker::start(StoreLocation::Memory).await.unwrap();
    let client = worker.client();
    let mut facts = delta("Stored.kt", "class Stored\nval value = 1\n");
    facts.symbols.extend(facts.symbols.clone());
    let literal = facts.expression_types[0].clone();
    let mut weaker = literal.clone();
    weaker.r#type = TypeRef::primitive("Long");
    weaker.confidence = Confidence::new(SemanticSource::Heuristic, 0.5).unwrap();
    let mut empty = literal.clone();
    empty.range.end = empty.range.start;
    facts.expression_types.extend([weaker, empty]);
    facts.lexical_tokens.extend(facts.lexical_tokens.clone());
    let mut legacy = compatibility("Stored.kt");
    legacy.definitions.extend(legacy.definitions.clone());
    legacy.usages.extend(legacy.usages.clone());
    client
        .commit(
            tag(),
            snapshot(1, [facts]),
            Some(vec![legacy]),
            CancellationToken::new(),
        )
        .unwrap()
        .resolve()
        .await
        .unwrap();
    let loaded = client
        .load(tag(), CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value;
    assert_eq!(
        loaded
            .snapshot
            .workspace_symbols()
            .filter(|s| s.name == "Stored")
            .count(),
        1
    );
    assert_eq!(
        loaded.snapshot.project().deltas[&literal.file_id]
            .expression_types
            .len(),
        1
    );
    assert_eq!(
        loaded
            .snapshot
            .expression_type(literal.file_id, literal.range),
        Some(&TypeRef::primitive("Int"))
    );
    assert_eq!(loaded.compatibility[0].definitions.len(), 1);
    assert_eq!(loaded.compatibility[0].usages.len(), 1);
    assert!(commit(&client, loaded.snapshot).await.reused);
    commit(
        &client,
        snapshot(2, [delta("Stored.kt", "class Stored\nval value = 2000\n")]),
    )
    .await;
    assert!(
        client
            .load(tag(), CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap()
            .value
            .compatibility
            .is_empty()
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn one_file_replacement_retains_dependent_resolved_facts_and_generations() {
    let worker = StoreWorker::start(StoreLocation::Memory).await.unwrap();
    let client = worker.client();
    let first = delta("Model.kt", "class Model { val value = 1 }\n");
    let consumer = delta("Use.kt", "fun use(model: Model) { model.value }\n");
    let mut project = ResolvedSemanticProject::from_deltas([first, consumer]).unwrap();
    let member = project
        .symbols
        .iter()
        .find(|s| s.name == "value")
        .unwrap()
        .id;
    let model = SemanticIds::file("Model.kt");
    let consumer = SemanticIds::file("Use.kt");
    project
        .occurrences
        .iter_mut()
        .find(|o| o.file_id == consumer && o.text == "value")
        .unwrap()
        .resolved_symbol_id = Some(member);
    project.dependencies.push(FileDependencyRecord {
        from_file_id: consumer,
        to_file_id: model,
        kind: FileDependencyKind::MemberReference,
        generation: 11,
    });
    project.resolution_generations.insert(consumer, 11);
    commit(&client, SemanticSnapshot::new(1, project).unwrap()).await;
    let before = client.snapshot().unwrap();
    let mut changed = before.project().clone();
    let mut next = delta("Model.kt", "class Model { val value = 2 }\n");
    next.file.parse_version = 2;
    next.file.semantic_version = 2;
    changed.deltas.insert(model, Arc::new(next));
    let receipt = commit(&client, SemanticSnapshot::new(2, changed).unwrap()).await;
    assert_eq!(receipt.changed_files, [model]);
    assert!(receipt.removed_files.is_empty());
    let loaded = client
        .load(tag(), CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value
        .snapshot;
    assert_eq!(
        loaded.references(member).count(),
        before.references(member).count()
    );
    assert!(loaded.references(member).any(|o| o.file_id == consumer));
    assert_eq!(loaded.resolution_generation(consumer), 11);
    assert_eq!(loaded.dependencies(consumer).count(), 1);
    assert_eq!(before.version(), 1);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_cancelled_stale_and_cleared_revisions_preserve_snapshot_and_drain_critical_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let worker = StoreWorker::start(StoreLocation::Project(directory.path().into()))
        .await
        .unwrap();
    let client = worker.client();
    commit(&client, snapshot(1, [delta("Stored.kt", "class Before\n")])).await;
    let held = client.snapshot().unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        client
            .commit(
                tag(),
                snapshot(2, [delta("Stored.kt", "class After\n")]),
                None,
                cancel
            )
            .unwrap()
            .resolve()
            .await,
        Err(kode_core::DomainError::Cancelled)
    ));
    assert!(
        client
            .commit(
                tag(),
                snapshot(1, [delta("Stored.kt", "class Other\n")]),
                None,
                CancellationToken::new()
            )
            .unwrap()
            .resolve()
            .await
            .is_err()
    );
    assert!(std::ptr::eq(
        held.project(),
        client.snapshot().unwrap().project()
    ));
    let ticket = client
        .commit(
            tag(),
            snapshot(2, [delta("Stored.kt", "class After\n")]),
            None,
            CancellationToken::new(),
        )
        .unwrap();
    drop(ticket);
    worker.shutdown().await.unwrap();
    let worker = StoreWorker::start(StoreLocation::Project(directory.path().into()))
        .await
        .unwrap();
    let client = worker.client();
    assert!(
        client
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "After")
    );
    let receipt = client
        .clear(tag(), 3, CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap()
        .value;
    assert_eq!(receipt.removed_files, [SemanticIds::file("Stored.kt")]);
    assert_eq!(client.snapshot().unwrap().workspace_symbols().count(), 0);
    assert!(held.workspace_symbols().any(|s| s.name == "Before"));
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn owned_incompatible_cache_rebuilds_and_foreign_or_corrupt_files_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let worker = StoreWorker::start(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    commit(
        &worker.client(),
        snapshot(1, [delta("Stored.kt", "class Stored\n")]),
    )
    .await;
    worker.shutdown().await.unwrap();
    let db = rusqlite::Connection::open(root.join(".kode-rs/index.sqlite")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let worker = StoreWorker::start(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    assert_eq!(
        worker.status,
        CacheStatus::Rebuilt {
            previous_version: 99
        }
    );
    assert_eq!(
        worker
            .client()
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .count(),
        0
    );
    worker.shutdown().await.unwrap();
    let db = rusqlite::Connection::open(root.join(".kode-rs/index.sqlite")).unwrap();
    db.execute(
        "UPDATE meta SET value='old-byte-model' WHERE key='format'",
        [],
    )
    .unwrap();
    drop(db);
    let worker = StoreWorker::start(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    assert_eq!(
        worker.status,
        CacheStatus::Rebuilt {
            previous_version: 1
        }
    );
    worker.shutdown().await.unwrap();
    let worker = StoreWorker::rebuild(StoreLocation::Project(root.into()))
        .await
        .unwrap();
    assert_eq!(
        worker.status,
        CacheStatus::Rebuilt {
            previous_version: 1
        }
    );
    worker.shutdown().await.unwrap();
    let foreign = tempfile::tempdir().unwrap();
    fs::create_dir(foreign.path().join(".kode-rs")).unwrap();
    let path = foreign.path().join(".kode-rs/index.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE important(value TEXT); INSERT INTO important VALUES ('keep');")
        .unwrap();
    drop(db);
    let bytes = fs::read(&path).unwrap();
    assert!(
        StoreWorker::start(StoreLocation::Project(foreign.path().into()))
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let corrupt = tempfile::tempdir().unwrap();
    fs::create_dir(corrupt.path().join(".kode-rs")).unwrap();
    let path = corrupt.path().join(".kode-rs/index.sqlite");
    fs::write(&path, b"preserve unknown file").unwrap();
    assert!(
        StoreWorker::start(StoreLocation::Project(corrupt.path().into()))
            .await
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"preserve unknown file");
}

#[tokio::test]
async fn another_writer_requires_reload_and_cannot_silently_replace_a_newer_revision() {
    let directory = tempfile::tempdir().unwrap();
    let location = StoreLocation::Project(directory.path().into());
    let first = StoreWorker::start(location.clone()).await.unwrap();
    let second = StoreWorker::start(location).await.unwrap();
    let a = first.client();
    let b = second.client();
    commit(&a, snapshot(1, [delta("Stored.kt", "class Before\n")])).await;
    assert!(
        b.commit(
            tag(),
            snapshot(2, [delta("Stored.kt", "class After\n")]),
            None,
            CancellationToken::new()
        )
        .unwrap()
        .resolve()
        .await
        .is_err()
    );
    b.load(tag(), CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap();
    commit(&b, snapshot(2, [delta("Stored.kt", "class After\n")])).await;
    assert_eq!(a.snapshot().unwrap().version(), 1);
    a.load(tag(), CancellationToken::new())
        .unwrap()
        .resolve()
        .await
        .unwrap();
    assert_eq!(a.snapshot().unwrap().version(), 2);
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cache_directory_file_and_sidecar_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    for target in ["directory", "file", "journal"] {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"keep").unwrap();
        if target == "directory" {
            symlink(outside.path(), root.path().join(".kode-rs")).unwrap();
        } else {
            fs::create_dir(root.path().join(".kode-rs")).unwrap();
            let name = if target == "file" {
                "index.sqlite"
            } else {
                "index.sqlite-journal"
            };
            symlink(
                outside.path().join("keep"),
                root.path().join(".kode-rs").join(name),
            )
            .unwrap();
        }
        assert!(
            StoreWorker::start(StoreLocation::Project(root.path().into()))
                .await
                .is_err()
        );
        assert_eq!(fs::read(outside.path().join("keep")).unwrap(), b"keep");
    }
}
