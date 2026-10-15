use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, JobTag, RequestId, WorkspaceGeneration,
    document::Document,
    intelligence::{IntelligenceProvider, IntelligenceRequest},
};
use kode_intelligence::{
    facade::{IntelligenceResources, source_path},
    ids::SemanticIds,
    service::{CodeIntelService, IndexPhase},
    tokens::SemanticTokenKind,
};
use kode_platform::jobs::{
    CancellationToken, Delivery, MonotonicClock, Notifications, OwnerId, Scheduler,
};
use kode_workspace::{mime::MimeDetector, project::Project};
use std::{fs, path::Path, sync::Arc};
fn document(path: &Path, text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(path.into()),
        Some("kotlin".into()),
        text,
    )
}
fn request(document: &Document, offset: usize) -> IntelligenceRequest {
    IntelligenceRequest {
        tag: JobTag {
            workspace: WorkspaceGeneration(1),
            request: RequestId::fresh().unwrap(),
            document: Some((document.id(), document.revision())),
        },
        snapshot: document.snapshot().to_service_snapshot(),
        position: ByteOffset(offset),
    }
}
async fn setup() -> (tempfile::TempDir, Project, CodeIntelService) {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("src")).unwrap();
    let project = Project::new(directory.path(), WorkspaceGeneration(1)).unwrap();
    let service = CodeIntelService::open(
        &project,
        Arc::new(IntelligenceResources::compiled().unwrap()),
        MimeDetector::compiled(),
    )
    .await
    .unwrap();
    (directory, project, service)
}
async fn drive(service: &mut CodeIntelService, now: u64) {
    while let Some(job) = service.next_job(now).unwrap() {
        let tag = job.tag;
        let prepared = tokio::task::spawn_blocking(move || job.run(CancellationToken::new()))
            .await
            .unwrap();
        if let Some(commit) = service.prepared(tag, prepared, now).unwrap() {
            service.committed(tag, commit.run().await, now).unwrap();
        }
    }
}

