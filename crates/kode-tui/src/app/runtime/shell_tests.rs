use super::*;

#[tokio::test]
async fn shell_suspension_discards_pending_paste_but_preserves_copy_publication() {
    let directory = tempfile::tempdir().unwrap();
    let prepared = workspace::prepare_workspace(
        directory.path(),
        kode_core::WorkspaceGeneration(1),
        None,
        &kode_platform::jobs::CancellationToken::new(),
    )
    .unwrap();
    let clipboard = Arc::new(kode_platform::clipboard::ClipboardService::internal());
    let mut app = AppRuntime::with_services(
        prepared,
        clipboard.clone(),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    let read = app.state.tag().unwrap();
    app.clipboard_work(read, ClipboardJob::Read, ClipboardTarget::None)
        .unwrap();
    let copy = app.state.tag().unwrap();
    let epoch = clipboard.remember("remembered before shell").unwrap();
    app.clipboard_work(copy, ClipboardJob::Publish(epoch), ClipboardTarget::None)
        .unwrap();
    app.suspend_interactions().unwrap();
    assert!(!app.state.window_focused);
    assert!(!app.pending.iter().any(|work| work.tag == read));
    assert!(!app.latest.values().any(|tag| *tag == read));
    assert!(app.pending.iter().any(|work| work.tag == copy));
    assert_eq!(clipboard.local_text(), "remembered before shell");
    // Closing while unfocused follows the same persistence/cleanup path as a
    // focused Ctrl+Q. This is required for ttyd's SIGHUP and SSH disconnects.
    app.state.request_quit().unwrap();
    assert!(app.state.closing && app.state.quit_requested);
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
