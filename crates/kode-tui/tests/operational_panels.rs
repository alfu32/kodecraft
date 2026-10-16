use crossterm::event::MouseButton;
use kode_tui::{
    focus::{FocusId, FocusState},
    input::terminal::*,
    panels::{
        project::ProjectWork,
        settings::{Settings, native::NativeSettings},
    },
    theme::Theme,
    widgets::text_panel::TextPanel,
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Default::default(),
        repeat: false,
    })
}
fn click(x: u16, y: u16) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Default::default(),
    })
}

#[test]
fn wrapped_informational_panels_scroll_resize_and_keep_source_line_hits() {
    let mut panel =
        TextPanel::new(vec!["abcdefghij".into(), "second".into(), "last".into()]).unwrap();
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 2));
    panel.render(buffer.area, &mut buffer, &Theme::compiled());
    assert_eq!(panel.line_at(Position::new(0, 1)), Some(0));
    panel.dispatch(&key(Key::End), &mut focus);
    panel.render(buffer.area, &mut buffer, &Theme::compiled());
    assert_eq!(panel.line_at(Position::new(0, 1)), Some(2));
    assert_eq!(panel.line_at(Position::new(5, 1)), None);
    panel.set_lines(vec!["new".into()]);
    panel.render(buffer.area, &mut buffer, &Theme::compiled());
    assert_eq!(panel.scroll, 0);
    for area in [
        Rect::new(0, 0, 0, 0),
        Rect::new(0, 0, 1, 1),
        Rect::new(0, 0, 10, 3),
    ] {
        let mut buffer = Buffer::empty(area);
        panel.render(area, &mut buffer, &Theme::compiled());
        panel.dispatch(&key(Key::PageDown), &mut focus);
        panel.render(area, &mut buffer, &Theme::compiled());
    }
}

#[test]
fn native_settings_wrapped_buttons_and_scoped_keys_use_real_worker_commands() {
    let mut settings = NativeSettings::new().unwrap();
    let mut focus = FocusState::default();
    let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 4));
    settings.render(
        buffer.area,
        &mut buffer,
        &Theme::compiled(),
        "Ready",
        std::path::Path::new("cache"),
    );
    assert!(!settings.dispatch(&key(Key::Character('c')), &mut focus));
    settings.dispatch(&click(2, 2), &mut focus);
    assert_eq!(settings.take_work(), vec![ProjectWork::RebuildIndex]);
    settings.dispatch(&key(Key::Character('c')), &mut focus);
    assert_eq!(settings.take_work(), vec![ProjectWork::ClearIndex]);
    settings.dispatch(&key(Key::End), &mut focus);
    settings.render(
        buffer.area,
        &mut buffer,
        &Theme::compiled(),
        "Ready",
        std::path::Path::new("cache"),
    );
    settings.dispatch(&click(1, 1), &mut focus);
    assert!(
        settings.take_work().is_empty(),
        "informational lines must not trigger a hidden button"
    );
}

#[test]
fn settings_navigation_transfers_focus_between_lsp_and_native_without_retargeting_keys() {
    let mut settings = Settings::new().unwrap();
    let lsp = FocusId(999);
    let mut focus = FocusState {
        focused: Some(lsp),
        ..Default::default()
    };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 28, 4));
    settings.render_header(buffer.area, &mut buffer, &Theme::compiled());
    assert!(settings.dispatch_navigation(&key(Key::Tab), &mut focus, lsp));
    assert!(settings.index_selected);
    assert_eq!(focus.focused, Some(settings.native.id()));
    assert!(settings.dispatch_navigation(&click(1, 0), &mut focus, lsp));
    assert!(!settings.index_selected);
    assert_eq!(focus.focused, Some(lsp));
}

