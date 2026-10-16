use crossterm::event::MouseButton;
use kode_core::RequestId;
use kode_rest::{
    import::{Import, Kind},
    model::{NodePath, Workspace},
};
use kode_tui::{
    focus::FocusState,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    panels::rest::sidebar::{Action, Intent, Sidebar},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn ctrl(ch: char) -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character(ch),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Modifiers::default(),
    })
}
fn setup() -> (Workspace, Sidebar, FocusState) {
    let model = Workspace::default();
    let sidebar = Sidebar::new(&model).unwrap();
    let focus = FocusState {
        focused: Some(sidebar.id),
        ..Default::default()
    };
    (model, sidebar, focus)
}
fn draw(sidebar: &mut Sidebar, model: &Workspace, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    sidebar.render(area, &mut buffer, &Theme::compiled(), model, true);
    buffer
}
fn submit(sidebar: &mut Sidebar, model: &mut Workspace, focus: &mut FocusState, text: &str) {
    sidebar.dispatch(&ctrl('a'), model, focus).unwrap();
    sidebar
        .dispatch(&InputEvent::Paste(text.into()), model, focus)
        .unwrap();
    sidebar.dispatch(&key(Key::Enter), model, focus).unwrap();
}
#[test]
fn tree_shortcuts_and_row_buttons_target_the_visible_node_and_emit_stable_open_and_run_intents() {
    let (mut model, mut sidebar, mut focus) = setup();
    draw(&mut sidebar, &model, 36, 24);
    sidebar
        .dispatch(&key(Key::Character('f')), &mut model, &mut focus)
        .unwrap();
    let folder = sidebar.cursor.clone();
    assert!(matches!(sidebar.take_intents().pop(),Some(Intent::Open(path))if path==folder));
    draw(&mut sidebar, &model, 36, 24);
    sidebar
        .dispatch(&key(Key::Character('n')), &mut model, &mut focus)
        .unwrap();
    let child = sidebar.cursor.clone();
    assert_eq!(child, folder.child(0));
    sidebar
        .dispatch(&key(Key::Character('r')), &mut model, &mut focus)
        .unwrap();
    assert!(
        sidebar
            .take_intents()
            .iter()
            .any(|intent| matches!(intent,Intent::Run(path)if path==&child))
    );
    draw(&mut sidebar, &model, 36, 24);
    let root_y = sidebar.geometry.tree.y;
    sidebar
        .dispatch(
            &pointer(34, root_y, PointerAction::Down(MouseButton::Left)),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert_eq!(model.collection()["item"].as_array().unwrap().len(), 2);
    assert_eq!(
        model.node(&folder).unwrap()["item"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn filtering_includes_ancestors_urls_and_method_and_owns_typing_clipboard_and_escape() {
    let (mut model, mut sidebar, mut focus) = setup();
    let folder = model.add_folder(&NodePath::default(), "Folder").unwrap();
    let path = model.add_request(&folder, "Call").unwrap();
    model
        .set_node_field(
            &path,
            "request",
            Some(json!({"method":"PATCH","url":{"raw":"https://find.example.test"}})),
        )
        .unwrap();
    draw(&mut sidebar, &model, 36, 24);
    assert_eq!(sidebar.visible.len(), 2);
    sidebar
        .dispatch(&key(Key::Character('/')), &mut model, &mut focus)
        .unwrap();
    sidebar
        .dispatch(
            &InputEvent::Paste("find.example".into()),
            &mut model,
            &mut focus,
        )
        .unwrap();
    draw(&mut sidebar, &model, 36, 24);
    assert_eq!(sidebar.visible.len(), 3);
    assert!(sidebar.visible.iter().any(|node| node.path == path));
    sidebar
        .dispatch(&ctrl('a'), &mut model, &mut focus)
        .unwrap();
    sidebar
        .dispatch(&ctrl('v'), &mut model, &mut focus)
        .unwrap();
    let proof = sidebar
        .take_intents()
        .into_iter()
        .find_map(|intent| {
            if let Intent::Clipboard { proof, .. } = intent {
                proof
            } else {
                None
            }
        })
        .unwrap();
    sidebar.paste(proof, "PATCH".into(), &focus);
    draw(&mut sidebar, &model, 36, 24);
    assert_eq!(sidebar.visible.len(), 3);
    sidebar
        .dispatch(&key(Key::Escape), &mut model, &mut focus)
        .unwrap();
    assert!(!sidebar.filtering);
    assert!(sidebar.filter.value.is_empty());
    assert_eq!(model.node(&path).unwrap()["name"], "Call");
}
#[test]
fn modal_rename_copy_move_delete_preserve_model_identity_and_expansion_after_rebasing() {
    let (mut model, mut sidebar, mut focus) = setup();
    let root = NodePath::default();
    let folder = model.add_folder(&root, "Source").unwrap();
    let child = model.add_request(&folder, "Call").unwrap();
    let destination = model.add_folder(&root, "Destination").unwrap();
    let id = model.node(&child).unwrap()["id"].clone();
    sidebar
        .perform(Action::Toggle(folder.clone()), &mut model, &mut focus)
        .unwrap();
    sidebar
        .perform(Action::Rename(folder.clone()), &mut model, &mut focus)
        .unwrap();
    submit(&mut sidebar, &mut model, &mut focus, "Renamed");
    assert_eq!(model.node(&folder).unwrap()["name"], "Renamed");
    sidebar
        .perform(
            Action::MoveInto {
                from: folder,
                to: destination,
            },
            &mut model,
            &mut focus,
        )
        .unwrap();
    let moved = NodePath(vec![0, 0]);
    draw(&mut sidebar, &model, 36, 24);
    assert!(
        sidebar
            .visible
            .iter()
            .any(|node| node.path == moved.child(0))
    );
    assert_eq!(model.node(&moved.child(0)).unwrap()["id"], id);
    sidebar
        .perform(Action::Copy(moved.clone()), &mut model, &mut focus)
        .unwrap();
    let copy = sidebar.cursor.clone();
    assert_ne!(model.node(&copy.child(0)).unwrap()["id"], id);
    sidebar
        .perform(Action::Delete(copy.clone()), &mut model, &mut focus)
        .unwrap();
    assert!(sidebar.has_modal());
    sidebar
        .dispatch(&key(Key::Tab), &mut model, &mut focus)
        .unwrap();
    sidebar
        .dispatch(&key(Key::Enter), &mut model, &mut focus)
        .unwrap();
    assert!(model.node(&copy).is_some());
    sidebar
        .perform(Action::Delete(copy.clone()), &mut model, &mut focus)
        .unwrap();
    sidebar
        .dispatch(&key(Key::Enter), &mut model, &mut focus)
        .unwrap();
    assert!(model.node(&copy).is_none());
    assert_eq!(model.node(&moved.child(0)).unwrap()["id"], id);
}
#[test]
fn stale_modal_and_clipboard_results_do_not_modify_a_replaced_collection_or_another_field() {
    let (mut model, mut sidebar, mut focus) = setup();
    let request = model.add_request(&NodePath::default(), "Original").unwrap();
    sidebar
        .perform(Action::Rename(request), &mut model, &mut focus)
        .unwrap();
    model
        .replace_collection(
            json!({"info":{"name":"Replacement"},"item":[{"name":"Keep","request":{}}]}),
        )
        .unwrap();
    submit(&mut sidebar, &mut model, &mut focus, "Wrong");
    assert_eq!(model.collection()["item"][0]["name"], "Keep");
    assert!(sidebar.message.contains("changed"));
    sidebar
        .dispatch(&key(Key::Character('/')), &mut model, &mut focus)
        .unwrap();
    sidebar
        .dispatch(&ctrl('v'), &mut model, &mut focus)
        .unwrap();
    let proof = sidebar
        .take_intents()
        .into_iter()
        .find_map(|intent| {
            if let Intent::Clipboard { proof, .. } = intent {
                proof
            } else {
                None
            }
        })
        .unwrap();
    sidebar
        .perform(Action::Export, &mut model, &mut focus)
        .unwrap();
    sidebar.paste(proof, "wrong-file".into(), &focus);
    assert!(sidebar.filter.value.is_empty());
    sidebar
        .dispatch(&key(Key::Escape), &mut model, &mut focus)
        .unwrap();
}
#[test]
fn environment_actions_keep_metadata_activate_scroll_and_never_fall_through_to_request_deletion() {
    let (mut model, mut sidebar, mut focus) = setup();
    let request = model
        .add_request(&NodePath::default(), "Retained request")
        .unwrap();
    sidebar
        .perform(Action::NewEnvironment, &mut model, &mut focus)
        .unwrap();
    submit(&mut sidebar, &mut model, &mut focus, "Development");
    let environment = sidebar.environment.clone().unwrap();
    model.set_environment_values(
        &environment,
        vec![kode_rest::model::EnvironmentValue {
            key: "token".into(),
            value: json!("synthetic"),
            secret: true,
            extra: serde_json::Map::from_iter([("x-note".into(), json!(7))]),
            ..Default::default()
        }],
    );
    sidebar
        .perform(
            Action::CopyEnvironment(environment.clone()),
            &mut model,
            &mut focus,
        )
        .unwrap();
    submit(&mut sidebar, &mut model, &mut focus, "Copy");
    let copy = sidebar.environment.clone().unwrap();
    assert_ne!(copy, environment);
    assert_eq!(
        model.environment(&copy).unwrap().values,
        model.environment(&environment).unwrap().values
    );
    sidebar
        .perform(
            Action::ActivateEnvironment(copy.clone()),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert_eq!(model.active_environment().unwrap().id, copy);
    sidebar
        .perform(
            Action::OpenEnvironment(copy.clone()),
            &mut model,
            &mut focus,
        )
        .unwrap();
    sidebar
        .dispatch(&key(Key::Character('f')), &mut model, &mut focus)
        .unwrap();
    assert_eq!(model.collection()["item"].as_array().unwrap().len(), 1);
    sidebar
        .dispatch(&key(Key::Delete), &mut model, &mut focus)
        .unwrap();
    assert!(model.environment(&copy).is_none());
    assert!(model.node(&request).is_some());
    assert!(
        sidebar
            .take_intents()
            .iter()
            .any(|intent| matches!(intent,Intent::EnvironmentRemoved(id)if id==&copy))
    );
}
fn pending_import(
    sidebar: &mut Sidebar,
    model: &mut Workspace,
    focus: &mut FocusState,
) -> (RequestId, ArcValue, NodePath) {
    sidebar
        .perform(Action::ImportOpenApi(NodePath::default()), model, focus)
        .unwrap();
    submit(sidebar, model, focus, "selected.yaml");
    sidebar
        .take_intents()
        .into_iter()
        .find_map(|intent| {
            if let Intent::Import {
                nonce,
                expected,
                parent,
                kind,
                path,
            } = intent
            {
                assert_eq!(kind, Kind::OpenApi);
                assert_eq!(path.to_str(), Some("selected.yaml"));
                Some((nonce, expected, parent))
            } else {
                None
            }
        })
        .unwrap()
}
type ArcValue = std::sync::Arc<serde_json::Value>;
#[test]
fn async_imports_apply_atomically_to_the_captured_destination_and_reject_late_replacements() {
    let (mut model, mut sidebar, mut focus) = setup();
    let (nonce, expected, parent) = pending_import(&mut sidebar, &mut model, &mut focus);
    model
        .rename(&NodePath::default(), "Edited while reading")
        .unwrap();
    let document = json!({"openapi":"3.0.0","paths":{"/health":{"get":{}}}});
    assert!(
        !sidebar
            .import_finished(
                nonce,
                &expected,
                Import::from_document(document.clone()).unwrap(),
                &parent,
                &mut model
            )
            .unwrap()
    );
    assert_eq!(model.collection_name(), "Edited while reading");
    let (nonce, expected, parent) = pending_import(&mut sidebar, &mut model, &mut focus);
    assert!(
        sidebar
            .import_finished(
                nonce,
                &expected,
                Import::from_document(document).unwrap(),
                &parent,
                &mut model
            )
            .unwrap()
    );
    assert!(!model.collection()["item"].as_array().unwrap().is_empty());
    sidebar.file_finished(nonce, "late incorrect status".into());
    assert!(!sidebar.message.contains("late incorrect"));
}
#[test]
fn wheel_scroll_is_independent_of_keyboard_follow_and_dragged_divider_persists() {
    let (mut model, mut sidebar, mut focus) = setup();
    for index in 0..60 {
        model.add_request(&NodePath::default(), &format!("Request {index}"));
    }
    let buffer = draw(&mut sidebar, &model, 36, 24);
    let tree = sidebar.geometry.tree;
    let old: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    sidebar
        .dispatch(
            &pointer(
                tree.x + 1,
                tree.y + 1,
                PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 4,
                },
            ),
            &mut model,
            &mut focus,
        )
        .unwrap();
    let buffer = draw(&mut sidebar, &model, 36, 24);
    let new: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert_ne!(old, new);
    assert_eq!(sidebar.cursor, NodePath::default());
    sidebar
        .dispatch(&key(Key::End), &mut model, &mut focus)
        .unwrap();
    let buffer = draw(&mut sidebar, &model, 36, 24);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("Request 59"));
    let divider = sidebar.geometry.divider;
    sidebar
        .dispatch(
            &pointer(divider.x, divider.y, PointerAction::Down(MouseButton::Left)),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert_eq!(focus.captured, Some(sidebar.id));
    sidebar
        .dispatch(
            &pointer(
                divider.x,
                divider.y - 2,
                PointerAction::Drag(MouseButton::Left),
            ),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert_eq!(model.ui().environment_panel_height, 10);
    sidebar
        .dispatch(
            &pointer(
                divider.x,
                divider.y - 2,
                PointerAction::Up(MouseButton::Left),
            ),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert_eq!(focus.captured, None);
}
#[test]
fn context_menus_are_mouse_accessible_and_tiny_views_stay_bounded() {
    let (mut model, mut sidebar, mut focus) = setup();
    model.add_request(&NodePath::default(), "Call");
    draw(&mut sidebar, &model, 36, 24);
    let row = sidebar.geometry.tree.y + 1;
    sidebar
        .dispatch(
            &pointer(32, row, PointerAction::Down(MouseButton::Left)),
            &mut model,
            &mut focus,
        )
        .unwrap();
    assert!(sidebar.has_modal());
    for (width, height) in [(80, 24), (120, 40), (20, 6), (1, 1), (0, 0)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        sidebar.render_modal(area, &mut buffer, &Theme::compiled());
        sidebar
            .dispatch(&key(Key::End), &mut model, &mut focus)
            .unwrap();
    }
    sidebar.dismiss(&mut focus);
    for (width, height) in [(20, 6), (1, 1), (0, 0)] {
        draw(&mut sidebar, &model, width, height);
        sidebar
            .dispatch(&key(Key::Tab), &mut model, &mut focus)
            .unwrap();
        draw(&mut sidebar, &model, width, height);
    }
}
