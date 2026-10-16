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