#[test]
fn help_comes_from_scoped_chords_and_preserves_letter_case_and_modifier_conflicts() {
    use kode_tui::input::shortcuts::{self, Scope};
    let ctrl_enter = KeyInput {
        key: Key::Enter,
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    };
    for scope in [Scope::Search, Scope::Sql, Scope::Request, Scope::Git] {
        assert!(shortcuts::command_key(scope, ctrl_enter).is_some());
    }
    assert!(shortcuts::command_key(Scope::Image, ctrl_enter).is_none());
    let plain = |ch| KeyInput {
        key: Key::Character(ch),
        modifiers: Default::default(),
        repeat: false,
    };
    assert_eq!(
        shortcuts::command_key(Scope::Git, plain('A')).unwrap().key,
        Key::Character('A')
    );
    assert_eq!(
        shortcuts::command_key(Scope::Git, plain('T')).unwrap().key,
        Key::Character('T')
    );
    assert!(shortcuts::command_key(Scope::Rest, plain('R')).is_none());
    let alt_f = KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            alt: true,
            ..Default::default()
        },
        repeat: false,
    };
    let ctrl_f = KeyInput {
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        ..alt_f
    };
    assert!(shortcuts::command_key(Scope::Global, alt_f).is_some());
    assert!(shortcuts::command_key(Scope::Editor, alt_f).is_none());
    assert!(shortcuts::command_key(Scope::Global, ctrl_f).is_none());
    assert_eq!(
        kode_tui::input::bindings::editor_command(ctrl_f),
        Some(kode_core::commands::EditorCommand::OpenSearch)
    );
    let help = shortcuts::help_lines().join("\n");
    for binding in shortcuts::BINDINGS {
        assert!(help.contains(binding.description));
    }
    for scope in [
        "Shared editor:",
        "Find/replace bar:",
        "Git diff:",
        "SQL results:",
        "REST environments:",
        "Image viewer:",
        "Hex viewer:",
    ] {
        assert!(help.contains(scope), "{scope}");
    }
    assert!(help.contains("classic encodings"));
    assert!(
        !help.contains("Ctrl+Z"),
        "do not advertise an unimplemented undo alias"
    );
}

#[test]
fn about_uses_loaded_languages_and_keeps_independent_lists_usable_when_tiny() {
    use kode_intelligence::{facade::IntelligenceResources, frontend::legacy::DefinitionRegistry};
    use kode_tui::panels::about::About;
    let mut resources = IntelligenceResources::compiled().unwrap();
    resources.definitions = std::sync::Arc::new(
        DefinitionRegistry::from_json(
            r#"{"languages":[{"language":"fixture-language","patterns":[]}]}"#,
        )
        .unwrap(),
    );
    let mut about = About::new().unwrap();
    about.load_languages(&resources);
    let theme = Theme::compiled();
    let area = Rect::new(0, 0, 32, 24);
    let mut buffer = Buffer::empty(area);
    let mut focus = FocusState {
        focused: Some(about.focus_id()),
        ..Default::default()
    };
    about.render(area, &mut buffer, &theme);
    assert!(
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains(&format!("Kode {} | MIT", env!("CARGO_PKG_VERSION")))
    );
    about.dispatch(&key(Key::End), &mut focus);
    let credits_scroll = about.credits.scroll;
    assert!(credits_scroll > 0);
    about.dispatch(&key(Key::Tab), &mut focus);
    about.dispatch(&key(Key::Home), &mut focus);
    about.render(area, &mut buffer, &theme);
    // The grammar catalog is unioned with the real external definition catalog.
    let mut found = false;
    for _ in 0..100 {
        about.render(area, &mut buffer, &theme);
        for y in area.y..area.bottom() {
            let row = (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            if row.contains("fixture-language") {
                about.dispatch(&click(4, y), &mut focus);
                found = true;
                break;
            }
        }
        if found {
            break;
        }
        about.dispatch(&key(Key::PageDown), &mut focus);
    }
    assert_eq!(about.selected_language.as_deref(), Some("fixture-language"));
    assert_eq!(about.credits.scroll, credits_scroll);
    for area in [
        Rect::new(0, 0, 16, 5),
        Rect::new(0, 0, 1, 1),
        Rect::new(0, 0, 0, 0),
    ] {
        let mut buffer = Buffer::empty(area);
        about.render(area, &mut buffer, &theme);
        about.dispatch(&key(Key::Tab), &mut focus);
        about.render(area, &mut buffer, &theme);
        about.dispatch(&key(Key::End), &mut focus);
        about.render(area, &mut buffer, &theme);
    }
}
