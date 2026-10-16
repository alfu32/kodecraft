use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_database::{
    catalog::{DriverRegistry, UrlCatalog},
    model::DataSourceDefinition,
};
use kode_tui::input::terminal::{PointerAction, PointerInput};
use kode_tui::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers},
    panels::database::connection_dialog::{Action, ConnectionDialog},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::sync::Arc;

fn paint(form: &mut ConnectionDialog, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    form.render(area, &mut buffer, &Theme::compiled());
    buffer
}
fn rendered(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}
fn click(form: &mut ConnectionDialog, label: &str, width: u16, height: u16) {
    let buffer = paint(form, width, height);
    for y in 0..height {
        let line: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
        if let Some(x) = line.find(label) {
            assert!(form.dispatch(&InputEvent::Pointer(PointerInput {
                x: x as u16,
                y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            })));
            return;
        }
    }
    panic!("missing button/choice: {label}");
}

#[test]
fn url_button_current_type_keyboard_mouse_and_cancel_preserve_manual_values() {
    let registry = DriverRegistry::compiled().unwrap();
    let urls = UrlCatalog::compiled(&registry).unwrap();
    let mut form = form(Some(existing()));
    for id in ["postgresql", "mysql", "oracle", "sqlserver", "h2"] {
        let index = registry
            .definitions()
            .iter()
            .position(|driver| driver.id == id)
            .unwrap();
        form.fields[1].value = "jdbc:manual:keep".into();
        form.fields[1].cursor = form.fields[1].value.len();
        form.select_driver(index, false);
        assert_eq!(form.fields[1].value, "jdbc:manual:keep");
        form.focus = 2;
        for (width, height) in [(100, 34), (60, 18)] {
            click(&mut form, "[Templates]", width, height);
            assert!(
                form.take_actions()
                    .iter()
                    .any(|action| matches!(action, Action::Suggestions(driver) if driver == id))
            );
            let popup = rendered(&paint(&mut form, width, height));
            assert!(popup.contains("Standard"));
            assert!(popup.contains(&urls.templates(id)[0].label));
            form.dispatch(&key(Key::Escape));
            assert_eq!(form.fields[1].value, "jdbc:manual:keep");
            assert_eq!(form.focus, 2);
        }
        form.dispatch(&key(Key::Down));
        form.dispatch(&key(Key::Home));
        form.dispatch(&key(Key::Down));
        form.dispatch(&key(Key::Enter));
        assert_eq!(form.fields[1].value, urls.templates(id)[1].template);
        assert_eq!(form.focus, 2);
        let name = form.fields[0].value.clone();
        form.dispatch(&key(Key::End));
        form.dispatch(&InputEvent::Paste("-edited".into()));
        assert!(form.fields[1].value.ends_with("-edited"));
        assert_eq!(form.fields[0].value, name);
    }
}

#[test]
fn late_recents_keep_selected_template_and_ignore_other_types_and_credentials() {
    let registry = DriverRegistry::compiled().unwrap();
    let urls = UrlCatalog::compiled(&registry).unwrap();
    let expected = urls.templates("h2").last().unwrap().template.clone();
    let mut form = form(Some(existing()));
    form.focus = 2;
    form.dispatch(&key(Key::Down));
    form.dispatch(&key(Key::End));
    form.set_recents("postgresql", vec!["jdbc:postgresql:other-type".into()]);
    form.set_recents(
        "h2",
        vec![
            "jdbc:h2:mem:recent".into(),
            "jdbc:h2:mem:x;password=secret".into(),
        ],
    );
    form.dispatch(&key(Key::Enter));
    assert_eq!(form.fields[1].value, expected);
    form.dispatch(&key(Key::Down));
    form.dispatch(&key(Key::Home));
    let popup = rendered(&paint(&mut form, 100, 34));
    assert!(popup.contains("Recents: jdbc:h2:mem:recent"));
    assert!(!popup.contains("other-type") && !popup.contains("password=secret"));
    click(&mut form, "Recents: jdbc:h2:mem:recent", 100, 34);
    assert_eq!(form.fields[1].value, "jdbc:h2:mem:recent");
    assert!(
        form.take_actions()
            .iter()
            .all(|action| !matches!(action, Action::Save(_)))
    );
}
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn form(existing: Option<DataSourceDefinition>) -> ConnectionDialog {
    let registry = Arc::new(DriverRegistry::compiled().unwrap());
    let urls = Arc::new(UrlCatalog::compiled(&registry).unwrap());
    ConnectionDialog::new(registry, urls, existing).unwrap()
}

#[test]
fn editing_an_unlisted_driver_preserves_its_identity_provider_and_unknown_configuration() {
    let mut original = existing();
    original.driver.id = "unlisted-vendor".into();
    original.driver.provider_id = Some("synthetic-original-provider".into());
    let form = form(Some(original.clone()));
    let draft = form.draft().unwrap();
    assert_eq!(draft.definition, original);
}