#[tokio::test]
async fn actual_bounded_scheduler_runs_scan_extract_resolve_persist_publish_and_native_editor_queries()
 {
    let (directory, project, mut service) = setup().await;
    let model = directory.path().join("src/Customer.kt");
    let usage = directory.path().join("src/Use.kt");
    fs::write(
        &model,
        "class Customer { val id: Long = 0; val name: String = \"\"; fun save() {} }\n",
    )
    .unwrap();
    let text = "fun getCustomer(): Customer = Customer()\nfun use() { val customer = getCustomer(); customer.\n}\n";
    fs::write(&usage, text).unwrap();
    let document = document(&usage, text);
    service.observe(document.snapshot(), 0).unwrap();
    service.rescan(project, false).unwrap();
    let job = service.next_job(0).unwrap().unwrap();
    let tag = job.tag;
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    scheduler
        .submit_blocking(OwnerId(7), tag, Delivery::Cancellable, move |cancel| {
            job.run(cancel)
        })
        .unwrap();
    let completion = scheduler.next().await.unwrap();
    assert_eq!(service.snapshot().unwrap().version(), 0);
    let commit = service
        .prepared(tag, completion.result, 0)
        .unwrap()
        .unwrap();
    assert_eq!(service.status().unwrap().phase, IndexPhase::Persisting);
    assert!(service.committed(tag, commit.run().await, 0).unwrap());
    let snapshot = service.snapshot().unwrap();
    assert_eq!(snapshot.version(), 1);
    assert_eq!(snapshot.project().deltas.len(), 2);
    let request = request(
        &document,
        text.find("customer.\n").unwrap() + "customer.".len(),
    );
    let result = service.facade().completions(&request).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|c| c.label.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        ["id", "name", "save"].into_iter().collect()
    );
    assert_eq!(service.take_changes().len(), 1);
    assert!(scheduler.shutdown().await.is_empty());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn debounced_document_identity_race_drops_old_preparation_without_publishing_or_losing_latest_input()
 {
    let (directory, _, mut service) = setup().await;
    let path = directory.path().join("Race.kt");
    let old = document(&path, "class Before\n");
    let new = document(&path, "class After\n");
    service.observe(old.snapshot(), 10).unwrap();
    assert!(service.next_job(209).unwrap().is_none());
    let job = service.next_job(210).unwrap().unwrap();
    let tag = job.tag;
    let prepared = job.run(CancellationToken::new()).unwrap();
    service.observe(new.snapshot(), 220).unwrap();
    assert_eq!(old.revision(), new.revision());
    assert_ne!(old.id(), new.id());
    assert!(service.prepared(tag, Ok(prepared), 220).unwrap().is_none());
    assert_eq!(service.snapshot().unwrap().version(), 0);
    assert!(service.next_job(419).unwrap().is_none());
    drive(&mut service, 420).await;
    let snapshot = service.snapshot().unwrap();
    assert!(snapshot.workspace_symbols().any(|s| s.name == "After"));
    assert!(!snapshot.workspace_symbols().any(|s| s.name == "Before"));
    assert!(service.facade().definitions(&request(&old, 8)).is_err());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_rescan_discards_prepared_facts_and_retains_independent_live_edits() {
    let (directory, project, mut service) = setup().await;
    let file = directory.path().join("src/Disk.kt");
    fs::write(&file, "class Disk\n").unwrap();
    service.rescan(project.clone(), false).unwrap();
    let job = service.next_job(0).unwrap().unwrap();
    let tag = job.tag;
    let prepared = job.run(CancellationToken::new()).unwrap();
    assert_eq!(service.cancel_rescan().unwrap(), Some(tag));
    assert!(service.prepared(tag, Ok(prepared), 0).unwrap().is_none());
    assert_eq!(service.status().unwrap().phase, IndexPhase::Cancelled);
    assert_eq!(service.snapshot().unwrap().version(), 0);
    assert!(service.next_job(500).unwrap().is_none());
    let live = document(&directory.path().join("Live.kt"), "class Live\n");
    service.observe(live.snapshot(), 1).unwrap();
    service.rescan(project, false).unwrap();
    service.cancel_rescan().unwrap();
    drive(&mut service, 201).await;
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Live")
    );
    assert!(
        !service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Disk")
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn freshness_is_checked_at_commit_submission_and_accepted_transactions_drain_before_latest_retry()
 {
    let (directory, _, mut service) = setup().await;
    let path = directory.path().join("Race.kt");
    let first = document(&path, "class First\n");
    service.observe(first.snapshot(), 0).unwrap();
    let job = service.next_job(200).unwrap().unwrap();
    let tag = job.tag;
    let commit = service
        .prepared(tag, job.run(CancellationToken::new()), 200)
        .unwrap()
        .unwrap();
    let second = document(&path, "class Second\n");
    service.observe(second.snapshot(), 210).unwrap();
    let result = commit.run().await;
    assert!(matches!(result, Err(kode_core::DomainError::Cancelled)));
    service.committed(tag, result, 210).unwrap();
    assert_eq!(service.snapshot().unwrap().version(), 0);
    drive(&mut service, 410).await;
    let held = service.snapshot().unwrap();
    let third = document(&path, "class Third\n");
    service.observe(third.snapshot(), 500).unwrap();
    let job = service.next_job(700).unwrap().unwrap();
    let tag = job.tag;
    let commit = service
        .prepared(tag, job.run(CancellationToken::new()), 700)
        .unwrap()
        .unwrap();
    let connection =
        rusqlite::Connection::open(directory.path().join(".kode-rs/index.sqlite")).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let mut future = Box::pin(commit.run());
    std::future::poll_fn(|context| {
        assert!(matches!(
            std::future::Future::poll(future.as_mut(), context),
            std::task::Poll::Pending
        ));
        std::task::Poll::Ready(())
    })
    .await;
    let fourth = document(&path, "class Fourth\n");
    service.observe(fourth.snapshot(), 710).unwrap();
    connection.execute_batch("COMMIT").unwrap();
    service.committed(tag, future.await, 710).unwrap();
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Third")
    );
    assert!(held.workspace_symbols().any(|s| s.name == "Second"));
    drive(&mut service, 910).await;
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Fourth")
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn unsaved_queries_include_other_captured_buffers_and_never_publish_their_private_snapshot() {
    let (directory, project, mut service) = setup().await;
    let model_path = directory.path().join("src/Customer.kt");
    let use_path = directory.path().join("src/Use.kt");
    fs::write(&model_path, "class Customer { val id: Int = 0 }\n").unwrap();
    let source = "fun use(customer: Customer) { customer.\n}\n";
    fs::write(&use_path, source).unwrap();
    service.rescan(project, false).unwrap();
    drive(&mut service, 0).await;
    let old_snapshot = service.snapshot().unwrap();
    let dirty_model = document(
        &model_path,
        "class Customer { val pendingName: String = \"\" }\n",
    );
    let usage = document(&use_path, source);
    service.observe(dirty_model.snapshot(), 1).unwrap();
    service.observe(usage.snapshot(), 1).unwrap();
    let result = service
        .facade()
        .completions(&request(&usage, source.find("customer.\n").unwrap() + 9))
        .unwrap();
    assert_eq!(
        result.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
        vec!["pendingName"]
    );
    assert_eq!(
        service.snapshot().unwrap().version(),
        old_snapshot.version()
    );
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "id")
    );
    assert!(fs::read_to_string(&model_path).unwrap().contains("val id"));
    assert_eq!(
        service
            .facade()
            .captured_source(model_path.to_str().unwrap())
            .unwrap()
            .unwrap()
            .text()
            .as_ref(),
        dirty_model.snapshot().text().as_ref()
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn tokens_navigation_references_and_missing_source_metadata_use_semantic_identity_and_raw_bytes()
 {
    let (directory, project, mut service) = setup().await;
    let model = directory.path().join("src/Model.kt");
    let usage = directory.path().join("src/Use.kt");
    fs::write(&model, "package demo\r\nclass Café { fun save() {} }\r\n").unwrap();
    let source = "package demo\r\nfun use(value: Café) { value.save(); val café = Café() }\r\n";
    fs::write(&usage, source).unwrap();
    service.rescan(project, false).unwrap();
    drive(&mut service, 0).await;
    let document = document(&usage, source);
    let facade = service.facade();
    let req = request(&document, source.rfind("Café()").unwrap() + 2);
    let definitions = facade.definitions(&req).unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].path, model);
    assert!(!definitions[0].missing_source);
    assert!(definitions[0].declaration_range.is_some());
    let references = facade.references(&req).unwrap();
    assert_eq!(references.len(), 3);
    let report = facade.tokens(&req.snapshot, None).unwrap();
    assert_eq!(report.index_revision, service.snapshot().unwrap().version());
    assert_eq!(report.document, (document.id(), document.revision()));
    assert!(
        report
            .tokens
            .iter()
            .any(|t| t.kind == SemanticTokenKind::Method
                && &source[t.range.start.0..t.range.end.0] == "save")
    );
    assert!(
        report
            .tokens
            .iter()
            .any(|t| t.kind == SemanticTokenKind::Class
                && &source[t.range.start.0..t.range.end.0] == "Café")
    );
    assert!(
        report
            .tokens
            .iter()
            .flat_map(|t| t.highlights())
            .any(|h| h.class == "codeintel.declaration")
    );
    for token in &report.tokens {
        token.range.validate(source).unwrap();
    }
    fs::remove_file(&model).unwrap();
    assert!(facade.definitions(&req).unwrap()[0].missing_source);
    let mut wrong = req.clone();
    wrong.tag.workspace = WorkspaceGeneration(2);
    assert!(matches!(
        facade.definitions(&wrong),
        Err(kode_core::DomainError::Cancelled)
    ));
    service.shutdown().await.unwrap();
    assert!(matches!(
        facade.definitions(&req),
        Err(kode_core::DomainError::Cancelled)
    ));
}

