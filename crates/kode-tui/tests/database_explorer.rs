use crossterm::event::MouseButton;
use kode_core::WorkspaceGeneration;
use kode_database::{
    connections::{Phase, Snapshot},
    model::{DataSourceDefinition, DatabaseObject, DatabaseObjectType, MetadataResult},
};
use kode_tui::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    panels::database::{
        controller::{DatabasePanel, Intent, Operation},
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

    // A package menu must offer real source only when the selected provider
    // explicitly advertises it; it must never manufacture a placeholder DDL.
    let mut package = object();
    package.object_type = DatabaseObjectType::Package;
    let package_request =
        kode_tui::panels::database::tree::DatabaseTree::request_for(&package).unwrap();
    assert_eq!(
        package_request.object_id.as_deref(),
        Some(package.id.as_str())
    );
    let mut menu = ObjectMenu::new("fixture".into(), package.clone());
    assert!(!menu.actions.iter().any(|(label, _)| label.contains("DDL")));
    assert!(menu.actions.iter().any(|(label, _)| label == "Copy name"));
    menu.dispatch(&key(Key::Enter));
    assert_eq!(menu.sql.as_deref(), Some("PUBLIC.items"));
    assert!(!menu.source_requested);

    // Db2's bound package identity contains a version and binary token. Keep
    // the opaque ID intact and route its SQL statements as source, not DDL.
    let mut db2 = package.clone();
    db2.id = json!(["db2-package", "APP_界", "PACK_%", "", "FEDCBA9876543210"]).to_string();
    db2.name = "PACK_%".into();
    db2.qualified_name = "APP_界.PACK_%".into();
    db2.schema = Some("APP_界".into());
    db2.attributes
        .insert("packageProvider".into(), "db2-luw".into());
    db2.attributes
        .insert("sourceAvailable".into(), "true".into());
    db2.attributes.insert("ddlAvailable".into(), "false".into());
    let db2_request = DatabaseTree::request_for(&db2).unwrap();
    assert_eq!(db2_request.object_id.as_deref(), Some(db2.id.as_str()));
    assert_eq!(db2_request.schema.as_deref(), Some("APP_界"));
    assert_eq!(db2_request.object_name.as_deref(), Some("PACK_%"));
    let mut db2_menu = ObjectMenu::new("fixture".into(), db2);
    assert_eq!(db2_menu.actions[0].0, "View package source");
    assert!(
        !db2_menu
            .actions
            .iter()
            .any(|(label, _)| label.contains("DDL"))
    );
    db2_menu.dispatch(&key(Key::Enter));
    assert!(db2_menu.source_requested);
    assert!(db2_menu.sql.is_none());

    package
        .attributes
        .insert("sourceAvailable".into(), "true".into());
    package
        .attributes
        .insert("ddlAvailable".into(), "true".into());
    let mut menu = ObjectMenu::new("fixture".into(), package);
    assert_eq!(menu.actions[0].0, "View package DDL");
    assert_eq!(menu.actions[0].1, "");
    menu.dispatch(&key(Key::Enter));
    assert!(menu.source_requested);
    assert!(menu.sql.is_none());
}

