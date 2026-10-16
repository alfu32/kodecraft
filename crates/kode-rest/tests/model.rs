use kode_rest::model::{NodeKind, Workspace};
use kode_rest::model::{NodePath, ProjectState, environment};
use serde_json::json;
#[test]
fn original_environment_management_active_variables_and_lossless_text_metadata_updates() {
    let mut model = Workspace::default();
    let dev = model.create_environment("dev");
    assert_eq!(model.active_environment().unwrap().id, dev);
    assert!(model.rename_environment(&dev, "development"));
    let copy = model.duplicate_environment(&dev, None).unwrap();
    assert!(model.activate_environment(Some(&copy)));
    assert!(model.remove_environment(&dev));
    assert_eq!(model.active_environment().unwrap().id, copy);
    let values=serde_json::from_value(json!([{"key":"token","value":"old","enabled":false,"type":"secret","secret":true,"description":"retain","future":42},{"key":"x","value":"first","enabled":true},{"key":"x","value":"last","enabled":true}])).unwrap();
    assert!(model.set_environment_values(&copy, values));
    assert!(model.update_environment_text(&copy, "token=changed\nx=one\nx=two"));
    let environment = model.environment(&copy).unwrap();
    assert!(!environment.values[0].enabled);
    assert!(environment.values[0].secret);
    assert_eq!(environment.values[0].extra["future"], 42);
    assert_eq!(environment.values[0].description, Some(json!("retain")));
    let variables = model.active_environment_variables();
    assert_eq!(variables.len(), 1);
    assert_eq!(variables["x"], "two");
    assert!(!model.update_environment_text(&copy, "token=changed\nx=one\nx=two"));
    assert!(model.activate_environment(None));
    assert!(model.active_environment().is_none());
    assert!(model.active_environment_variables().is_empty());
}
#[test]
fn original_creates_renames_duplicates_and_moves_postman_items() {
    let mut model = Workspace::default();
    let root = NodePath::default();
    let folder = model.add_folder(&root, "v1").unwrap();
    let request = model.add_request(&folder, "List").unwrap();
    model.rename(&request, "List Models").unwrap();
    let duplicate = model.duplicate(&request).unwrap();
    assert_ne!(
        model.node(&request).unwrap()["id"],
        model.node(&duplicate).unwrap()["id"]
    );
    let moved = model.move_node(&duplicate, &root, None).unwrap();
    assert_eq!(moved, NodePath(vec![1]));
    assert_eq!(model.node(&request).unwrap()["name"], "List Models");
    assert_eq!(model.node(&moved).unwrap()["name"], "List Models");
    assert_eq!(model.collection()["item"].as_array().unwrap().len(), 2);
    let tree = model.tree();
    assert_eq!(tree.len(), 4);
    assert_eq!(tree[0].kind, NodeKind::Collection);
    assert_eq!(tree[1].kind, NodeKind::Folder);
    assert_eq!(tree[2].depth, 2);
    assert_eq!(tree[3].method.as_deref(), Some("GET"));
}
#[test]
fn moving_into_a_later_sibling_rebases_destination_and_selected_descendant_without_losing_unknowns()
{
    let mut model = Workspace::default();
    let root = NodePath::default();
    let source = model.add_folder(&root, "Source").unwrap();
    let request = model.add_request(&source, "Original").unwrap();
    model
        .set_node_field(&request, "x-future", Some(json!({"keep":[false,null,1]})))
        .unwrap();
    let target = model.add_folder(&root, "Target").unwrap();
    model.select(&request);
    let original = model.node(&request).unwrap().clone();
    let moved = model.move_node(&source, &target, None).unwrap();
    assert_eq!(moved, NodePath(vec![0, 0]));
    assert_eq!(model.selected_path(), moved.child(0));
    assert_eq!(model.node(&moved.child(0)).unwrap(), &original);
    let before = model.export_collection();
    assert!(model.move_node(&moved, &moved.child(0), None).is_none());
    assert!(model.move_node(&root, &moved, None).is_none());
    assert_eq!(*model.export_collection(), *before);
    let returned = model.move_node(&moved, &root, Some(usize::MAX)).unwrap();
    assert_eq!(returned, NodePath(vec![1]));
    assert_eq!(model.selected_path(), returned.child(0));
    assert!(model.delete(&returned));
    assert_eq!(model.selected_path(), root);
}
#[test]
fn deletion_insertion_duplication_and_sibling_moves_preserve_the_selected_node_identity() {
    let mut model = Workspace::default();
    let root = NodePath::default();
    let first = model.add_request(&root, "first").unwrap();
    let second = model.add_request(&root, "second").unwrap();
    let third = model.add_request(&root, "third").unwrap();
    let id = model.node(&third).unwrap()["id"].clone();
    model.select(&third);
    model.duplicate(&first).unwrap();
    assert_eq!(model.selected_path(), NodePath(vec![3]));
    assert_eq!(model.node(&model.selected_path()).unwrap()["id"], id);
    assert!(model.delete(&first));
    assert_eq!(model.selected_path(), third);
    assert_eq!(model.node(&third).unwrap()["id"], id);
    let moved = model.move_up(&third).unwrap();
    assert_eq!(moved, second);
    assert_eq!(model.selected_path(), moved);
    assert_eq!(model.node(&moved).unwrap()["id"], id);
    let moved = model.move_down(&moved).unwrap();
    assert_eq!(moved, third);
    assert_eq!(model.node(&model.selected_path()).unwrap()["id"], id);
}
#[test]
fn copying_regenerates_nested_request_ids_and_preserves_nonobject_children_scripts_examples_and_variable_objects()
 {
    let collection = json!({"info":{"name":"Imported"},"item":[{"name":"Folder","event":[{"script":{"exec":["pm.test('kept')"]}}],"item":[null,7,{"id":"original","name":"Call","request":{"method":"GET","url":"https://example.invalid"},"response":[{"id":"example-id","body":"saved example"}],"variable":[{"value":{"nested":42}}],"x-item":"keep"}]}],"x-root":true});
    let mut model = Workspace::default();
    model.replace_collection(collection.clone()).unwrap();
    let original = model.project_state();
    let copy = model.duplicate(&NodePath(vec![0])).unwrap();
    assert_eq!(copy, NodePath(vec![1]));
    let cloned = model.node(&copy).unwrap();
    assert_eq!(cloned["item"][0], serde_json::Value::Null);
    assert_eq!(cloned["item"][1], 7);
    assert_ne!(cloned["item"][2]["id"], "original");
    assert_eq!(cloned["item"][2]["response"][0]["id"], "example-id");
    assert_eq!(cloned["event"], collection["item"][0]["event"]);
    assert_eq!(
        cloned["item"][2]["variable"],
        collection["item"][0]["item"][2]["variable"]
    );
    assert_eq!(*original.collection, collection);
    assert_eq!(model.collection()["x-root"], true);
    let exported = model.export_collection();
    let mut reloaded = Workspace::default();
    reloaded.replace_collection((*exported).clone()).unwrap();
    assert_eq!(reloaded.collection(), model.collection());
}
#[test]
fn restore_and_invalid_edits_are_transactional_and_ui_bounds_and_selection_are_repaired() {
    let mut model = Workspace::default();
    let root = NodePath::default();
    let request = model.add_request(&root, "Request").unwrap();
    let before = model.project_state();
    let revision = model.revision();
    assert!(
        model
            .replace_collection(json!({"info":{},"item":7}))
            .is_err()
    );
    assert!(model.set_node_field(&root, "info", None).is_err());
    assert_eq!(model.project_state(), before);
    assert_eq!(model.revision(), revision);
    assert!(model.add_folder(&request, "invalid parent").is_none());
    assert!(!model.delete(&root));
    assert!(!model.select(&NodePath(vec![usize::MAX])));
    let mut restore = before.clone();
    restore.ui.selected_path = Some("0/invalid/1".into());
    restore.active_environment_id = Some("missing".into());
    model.restore(restore).unwrap();
    assert!(model.selected_path().is_root());
    assert!(model.active_environment().is_none());
    model.set_response_panel_height(-1);
    model.set_environment_panel_height(500);
    model.set_pretty_print_responses(false);
    assert_eq!(model.ui().response_panel_height, 5);
    assert_eq!(model.ui().environment_panel_height, 60);
    assert!(!model.ui().pretty_print_responses);
    let mut invalid = model.project_state();
    let env = kode_rest::model::Environment::default();
    invalid.environments = std::sync::Arc::new(vec![env.clone(), env]);
    let before = model.project_state();
    assert!(model.restore(invalid).is_err());
    assert_eq!(model.project_state(), before);
}
#[test]
fn original_environment_codec_escapes_keys_values_and_preserves_unrecognized_backslashes() {
    let values = environment::parse("base\\=url=https://example.test\\\\api\nname=dev");
    assert_eq!(values[0].key, "base=url");
    assert_eq!(values[0].text(), "https://example.test\\api");
    assert_eq!(
        environment::format(&values),
        "base\\=url=https://example.test\\\\api\nname=dev"
    );
    let values =
        environment::parse("# comment\ninvalid\n=bad\n   =bad\n key = value\\q\\\r\nemoji=界😀\n");
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].key, "key");
    assert_eq!(values[0].text(), "value\\q\\");
    assert_eq!(values[1].text(), "界😀");
}
#[test]
fn collection_environment_metadata_and_unknown_fields_survive_typed_session_roundtrip_without_diagnostic_values()
 {
    let collection = json!({"info":{"name":"Imported","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json","x-info":{"keep":true}},"variable":[{"key":"object","value":{"arbitrary":[1,2,3]}}],"item":[{"id":"original","name":"Call","event":[{"script":{"exec":["pm.test('kept')"]}}],"request":{"method":"GET","url":"https://example.invalid"},"response":[{"name":"saved","body":"saved example"}],"x-item":"kept"}],"alien":[false,null,1]});
    let input = json!({"version":1,"collection":collection,"ui":{"selectedPath":"0","responsePanelHeight":-5,"environmentPanelHeight":123,"futureUi":true},"environments":[{"id":"env","name":"dev","futureEnv":{"keep":1},"values":[{"key":"token","value":"synthetic-private-value","enabled":false,"type":"secret","description":{"content":"rich description"},"secret":true,"futureValue":true},{"key":"object","value":{"nested":42}}]}],"activeEnvironmentId":"env","futureRoot":[1,2]});
    let state: ProjectState = serde_json::from_value(input).unwrap();
    let output = serde_json::to_value(&state).unwrap();
    assert_eq!(output["collection"], collection);
    assert_eq!(output["futureRoot"], json!([1, 2]));
    assert_eq!(output["ui"]["futureUi"], true);
    assert_eq!(output["ui"]["responsePanelHeight"], -5);
    assert_eq!(output["environments"][0]["futureEnv"], json!({"keep":1}));
    assert_eq!(output["environments"][0]["values"][0]["futureValue"], true);
    assert_eq!(
        output["environments"][0]["values"][1]["value"],
        json!({"nested":42})
    );
    assert!(!format!("{state:?}").contains("synthetic-private-value"));
    assert!(!format!("{:?}", state.environments[0]).contains("synthetic-private-value"));
}
#[test]
fn paths_reject_malformed_input_instead_of_silently_retargeting_another_item() {
    let path = NodePath::decode(Some("0/2/3")).unwrap();
    assert_eq!(path.encode(), "0/2/3");
    assert_eq!(path.parent().encode(), "0/2");
    assert_eq!(path.parent().child(4).encode(), "0/2/4");
    for input in ["0/x/3", "-1", "1//2", "184467440737095516160"] {
        assert!(NodePath::decode(Some(input)).is_err());
    }
    assert!(NodePath::decode(None).unwrap().is_root());
}
#[test]
fn structure_limits_apply_to_mutations_and_invalid_move_indices_do_not_overflow() {
    let mut model = Workspace::default();
    let root = NodePath::default();
    assert!(model.move_down(&NodePath(vec![usize::MAX])).is_none());
    let mut path = root.clone();
    for _ in 0..64 {
        path = model.add_folder(&path, "nested").unwrap();
    }
    assert!(model.add_folder(&path, "too deep").is_none());
    let leaf = model.add_request(&root, "leaf").unwrap();
    let before = model.export_collection();
    assert!(model.move_node(&leaf, &path, None).is_none());
    assert_eq!(*model.export_collection(), *before);
}
#[test]
fn unchanged_text_preserves_structured_environment_values_and_captured_state() {
    let mut model = Workspace::default();
    let id = model.create_environment("structured");
    let values = serde_json::from_value(
        json!([{"key":"object","value":{"nested":42},"enabled":false,"secret":true}]),
    )
    .unwrap();
    model.set_environment_values(&id, values);
    let captured = model.project_state();
    let formatted = environment::format(&model.environment(&id).unwrap().values);
    assert!(!model.update_environment_text(&id, &formatted));
    assert_eq!(
        model.environment(&id).unwrap().values[0].value,
        json!({"nested":42})
    );
    model.update_environment_text(&id, "object=edited text");
    assert_eq!(
        captured.environments[0].values[0].value,
        json!({"nested":42})
    );
    assert!(model.environment(&id).unwrap().values[0].secret);
    assert!(!model.environment(&id).unwrap().values[0].enabled);
}
#[test]
fn checked_in_postman_fixture_keeps_exact_json_data_except_the_intended_edit() {
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/rest/model/preserved.postman.json"
    ))
    .unwrap();
    let mut model = Workspace::default();
    model.replace_collection(original.clone()).unwrap();
    model.rename(&NodePath(vec![0, 0]), "Renamed").unwrap();
    let exported = serde_json::to_string(&*model.export_collection()).unwrap();
    let actual: serde_json::Value = serde_json::from_str(&exported).unwrap();
    let mut expected = original;
    expected["item"][0]["item"][0]["name"] = json!("Renamed");
    assert_eq!(actual, expected);
    let session = model.project_state();
    let decoded: ProjectState =
        serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
    assert_eq!(session, decoded);
    assert_eq!(
        session.collection["info"]["x-info"]["future"][2].to_string(),
        "123456789012345678901234567890"
    );
}