#[tokio::test]
async fn rescan_remove_clear_rebuild_and_restart_keep_generations_and_publication_events_consistent()
 {
    let (directory, project, mut service) = setup().await;
    let model = directory.path().join("src/Model.kt");
    fs::write(&model, "class Before\n").unwrap();
    service.rescan(project.clone(), false).unwrap();
    drive(&mut service, 0).await;
    let file = SemanticIds::file(model.to_str().unwrap());
    assert_eq!(service.snapshot().unwrap().resolution_generation(file), 1);
    let revision = service.snapshot().unwrap().version();
    service.shutdown().await.unwrap();
    let mut service = CodeIntelService::open(
        &project,
        Arc::new(IntelligenceResources::compiled().unwrap()),
        MimeDetector::compiled(),
    )
    .await
    .unwrap();
    assert_eq!(service.snapshot().unwrap().version(), revision);
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Before")
    );
    fs::write(&model, "class After\n").unwrap();
    service.rescan(project.clone(), false).unwrap();
    drive(&mut service, 0).await;
    assert_eq!(service.snapshot().unwrap().resolution_generation(file), 2);
    fs::remove_file(&model).unwrap();
    service.rescan(project.clone(), false).unwrap();
    drive(&mut service, 0).await;
    assert!(service.snapshot().unwrap().file(file).is_none());
    assert!(
        service
            .take_changes()
            .iter()
            .any(|change| change.removed_files.contains(&file))
    );
    fs::write(&model, "class Rebuilt\n").unwrap();
    service.rescan(project.clone(), true).unwrap();
    drive(&mut service, 0).await;
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Rebuilt")
    );
    service.clear(project).unwrap();
    drive(&mut service, 0).await;
    assert_eq!(service.status().unwrap().files, 0);
    assert!(service.snapshot().unwrap().project().deltas.is_empty());
    assert!(fs::read_to_string(&model).unwrap().contains("Rebuilt"));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn unavailable_owned_cache_uses_visible_memory_fallback_and_retains_foreign_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let project = Project::new(directory.path(), WorkspaceGeneration(1)).unwrap();
    fs::create_dir(directory.path().join(".kode-rs")).unwrap();
    let path = directory.path().join(".kode-rs/index.sqlite");
    fs::write(&path, b"foreign cache bytes").unwrap();
    let mut service = CodeIntelService::open(
        &project,
        Arc::new(IntelligenceResources::compiled().unwrap()),
        MimeDetector::compiled(),
    )
    .await
    .unwrap();
    assert!(
        service
            .status()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("using memory"))
    );
    let doc = document(&directory.path().join("Open.kt"), "class Memory\n");
    service.observe(doc.snapshot(), 0).unwrap();
    drive(&mut service, 200).await;
    assert!(
        service
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "Memory")
    );
    assert_eq!(fs::read(path).unwrap(), b"foreign cache bytes");
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn restart_requalifies_unchanged_source_against_current_external_definition_patterns() {
    use kode_intelligence::frontend::legacy::DefinitionRegistry;
    let directory = tempfile::tempdir().unwrap();
    let project = Project::new(directory.path(), WorkspaceGeneration(1)).unwrap();
    let resources = |pattern: &str| {
        let mut resources = IntelligenceResources::compiled().unwrap();
        let json = serde_json::json!({"languages":[{"language":"custom", "patterns":[{"kind":"FUNCTION","regex":pattern}]}]});
        resources.definitions = Arc::new(DefinitionRegistry::from_json(&json.to_string()).unwrap());
        Arc::new(resources)
    };
    let source = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(directory.path().join("Source.custom")),
        Some("custom".into()),
        "spice_run()",
    );
    let mut first = CodeIntelService::open(
        &project,
        resources(r"spice_([A-Za-z_]+)"),
        MimeDetector::compiled(),
    )
    .await
    .unwrap();
    first.observe(source.snapshot(), 0).unwrap();
    drive(&mut first, 200).await;
    assert!(
        first
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "run")
    );
    first.shutdown().await.unwrap();
    let mut second = CodeIntelService::open(
        &project,
        resources(r"(spice_[A-Za-z_]+)"),
        MimeDetector::compiled(),
    )
    .await
    .unwrap();
    second.observe(source.snapshot(), 0).unwrap();
    drive(&mut second, 200).await;
    assert!(
        second
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "spice_run")
    );
    assert!(
        !second
            .snapshot()
            .unwrap()
            .workspace_symbols()
            .any(|s| s.name == "run")
    );
    second.shutdown().await.unwrap();
}

#[test]
fn virtual_source_identity_depends_on_owner_and_key_and_never_on_display_label() {
    let origin = |owner: &str, key: &str, label: &str| DocumentOrigin::Virtual {
        owner: owner.into(),
        key: key.into(),
        label: label.into(),
    };
    assert_eq!(
        source_path(&origin("a:b", "c", "one")).unwrap(),
        source_path(&origin("a:b", "c", "two")).unwrap()
    );
    assert_ne!(
        source_path(&origin("a:b", "c", "same")).unwrap(),
        source_path(&origin("a", "b:c", "same")).unwrap()
    );
}
