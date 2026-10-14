use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use kode_tui::input::terminal::{InputEvent, Key, PointerAction, normalize};

#[test]
fn enter_null_backtab_case_and_modifiers_normalize_once() {
    for code in [KeyCode::Enter, KeyCode::Char('\r'), KeyCode::Char('\n')] {
        assert!(
            matches!(normalize(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))), Some(InputEvent::Key(input)) if input.key == Key::Enter)
        );
    }
    assert!(
        matches!(normalize(Event::Key(KeyEvent::new(KeyCode::Null, KeyModifiers::NONE))), Some(InputEvent::Key(input)) if input.key == Key::Character(' ') && input.modifiers.ctrl)
    );
    assert!(
        matches!(normalize(Event::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE))), Some(InputEvent::Key(input)) if input.key == Key::Tab && input.modifiers.shift)
    );
    assert!(
        matches!(normalize(Event::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::SUPER))), Some(InputEvent::Key(input)) if input.key == Key::Character('A') && input.modifiers.shift && input.modifiers.alt && input.modifiers.meta)
    );
}

#[test]
fn releases_never_edit_and_repeats_are_explicit() {
    let press = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(
        normalize(Event::Key(KeyEvent {
            kind: KeyEventKind::Release,
            ..press
        }))
        .is_none()
    );
    assert!(
        matches!(normalize(Event::Key(KeyEvent { kind: KeyEventKind::Repeat, ..press })), Some(InputEvent::Key(input)) if input.repeat)
    );
    assert!(
        matches!(normalize(Event::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))), Some(InputEvent::Key(input)) if input.key == Key::PageDown)
    );
}

#[test]
fn pointer_paste_focus_and_tiny_resize_keep_their_own_semantics() {
    let mouse = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 2,
        row: 3,
        modifiers: KeyModifiers::CONTROL,
    };
    assert!(
        matches!(normalize(Event::Mouse(mouse)), Some(InputEvent::Pointer(input)) if input.action == PointerAction::Drag(MouseButton::Left) && input.x == 2 && input.y == 3 && input.modifiers.ctrl)
    );
    let text = "a\r\n😀\t";
    assert_eq!(
        normalize(Event::Paste(text.into())),
        Some(InputEvent::Paste(text.into()))
    );
    assert_eq!(
        normalize(Event::Resize(1, 1)),
        Some(InputEvent::Resize {
            width: 1,
            height: 1
        })
    );
    assert_eq!(
        normalize(Event::Resize(0, 0)),
        Some(InputEvent::Resize {
            width: 0,
            height: 0
        })
    );
    assert_eq!(normalize(Event::FocusLost), Some(InputEvent::Focus(false)));
}
