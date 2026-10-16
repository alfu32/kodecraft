use kode_tui::{
    focus::FocusState,
    input::terminal::*,
    theme::Theme,
    widgets::{
        diagnostic::{Action, Diagnostic},
        text_panel::wrap_line,
    },
};
use ratatui::{buffer::Buffer, layout::Rect};
fn key(key: Key, ctrl: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl,
            ..Default::default()
        },
        repeat: false,
    })
}
fn draw(dialog: &mut Diagnostic, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    dialog.render(area, &mut buffer, &Theme::compiled());
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}
#[test]
fn wrapped_multiline_unicode_diagnostics_reach_the_tail_copy_full_text_and_clip_tiny_sizes() {
    let text = format!(
        "Operation / request 42\nFirst line\n{}\nTAIL SQLSTATE 42000 vendor 73\nCaused by: TLS explanation",
        "wide 界 and escaped \x1b diagnostic ".repeat(300)
    );
    for width in [60, 100] {
        let mut dialog =
            Diagnostic::new("Database test failure".into(), text.clone(), None).unwrap();
        let mut focus = FocusState {
            focused: Some(dialog.panel.id),
            ..Default::default()
        };
        let first = draw(&mut dialog, width, 20);
        assert!(first.contains("Operation / request 42"));
        assert!(!first.contains('\x1b'));
        dialog.dispatch(&key(Key::End, false), &mut focus);
        assert!(draw(&mut dialog, width, 20).contains("TAIL SQLSTATE 42000 vendor 73"));
        assert!(matches!(
            dialog.dispatch(&key(Key::Character('c'), true), &mut focus),
            Some(Action::Copy)
        ));
        assert!(dialog.text.ends_with(&text));
        assert!(dialog.text.starts_with("Database test failure"));
        dialog.dispatch(&key(Key::Home, false), &mut focus);
        assert_eq!(dialog.panel.scroll, 0);
        dialog.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: 3,
                y: 4,
                action: PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 3,
                },
                modifiers: Modifiers::default(),
            }),
            &mut focus,
        );
        assert!(dialog.panel.scroll > 0);
        for (width, height) in [(1, 1), (3, 4), (0, 0)] {
            draw(&mut dialog, width, height);
        }
        assert!(matches!(
            dialog.dispatch(&key(Key::Escape, false), &mut focus),
            Some(Action::Dismiss)
        ));
    }
    let narrow = wrap_line("a界b", 1).concat();
    assert!(narrow.contains("\\x754C"));
    assert!(narrow.ends_with('b'));
}
