use super::*;
use kode_platform::jobs::CancellationToken;
#[tokio::test]
async fn superseded_cancellable_jobs_release_their_context_proofs_without_a_completion() {
    let directory = tempfile::tempdir().unwrap();
    let prepared = crate::app::workspace::prepare_workspace(
        directory.path(),
        kode_core::WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(kode_platform::clipboard::ClipboardService::internal()),
        Arc::new(kode_platform::jobs::MonotonicClock::default()),
    )
    .unwrap();
    // The worker does not capture this value. Its only remaining strong owner is
    // the UI context, which cancellable delivery is allowed never to return.
    let collection = Arc::new(serde_json::json!({"info":{},"item":[]}));
    let proof = Arc::downgrade(&collection);
    let old = app.state.tag().unwrap();
    app.blocking(
        old,
        app.root_owner,
        "rest-retention",
        false,
        Context::Rest(Box::new(crate::panels::rest::state::Context::Editor(
            crate::panels::rest::editor::jobs::Context::Form {
                key: "old".into(),
                path: Default::default(),
                collection,
            },
        ))),
        Box::new(|cancel| {
            std::thread::sleep(std::time::Duration::from_millis(30));
            if cancel.is_cancelled() {
                Err(DomainError::Cancelled)
            } else {
                Err(DomainError::Unsupported("fixture".into()))
            }
        }),
    )
    .unwrap();
    app.pump().unwrap();
    assert!(app.contexts.contains_key(&old));
    assert!(proof.upgrade().is_some());
    let next = app.state.tag().unwrap();
    app.blocking(
        next,
        app.root_owner,
        "rest-retention",
        false,
        Context::Rest(Box::new(crate::panels::rest::state::Context::Run {
            structure: 0,
        })),
        Box::new(|_| Err(DomainError::Cancelled)),
    )
    .unwrap();
    assert!(!app.contexts.contains_key(&old));
    assert!(proof.upgrade().is_none());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
