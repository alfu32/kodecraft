use kode_core::WorkspaceGeneration;
use kode_database::{
    connections::{Phase, Snapshot},
    model::{DataSourceDefinition, DatabaseObject, DatabaseObjectType, MetadataResult},
};
use kode_tui::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers},
    panels::database::{
        object_menu::ObjectMenu,
        tree::{Action, DatabaseTree, Load},
    },
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::sync::Arc;
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn source() -> DataSourceDefinition {
    serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver"},"jdbcUrl":"jdbc:h2:mem:explorer"})).unwrap()
}
fn object() -> DatabaseObject {
    serde_json::from_value(json!({"id":"object:table","name":"items","qualifiedName":"PUBLIC.items","objectType":"TABLE","schema":"PUBLIC","hasChildren":true,"attributes":{"driverId":"h2"}})).unwrap()
}
fn result() -> Arc<MetadataResult> {
    Arc::new(MetadataResult {
        objects: vec![object()],
        capabilities: None,
        message: None,
    })
}
fn load(tree: &mut DatabaseTree) -> Load {
    tree.take_actions()
        .into_iter()
        .find_map(|action| match action {
            Action::Load(load) => Some(load),
            _ => None,
        })
        .unwrap()
}
fn tree() -> DatabaseTree {
    let mut tree = DatabaseTree::new(WorkspaceGeneration(1)).unwrap();
    tree.replace_sources(vec![source()]);
    tree
}
#[test]
fn connects_then_loads_tagged_metadata_and_rejects_superseded_or_old_workspace_replies() {
    let mut tree = tree();
    assert!(tree.dispatch(&key(Key::Enter)));
    assert!(matches!(&tree.take_actions()[0],Action::Connection(id)if id=="fixture"));
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Connected,
            revision: 1,
            ..Default::default()
        },
    );
    let old = load(&mut tree);
    assert!(tree.lines.iter().any(|line| line.label.contains("Loading")));
    tree.refresh_selected();
    let fresh = load(&mut tree);
    assert!(old.cancel.is_cancelled());
    assert!(!tree.complete(&old, Ok(result()), true));
    assert!(tree.complete(&fresh, Ok(result()), true));
    assert!(tree.lines.iter().any(|line| line.label == "items"));
    tree.selected = tree
        .lines
        .iter()
        .position(|line| line.object.is_some())
        .unwrap();
    tree.activate();
    let child = load(&mut tree);
    assert_eq!(child.request.object_name.as_deref(), Some("items"));
    assert_eq!(child.request.object_type, Some(DatabaseObjectType::Table));
    tree.generation = WorkspaceGeneration(2);
    assert!(!tree.complete(&child, Ok(result()), true));
}
#[test]
fn stale_cache_and_errors_are_visible_and_reconnect_cancels_previous_tree_jobs() {
    let mut tree = tree();
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Connected,
            revision: 1,
            ..Default::default()
        },
    );
    let pending = load(&mut tree);
    tree.complete(&pending, Ok(result()), false);
    assert!(!tree.lines.iter().any(|line| line.label == "items"));
    assert!(
        tree.lines
            .iter()
            .any(|line| line.label.contains("Metadata changed"))
    );
    tree.refresh_selected();
    let pending = load(&mut tree);
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Disconnected,
            revision: 2,
            ..Default::default()
        },
    );
    assert!(pending.cancel.is_cancelled());
    assert!(!tree.complete(&pending, Ok(result()), true));
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Connected,
            revision: 3,
            ..Default::default()
        },
    );
    let fresh = load(&mut tree);
    assert!(tree.complete(&fresh, Err("Driver metadata unavailable".into()), false));
    assert!(
        tree.lines
            .iter()
            .any(|line| line.label == "Driver metadata unavailable")
    );
}
#[test]
fn source_actions_and_object_sql_actions_preserve_source_behavior() {
    let mut tree = tree();
    for (input, expected) in [
        (Key::Character('n'), "new"),
        (Key::Function(2), "edit"),
        (Key::Character('c'), "connection"),
        (Key::Character('s'), "console"),
        (Key::Delete, "delete"),
    ] {
        tree.dispatch(&key(input));
        let actions = tree.take_actions();
        assert_eq!(actions.len(), 1);
        let action = match &actions[0] {
            Action::New => "new",
            Action::Edit(_) => "edit",
            Action::Connection(_) => "connection",
            Action::Console(_) => "console",
            Action::Delete(_) => "delete",
            _ => "other",
        };
        assert_eq!(action, expected);
    }
    let mut menu = ObjectMenu::new("fixture".into(), object());
    assert_eq!(menu.actions[0].1, "SCRIPT NODATA TABLE PUBLIC.items;");
    menu.dispatch(&key(Key::Down));
    menu.dispatch(&key(Key::Enter));
    assert_eq!(menu.sql.as_deref(), Some("SELECT * FROM PUBLIC.items;"));
    assert!(menu.finished);
    let mut function = object();
    function.object_type = DatabaseObjectType::Function;
    assert_eq!(
        ObjectMenu::new("fixture".into(), function).actions[1].1,
        "SELECT PUBLIC.items();"
    );
}
#[test]
fn explorer_and_object_menu_render_safely_at_tiny_sizes_and_unicode_names() {
    let mut tree = tree();
    tree.sources[0].name = "é界😀".into();
    let theme = Theme::compiled();
    for width in 0..90 {
        for height in [0, 1, 2, 3, 4, 8, 24] {
            let area = Rect::new(2, 3, width, height);
            let mut buffer = Buffer::empty(area);
            tree.render(area, &mut buffer, &theme, true);
            let mut menu = ObjectMenu::new("fixture".into(), object());
            menu.render(area, &mut buffer, &theme);
        }
    }
}

