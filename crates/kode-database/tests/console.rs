use kode_core::RequestId;
use kode_database::{
    console::Manager,
    execution::{Phase, Snapshot},
    model::{DataSourceDefinition, DatabaseProjectState},
};
use serde_json::json;
fn source() -> DataSourceDefinition {
    serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver"},"jdbcUrl":"jdbc:h2:mem:console","autoCommit":false})).unwrap()
}
#[test]
fn original_per_source_identities_title_counters_buffers_restore_context_and_unknown_fields() {
    let mut manager = Manager::default();
    let a = manager.create(&source()).unwrap();
    assert_eq!(a.state.id, "fixture:1");
    assert_eq!(a.state.title, "sql@Fixture:1");
    assert_eq!(a.state.buffer, "-- Fixture\n");
    assert!(!a.state.auto_commit);
    manager.remove("fixture:1").unwrap();
    assert_eq!(manager.create(&source()).unwrap().state.id, "fixture:2");
    let mut state = manager.project_state();
    state.extra.insert("future".into(), json!({"enabled":true}));
    state.sessions[0].schema = Some("OTHER".into());
    state.sessions[0].buffer = "select 'private SQL buffer';".into();
    state.sessions[0]
        .extra
        .insert("futureSession".into(), json!(5));
    let mut restored = Manager::default();
    restored.restore(state.clone(), None).unwrap();
    assert_eq!(restored.project_state(), state);
    assert_eq!(restored.create(&source()).unwrap().state.id, "fixture:3");
    assert!(!format!("{:?}", restored.get("fixture:2").unwrap()).contains("private SQL buffer"));
}
#[test]
fn stale_completions_cannot_finish_a_new_request_and_running_sessions_cannot_be_silently_removed() {
    let mut manager = Manager::default();
    manager.create(&source()).unwrap();
    manager.begin("fixture:1", RequestId(1)).unwrap();
    assert!(manager.begin("fixture:1", RequestId(2)).is_err());
    assert!(manager.remove("fixture:1").is_err());
    let final_result = Snapshot {
        phase: Phase::Finished,
        unknown_outcome: true,
        ..Default::default()
    };
    assert!(!manager.finish(
        "fixture:1",
        RequestId(2),
        "wrong".into(),
        0,
        final_result.clone()
    ));
    assert!(manager.finish(
        "fixture:1",
        RequestId(1),
        "captured SQL".into(),
        123,
        final_result
    ));
    let console = manager.get("fixture:1").unwrap();
    assert_eq!(console.history.len(), 1);
    assert!(console.history[0].unknown_outcome);
    assert!(!console.history[0].success);
    assert!(!format!("{:?}", console.history).contains("captured SQL"));
    manager.begin("fixture:1", RequestId(2)).unwrap();
    assert!(!manager.progress("fixture:1", RequestId(1), Snapshot::default()));
}
#[test]
fn restore_is_transactional_and_returns_orphans_and_history_is_bounded() {
    let mut manager = Manager::default();
    manager.create(&source()).unwrap();
    let state = manager.project_state();
    let mut invalid = state.clone();
    invalid.sessions.push(invalid.sessions[0].clone());
    assert!(manager.restore(invalid, None).is_err());
    assert_eq!(manager.project_state(), state);
    let mut restored = Manager::default();
    let missing = restored.restore(state, Some(&Default::default())).unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(restored.all().count(), 0);
    for index in 0..600 {
        let request = RequestId(index);
        manager.begin("fixture:1", request).unwrap();
        manager.finish(
            "fixture:1",
            request,
            format!("select {index}"),
            index,
            Snapshot {
                phase: Phase::Finished,
                ..Default::default()
            },
        );
    }
    assert_eq!(manager.get("fixture:1").unwrap().history.len(), 512);
    let _: DatabaseProjectState =
        serde_json::from_value(serde_json::to_value(manager.project_state()).unwrap()).unwrap();
}
#[test]
fn persistence_capture_is_stable_while_worker_replaces_captured_buffers() {
    let mut manager = Manager::default();
    manager.create(&source()).unwrap();
    let snapshot = manager.snapshot();
    manager.set_context("fixture:1", None, Some("NEW_SCHEMA".into()));
    manager.update_buffer("fixture:1", "newer live text".into());
    let captured = snapshot.into_state([("fixture:1".into(), "captured rope text".into())]);
    assert_eq!(captured.sessions[0].buffer, "captured rope text");
    assert_eq!(captured.sessions[0].schema, None);
    assert_eq!(
        manager.project_state().sessions[0].schema.as_deref(),
        Some("NEW_SCHEMA")
    );
    assert_eq!(
        manager.project_state().sessions[0].buffer,
        "newer live text"
    );
}
