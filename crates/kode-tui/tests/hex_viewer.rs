use crossterm::event::MouseButton;
use kode_core::document::bytes::ByteIndex;
use kode_tui::{focus::FocusState, input::terminal::*, theme::Theme, viewers::hex::HexView};
use ratatui::{buffer::Buffer, layout::Rect};
fn key(key: Key, shift: bool, ctrl: bool, alt: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            shift,
            ctrl,
            alt,
            meta: false,
        },
        repeat: false,
    })
}
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Default::default(),
    })
}
fn setup() -> (HexView, FocusState, Buffer) {
    let mut view = HexView::new(
        "bytes.bin".into(),
        "application/octet-stream".into(),
        (0..=255).collect(),
    )
    .unwrap();
    let focus = FocusState {
        focused: Some(view.id),
        ..Default::default()
    };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 20));
    view.render(
        Rect::new(3, 2, 90, 8),
        &mut buffer,
        &Theme::compiled(),
        true,
    );
    (view, focus, buffer)
}
#[test]
fn hex_ascii_cells_offsets_selection_and_cursor_share_byte_identity() {
    let (mut view, focus, mut buffer) = setup();
    view.buffer.move_to(ByteIndex(34), false);
    view.buffer.move_to(ByteIndex(32), true);
    view.render(
        Rect::new(3, 2, 90, 8),
        &mut buffer,
        &Theme::compiled(),
        true,
    );
    assert_eq!(buffer[(13, 5)].symbol(), "2");
    assert_eq!(buffer[(14, 5)].symbol(), "0");
    assert_eq!(buffer[(63, 5)].symbol(), " ");
    assert_eq!(buffer[(64, 5)].symbol(), "!");
    assert_eq!(buffer[(13, 5)].style(), buffer[(63, 5)].style());
    assert_eq!(buffer[(16, 5)].style(), buffer[(64, 5)].style());
    assert_eq!(view.hit_test(13, 5), Some(ByteIndex(32)));
    assert_eq!(view.hit_test(14, 5), Some(ByteIndex(32)));
    assert_eq!(view.hit_test(63, 5), Some(ByteIndex(32)));
    assert_eq!(view.hit_test(64, 5), Some(ByteIndex(33)));
    assert_eq!(view.hit_test(3, 5), None);
    assert_eq!(view.hit_test(13, 2), None);
    assert_eq!(focus.focused, Some(view.id));
}
#[test]
fn reverse_drag_release_outside_and_focus_loss_use_absolute_coordinates() {
    let (mut view, mut focus, _) = setup();
    assert!(view.dispatch(
        &pointer(69, 4, PointerAction::Down(MouseButton::Left)),
        &mut focus
    ));
    assert_eq!(view.buffer.cursor(), ByteIndex(22));
    assert!(view.dispatch(
        &pointer(16, 3, PointerAction::Drag(MouseButton::Left)),
        &mut focus
    ));
    assert_eq!(view.buffer.selection(), Some(1..22));
    assert!(view.dispatch(
        &pointer(0, 0, PointerAction::Up(MouseButton::Left)),
        &mut focus
    ));
    assert!(focus.captured.is_none());
    view.dispatch(
        &pointer(13, 3, PointerAction::Down(MouseButton::Left)),
        &mut focus,
    );
    view.dispatch(&InputEvent::Focus(false), &mut focus);
    assert!(!view.is_dragging(&focus));
    assert!(!view.dispatch(&key(Key::Delete, false, false, false), &mut focus));
    assert_eq!(view.buffer.len(), 256);
}
#[test]
fn row_page_movement_scroll_and_exact_local_clipboard_match_active_bindings() {
    let (mut view, mut focus, _) = setup();
    for command in [Key::Right, Key::Down, Key::End] {
        view.dispatch(&key(command, false, false, false), &mut focus);
    }
    assert_eq!(view.buffer.cursor(), ByteIndex(32));
    view.dispatch(&key(Key::Home, true, false, false), &mut focus);
    view.dispatch(&key(Key::Left, true, false, false), &mut focus);
    assert_eq!(view.buffer.selection(), Some(31..32));
    view.dispatch(&key(Key::Character('X'), false, false, true), &mut focus);
    assert_eq!(view.buffer.len(), 255);
    view.dispatch(&key(Key::Character('v'), false, false, true), &mut focus);
    assert_eq!(view.buffer.bytes(), &(0..=255).collect::<Vec<_>>());
    view.dispatch(&key(Key::PageDown, true, false, false), &mut focus);
    assert!(view.buffer.selection().is_none());
    assert_eq!(view.buffer.cursor(), ByteIndex(144));
    view.dispatch(&key(Key::PageDown, false, false, false), &mut focus);
    assert_eq!(view.buffer.cursor(), ByteIndex(256));
    view.dispatch(
        &pointer(
            13,
            3,
            PointerAction::Scroll {
                horizontal: 0,
                vertical: i16::MIN,
            },
        ),
        &mut focus,
    );
    assert_eq!(view.scroll_row, 0);
    assert!(!view.dispatch(&key(Key::Character('f'), false, false, false), &mut focus));
    assert!(!view.dispatch(&key(Key::Character('s'), false, true, false), &mut focus));
    assert!(!view.dispatch(
        &InputEvent::Paste("not binary clipboard".into()),
        &mut focus
    ));
}
#[test]
fn empty_partial_rows_and_tiny_surfaces_clear_only_owned_cells() {
    for bytes in [vec![], vec![0, 0xff, b'A'], (0..17).collect()] {
        for width in [0, 1, 9, 10, 11, 20, 76] {
            for height in [0, 1, 2, 8] {
                let mut view =
                    HexView::new("\u{1b}[0mfile".into(), "binary".into(), bytes.clone()).unwrap();
                let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 20));
                buffer[(0, 0)].set_symbol("X");
                view.render(
                    Rect::new(2, 3, width, height),
                    &mut buffer,
                    &Theme::compiled(),
                    true,
                );
                assert_eq!(buffer[(0, 0)].symbol(), "X");
                assert!(
                    buffer
                        .content()
                        .iter()
                        .all(|cell| !cell.symbol().contains('\u{1b}'))
                );
                let mut focus = FocusState {
                    focused: Some(view.id),
                    ..Default::default()
                };
                view.dispatch(&key(Key::End, false, false, false), &mut focus);
                assert!(view.buffer.cursor().0 <= bytes.len());
            }
        }
    }
}