#[test]
fn expandable_package_keeps_enter_for_members_and_opens_source_actions_by_key_or_button() {
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
    let mut package = object();
    package.id = "package:owned".into();
    package.name = "owned_package".into();
    package.object_type = DatabaseObjectType::Package;
    package
        .attributes
        .insert("sourceAvailable".into(), "true".into());
    tree.complete(
        &root,
        Ok(Arc::new(MetadataResult {
            objects: vec![package.clone()],
            capabilities: None,
            message: None,
        })),
        true,
    );
    tree.selected = tree
        .lines
        .iter()
        .position(|line| {
            line.object
                .as_ref()
                .is_some_and(|item| item.id == package.id)
        })
        .unwrap();
    tree.dispatch(&key(Key::Enter));
    let child = load(&mut tree);
    assert_eq!(
        child.request.object_id.as_deref(),
        Some(package.id.as_str())
    );

    tree.dispatch(&key(Key::Character('m')));
    let actions = tree.take_actions();
    assert!(
        matches!(&actions[..], [Action::Object { source, object }] if source == "fixture" && object.id == package.id)
    );

    let area = Rect::new(2, 3, 44, 12);
    let mut buffer = Buffer::empty(area);
    tree.render(area, &mut buffer, &Theme::compiled(), true);
    let (x, y) = (area.y..area.bottom())
        .find_map(|y| {
            let row: String = (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.find("[source]").map(|x| (area.x + x as u16, y))
        })
        .expect("package source button is visible");
    tree.dispatch(&InputEvent::Pointer(PointerInput {
        x,
        y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    }));
    let actions = tree.take_actions();
    assert!(
        matches!(&actions[..], [Action::Object { source, object }] if source == "fixture" && object.id == package.id)
    );
}

#[test]
fn package_source_work_is_cancellable_and_stale_context_never_opens_editor() {
    let source_request = |panel: &mut DatabasePanel| {
        let mut package = object();
        package.object_type = DatabaseObjectType::Package;
        package
            .attributes
            .insert("sourceAvailable".into(), "true".into());
        package
            .attributes
            .insert("ddlAvailable".into(), "true".into());
        panel.menu = Some(ObjectMenu::new("fixture".into(), package));
        let mut focus = kode_tui::focus::FocusState::default();
        panel
            .dispatch(&key(Key::Enter), &mut focus)
            .expect("select package source action");
        let work = panel
            .take_intents()
            .into_iter()
            .find_map(|intent| match intent {
                Intent::Work(work) => Some(work),
                _ => None,
            })
            .expect("source action schedules database work");
        assert!(matches!(work.operation, Operation::Source { .. }));
        (work, focus)
    };

    let mut cancelled_panel = DatabasePanel::new(WorkspaceGeneration(77)).unwrap();
    let (cancelled, mut focus) = source_request(&mut cancelled_panel);
    cancelled_panel
        .dispatch(&key(Key::Escape), &mut focus)
        .unwrap();
    assert!(cancelled.cancel.is_cancelled());
    assert_eq!(
        cancelled_panel.tree.notice,
        "Package source request cancelled"
    );

    let mut stale_panel = DatabasePanel::new(WorkspaceGeneration(78)).unwrap();
    let (stale, mut focus) = source_request(&mut stale_panel);
    stale_panel.complete(
        stale.tag,
        stale.context(),
        Err("late package source response".into()),
        false,
        &mut focus,
    );
    assert!(stale_panel.tree.notice.contains("context changed"));
    assert!(
        stale_panel
            .take_intents()
            .into_iter()
            .all(|intent| !matches!(intent, Intent::Source { .. }))
    );

    let mut error_panel = DatabasePanel::new(WorkspaceGeneration(80)).unwrap();
    let (failed, mut focus) = source_request(&mut error_panel);
    let readable_error = "Package definition is unavailable; check source privileges";
    error_panel.complete(
        failed.tag,
        failed.context(),
        Err(readable_error.into()),
        true,
        &mut focus,
    );
    assert_eq!(error_panel.tree.notice, readable_error);
    error_panel
        .dispatch(&key(Key::Function(4)), &mut focus)
        .unwrap();
    assert!(error_panel.take_intents().iter().any(|intent| matches!(
        intent,
        Intent::Details { text, .. } if text == readable_error
    )));

    let mut superseded_panel = DatabasePanel::new(WorkspaceGeneration(79)).unwrap();
    let (older, mut focus) = source_request(&mut superseded_panel);
    let (newer, _) = source_request(&mut superseded_panel);
    assert_ne!(older.tag, newer.tag);
    assert!(older.cancel.is_cancelled());
    superseded_panel.complete(
        older.tag,
        older.context(),
        Err("late superseded package source response".into()),
        false,
        &mut focus,
    );
    assert_eq!(
        superseded_panel.tree.notice,
        "Loading package source… Esc cancels"
    );
    assert!(
        superseded_panel
            .take_intents()
            .into_iter()
            .all(|intent| !matches!(intent, Intent::Source { .. }))
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
