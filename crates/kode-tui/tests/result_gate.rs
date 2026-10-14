use kode_core::{DocumentId, Revision, WorkspaceGeneration};
use kode_tui::app::events::ResultGate;

#[test]
fn independent_owners_never_collide_in_the_shared_worker_queue() {
    let mut first = ResultGate::new(WorkspaceGeneration(1));
    let mut second = ResultGate::new(WorkspaceGeneration(1));
    assert_ne!(
        first.request("load".into(), None).request,
        second.request("index".into(), None).request
    );
}

#[test]
fn superseded_jobs_document_edits_workspace_switch_and_popup_dismissal_reject_results() {
    let mut gate = ResultGate::new(WorkspaceGeneration(1));
    let a = gate.request("file-load".into(), None);
    let b = gate.request("file-load".into(), None);
    assert!(!gate.accepts("file-load", a));
    assert!(gate.accepts("file-load", b));
    let preview = gate.request("definition".into(), Some((DocumentId(1), Revision(3))));
    gate.document_changed(DocumentId(1), Revision(4));
    assert!(!gate.accepts("definition", preview));
    let stale_snapshot = gate.request("completion".into(), Some((DocumentId(1), Revision(3))));
    assert!(!gate.accepts("completion", stale_snapshot));
    let fresh = gate.request("definition".into(), Some((DocumentId(1), Revision(4))));
    gate.dismiss("definition");
    assert!(!gate.accepts("definition", fresh));
    let fresh = gate.request("definition".into(), Some((DocumentId(1), Revision(4))));
    gate.document_closed(DocumentId(1));
    assert!(!gate.accepts("definition", fresh));
    gate.switch_workspace();
    assert!(!gate.accepts("file-load", b));
}