#[test]
fn refreshing_a_parent_cancels_descendant_loads_and_never_keeps_obsolete_children() {
    let mut tree = tree();
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Connected,
            revision: 1,
            ..Default::default()
        },
    );
    let root = load(&mut tree);
    tree.complete(&root, Ok(result()), true);
    tree.selected = tree
        .lines
        .iter()
        .position(|line| line.object.is_some())
        .unwrap();
    tree.activate();
    let old = load(&mut tree);
    tree.selected = 0;
    tree.refresh_selected();
    let fresh = load(&mut tree);
    assert!(old.cancel.is_cancelled());
    assert!(!tree.complete(&old, Ok(result()), true));
    tree.complete(
        &fresh,
        Ok(Arc::new(MetadataResult {
            objects: vec![],
            capabilities: None,
            message: Some("No tables".into()),
        })),
        true,
    );
    assert!(!tree.lines.iter().any(|line| line.label == "items"));
    assert!(tree.lines.iter().any(|line| line.label == "No tables"));
    tree.selected = tree.lines.iter().position(|line| line.message).unwrap();
    tree.dispatch(&key(Key::Delete));
    assert!(tree.take_actions().is_empty());
}
#[test]
fn session_folder_and_remove_buttons_have_distinct_mouse_targets_and_do_not_delete_the_datasource()
{
    use crossterm::event::MouseButton;
    use kode_tui::input::terminal::{PointerAction, PointerInput};
    let mut tree = tree();
    tree.set_status(
        "fixture",
        Snapshot {
            phase: Phase::Connected,
            revision: 1,
            ..Default::default()
        },
    );
    let root = load(&mut tree);
    tree.complete(&root, Ok(result()), true);
    tree.set_sessions(vec![kode_tui::panels::database::tree::Session {
        id: "one".into(),
        source: "fixture".into(),
        title: "SQL 1".into(),
    }]);
    let area = Rect::new(0, 0, 40, 15);
    let theme = Theme::compiled();
    let pointer = |x, y| {
        InputEvent::Pointer(PointerInput {
            x,
            y,
            action: PointerAction::Down(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    };
    tree.render(area, &mut Buffer::empty(area), &theme, true);
    tree.dispatch(&pointer(36, 4));
    assert!(matches!(&tree.take_actions()[0],Action::Console(source)if source=="fixture"));
    tree.dispatch(&pointer(5, 4));
    tree.render(area, &mut Buffer::empty(area), &theme, true);
    tree.dispatch(&pointer(36, 5));
    assert!(matches!(&tree.take_actions()[0],Action::RemoveSession(id)if id=="one"));
}