#[test]
fn custom_jar_allows_service_loader_discovery_without_inventing_a_driver_class() {
    let registry = DriverRegistry::compiled().unwrap();
    let mut form = form(None);
    let custom = registry
        .definitions()
        .iter()
        .position(|d| d.id == "custom-jdbc")
        .unwrap();
    form.select_driver(custom, false);
    form.fields[10].value = "/synthetic/driver.jar".into();
    let draft = form.draft().unwrap();
    assert!(draft.definition.driver.driver_class.is_empty());
    assert!(draft.definition.driver.jar_path.is_some());
}
fn existing() -> DataSourceDefinition {
    serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224","artifact":{"repositoryType":"MAVEN_CENTRAL","groupId":"com.h2database","artifactId":"h2","version":"2.2.224"},"futureDriver":3},"jdbcUrl":"jdbc:h2:mem:dialog","credentialReference":"fixture.saved","properties":{"applicationName":"fixture"},"credentialProperties":{"token":"fixture.token"},"futureDatasource":{"enabled":true}})).unwrap()
}
#[test]
fn editing_preserves_private_references_unknown_fields_and_empty_password_without_reading_secrets()
{
    let mut form = form(Some(existing()));
    let original = existing();
    let draft = form.draft().unwrap();
    assert!(draft.password.is_none());
    assert_eq!(draft.definition, original);
    form.focus = 4;
    form.dispatch(&InputEvent::Paste("synthetic-private-password".into()));
    let draft = form.draft().unwrap();
    assert_eq!(
        draft.password.unwrap().expose(),
        "synthetic-private-password"
    );
    assert_eq!(
        draft.definition.credential_reference.as_deref(),
        Some("fixture.password")
    );
    assert!(
        !serde_json::to_string(&draft.definition)
            .unwrap()
            .contains("synthetic-private-password")
    );
    assert_eq!(draft.definition.extra["futureDatasource"]["enabled"], true);
    assert_eq!(draft.definition.driver.extra["futureDriver"], 3);
}
#[test]
fn validation_keeps_entered_fields_and_caret_and_does_not_emit_an_invalid_save() {
    let mut form = form(Some(existing()));
    form.focus = 2;
    form.dispatch(&key(Key::Home));
    form.dispatch(&InputEvent::Paste("invalid:".into()));
    let position = form.fields[1].cursor;
    let value = form.fields[1].value.clone();
    form.dispatch(&InputEvent::Key(KeyInput {
        key: Key::Character('s'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    }));
    assert!(form.status.contains("JDBC URL"));
    assert!(
        form.take_actions()
            .iter()
            .all(|action| !matches!(action, Action::Save(_)))
    );
    assert_eq!(form.focus, 2);
    assert_eq!(form.fields[1].cursor, position);
    assert_eq!(form.fields[1].value, value);
    form.fields[1].value = "jdbc:h2:mem:dialog;password=forbidden".into();
    form.fields[1].cursor = form.fields[1].value.len();
    assert!(form.draft().is_err());
}
#[test]
fn driver_selection_all_entries_version_override_and_local_custom_sources_remain_available() {
    let registry = DriverRegistry::compiled().unwrap();
    let mut form = form(None);
    for (index, driver) in registry.definitions().iter().enumerate() {
        form.select_driver(index, false);
        assert_eq!(form.selected_id(), driver.id);
    }
    let h2 = registry
        .definitions()
        .iter()
        .position(|d| d.id == "h2")
        .unwrap();
    form.select_driver(h2, false);
    form.fields[6].value = "2.3.232".into();
    assert_eq!(
        form.draft()
            .unwrap()
            .definition
            .driver
            .artifact
            .unwrap()
            .version
            .as_deref(),
        Some("2.3.232")
    );
    let custom = registry
        .definitions()
        .iter()
        .position(|d| d.id == "custom-jdbc")
        .unwrap();
    form.select_driver(custom, false);
    form.fields[7].value = "fixture.Driver".into();
    form.fields[10].value = "/synthetic/driver.jar:/synthetic/dependency.jar".into();
    let draft = form.draft().unwrap();
    assert!(
        draft
            .definition
            .driver
            .provider_id
            .unwrap()
            .starts_with("custom-")
    );
    assert_eq!(
        draft.definition.driver.jar_path.as_deref(),
        Some("/synthetic/driver.jar:/synthetic/dependency.jar")
    );
}
#[test]
fn tagged_test_completion_and_cancel_preserve_form_values_and_focus() {
    let mut form = form(Some(existing()));
    form.focus = 3;
    let tag = JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId(10),
        document: None,
    };
    form.busy = Some(tag);
    let stale = JobTag {
        request: RequestId(11),
        ..tag
    };
    assert!(!form.complete(stale, "late failure".into()));
    assert!(form.busy.is_some());
    assert!(form.complete(tag, "Authentication failed".into()));
    assert_eq!(form.fields[0].value, "Fixture");
    assert_eq!(form.focus, 3);
    assert_eq!(form.status, "Authentication failed");
    form.dispatch(&key(Key::Escape));
    assert!(matches!(form.take_actions().last(), Some(Action::Dismiss)));
}
#[test]
fn masked_fields_and_every_focus_render_without_overlap_or_panics_at_tiny_sizes() {
    let mut form = form(Some(existing()));
    form.focus = 4;
    form.dispatch(&InputEvent::Paste("synthetic-private-password".into()));
    let theme = Theme::compiled();
    for width in [0, 1, 2, 3, 8, 20, 39, 40, 60, 90] {
        for height in [0, 1, 2, 3, 4, 6, 8, 24, 40] {
            for focus in 0..20 {
                form.focus = focus;
                let area = Rect::new(2, 3, width, height);
                let mut buffer = Buffer::empty(area);
                form.render(area, &mut buffer, &theme);
                let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
                assert!(!rendered.contains("synthetic-private-password"));
            }
        }
    }
}
