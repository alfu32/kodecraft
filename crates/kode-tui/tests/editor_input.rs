use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, LineIndex, Result, Selection,
    commands::{EditorCommand, Movement},
    document::Document,
};
use kode_tui::{
    editor::{
        input::{
            EditorEffect, EditorInput, EditorInputHooks, EditorState, HookContext, InputOutcome,
            InputTarget, InputViewport, PasteTicket,
        },
        render::{EditorGeometry, EditorHeader, RenderOptions, render_editor},
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};

struct Harness {
    document: Document,
    view: EditorInput,
    focus: FocusState,
    area: Rect,
    search: u16,
}
impl Harness {
    fn new(text: &str, width: u16, height: u16) -> Self {
        let document = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "test".into(),
                key: "input".into(),
                label: "buffer".into(),
            },
            None,
            text,
        );
        let view = EditorInput::new(FocusId(1), document.id());
        Self {
            document,
            view,
            focus: FocusState {
                focused: Some(FocusId(1)),
                ..Default::default()
            },
            area: Rect::new(0, 0, width, height),
            search: 0,
        }
    }
    fn geometry(&self) -> EditorGeometry {
        EditorGeometry::new(self.area, self.document.snapshot().len_lines(), self.search)
    }
    fn send(&mut self, event: InputEvent) -> Result<InputOutcome> {
        self.send_with(event, &mut ())
    }
    fn send_with(
        &mut self,
        event: InputEvent,
        hooks: &mut dyn EditorInputHooks,
    ) -> Result<InputOutcome> {
        let geometry = self.geometry();
        self.view.dispatch(
            &event,
            &mut self.document,
            &mut self.focus,
            InputViewport {
                geometry,
                folds: &[],
            },
            hooks,
        )
    }
    fn render(&mut self) -> Buffer {
        let geometry = self.geometry();
        let layout = self
            .view
            .layout(
                &self.document,
                InputViewport {
                    geometry,
                    folds: &[],
                },
            )
            .unwrap();
        let mut buffer = Buffer::empty(self.area);
        render_editor(
            &layout,
            &mut buffer,
            &Theme::default(),
            RenderOptions {
                geometry,
                header: &EditorHeader::for_document(&self.document, None, String::new()),
                state: &self.view.render_state(&self.focus),
                overlays: &[],
                warning_lines: &[],
                fold_starts: &[],
            },
        );
        buffer
    }
    fn paste_ticket(&mut self) -> PasteTicket {
        self.send(key(
            Key::Character('v'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        ))
        .unwrap()
        .effects
        .into_iter()
        .find_map(|effect| match effect {
            EditorEffect::ReadClipboard(ticket) => Some(ticket),
            _ => None,
        })
        .unwrap()
    }
    fn complete(&mut self, ticket: &PasteTicket, text: &str) -> InputOutcome {
        let geometry = self.geometry();
        self.view
            .complete_paste(
                ticket,
                text,
                &mut self.document,
                &self.focus,
                InputViewport {
                    geometry,
                    folds: &[],
                },
            )
            .unwrap()
    }
}
fn key(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers,
        repeat: false,
    })
}
fn plain(key_value: Key) -> InputEvent {
    key(key_value, Modifiers::default())
}
fn pointer(x: u16, y: u16, action: PointerAction, modifiers: Modifiers) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers,
    })
}
fn mouse(x: u16, y: u16, action: PointerAction) -> InputEvent {
    pointer(x, y, action, Modifiers::default())
}
fn lines() -> String {
    (0..100)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn source_page_regressions_move_caret_selection_and_rendered_viewport() {
    let mut h = Harness::new(&"\t".repeat(100), 23, 4);
    h.send(key(
        Key::PageDown,
        Modifiers {
            shift: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        h.view.state.caret.selection,
        Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(15)
        }
    );
    assert_eq!(h.view.state.scroll_rows, 3);
    assert_eq!(
        h.view
            .state
            .caret
            .selected_text(&h.document.snapshot())
            .unwrap(),
        "\t".repeat(15)
    );
    h.send(plain(Key::PageUp)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(0));
    assert!(h.view.state.caret.selection.is_empty());
    assert_eq!(h.view.state.scroll_rows, 0);

    let mut h = Harness::new(&lines(), 35, 11);
    h.send(plain(Key::PageDown)).unwrap();
    assert_eq!(
        h.document
            .snapshot()
            .byte_to_position(h.view.state.caret.cursor())
            .unwrap()
            .line,
        LineIndex(10)
    );
    assert_eq!(h.view.state.scroll_rows, 10);
    let geometry = h.geometry();
    let buffer = h.render();
    assert_eq!(buffer[(geometry.text.x, geometry.text.y)].symbol(), "l");
    let row: String = (geometry.text.x..geometry.text.x + 7)
        .map(|x| buffer[(x, geometry.text.y)].symbol())
        .collect();
    assert_eq!(row, "line 10");
    h.send(plain(Key::PageUp)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(0));
    assert_eq!(h.view.state.scroll_rows, 0);
}

#[test]
fn wheel_preserves_caret_and_navigation_reveals_it_even_without_movement() {
    let mut h = Harness::new(&lines(), 35, 11);
    let wheel = PointerAction::Scroll {
        horizontal: 0,
        vertical: 1,
    };
    h.send(mouse(8, 2, wheel)).unwrap();
    assert_eq!(h.view.state.scroll_rows, 3);
    h.send(pointer(
        8,
        2,
        wheel,
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.view.state.scroll_rows, 12);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(0));
    h.send(plain(Key::Home)).unwrap();
    assert_eq!(h.view.state.scroll_rows, 0);
    h.send(mouse(
        8,
        2,
        PointerAction::Scroll {
            horizontal: 0,
            vertical: i16::MAX,
        },
    ))
    .unwrap();
    assert_eq!(h.view.state.scroll_rows, 90);
    h.send(mouse(
        8,
        2,
        PointerAction::Scroll {
            horizontal: 0,
            vertical: i16::MIN,
        },
    ))
    .unwrap();
    assert_eq!(h.view.state.scroll_rows, 0);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(0));
}

#[test]
fn reverse_drag_across_wrapped_escapes_keeps_anchor_and_release_outside_clears_capture() {
    let mut h = Harness::new("a\t😀Z0123456789", 12, 6);
    h.area = Rect::new(20, 10, 12, 6);
    h.focus.focused = Some(FocusId(2));
    let geometry = h.geometry();
    h.send(mouse(
        geometry.text.x + 1,
        geometry.text.y + 1,
        PointerAction::Down(MouseButton::Left),
    ))
    .unwrap();
    assert_eq!(h.focus.focused, Some(h.view.id));
    assert_eq!(h.focus.captured, Some(h.view.id));
    let anchor = h.view.state.caret.cursor();
    assert_eq!(anchor, ByteOffset(6)); // interior emoji escape hits the scalar end
    h.send(mouse(
        geometry.text.x + 2,
        geometry.text.y,
        PointerAction::Drag(MouseButton::Left),
    ))
    .unwrap();
    assert_eq!(
        h.view.state.caret.selection,
        Selection {
            anchor,
            head: ByteOffset(2)
        }
    );
    assert_eq!(
        h.view
            .state
            .caret
            .selected_text(&h.document.snapshot())
            .unwrap(),
        "😀"
    );
    h.send(mouse(0, 0, PointerAction::Up(MouseButton::Left)))
        .unwrap();
    assert!(!h.view.is_dragging(&h.focus));
    assert_eq!(h.focus.captured, None);
    let selection = h.view.state.caret.selection;
    h.send(mouse(
        geometry.text.x,
        geometry.text.y + 2,
        PointerAction::Move,
    ))
    .unwrap();
    assert_eq!(h.view.state.caret.selection, selection);
    h.send(plain(Key::Character('X'))).unwrap();
    assert_eq!(
        h.document.snapshot().to_service_snapshot().text.as_ref(),
        "a\tXZ0123456789"
    );
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.view.state.caret.selection, selection);
}

#[test]
fn drag_autoscroll_ticks_clamp_to_source_and_focus_loss_releases_owner() {
    let mut h = Harness::new(&lines(), 35, 5);
    let geometry = h.geometry();
    h.send(mouse(
        geometry.text.x,
        geometry.text.y,
        PointerAction::Down(MouseButton::Left),
    ))
    .unwrap();
    h.send(mouse(
        geometry.text.x + 100,
        geometry.text.bottom() + 10,
        PointerAction::Drag(MouseButton::Left),
    ))
    .unwrap();
    assert_eq!(h.view.state.scroll_rows, 1);
    assert_eq!(
        h.view.state.caret.cursor(),
        h.document.snapshot().line_end(LineIndex(4)).unwrap()
    );
    h.view
        .tick_drag(
            &h.document,
            &h.focus,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    assert_eq!(h.view.state.scroll_rows, 2);
    h.send(InputEvent::Focus(false)).unwrap();
    assert_eq!(h.focus.captured, None);
    assert!(!h.view.render_state(&h.focus).focused);
    assert!(!h.send(plain(Key::Character('x'))).unwrap().handled);
    h.send(InputEvent::Focus(true)).unwrap();
    assert!(h.view.render_state(&h.focus).focused);
}

#[derive(Default)]
struct Hooks {
    consume: Option<InputTarget>,
    seen: Vec<(InputTarget, InputEvent, Rect)>,
}
impl EditorInputHooks for Hooks {
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        _: &mut Document,
        _: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        self.seen
            .push((target, event.clone(), context.geometry.text));
        Ok(self.consume == Some(target))
    }
}

#[test]
fn popup_search_navigation_and_nested_offsets_preserve_priority_and_modifiers() {
    let mut h = Harness::new("A.*B", 35, 8);
    h.area = Rect::new(14, 7, 35, 8);
    let mut hooks = Hooks {
        consume: Some(InputTarget::Popup),
        ..Default::default()
    };
    h.view.state.scroll_rows = 0;
    assert!(
        h.send_with(plain(Key::Character('X')), &mut hooks)
            .unwrap()
            .handled
    );
    assert_eq!(h.document.revision().0, 0);
    assert_eq!(hooks.seen.len(), 1);
    h.view.state.caret.select_all(&h.document.snapshot());
    hooks.consume = None;
    let outcome = h
        .send_with(
            key(
                Key::Character('f'),
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            ),
            &mut hooks,
        )
        .unwrap();
    assert!(matches!(&outcome.effects[0], EditorEffect::OpenSearch(text) if text == "A.*B"));
    assert!(h.view.state.search_has_focus);
    h.search = 2;
    let geometry = h.geometry();
    assert_eq!(geometry.text.y, h.area.y + 3);
    hooks.consume = Some(InputTarget::Search);
    let modifiers = Modifiers {
        ctrl: true,
        alt: true,
        shift: true,
        meta: true,
    };
    let event = pointer(
        geometry.search.x + 1,
        geometry.search.y,
        PointerAction::Down(MouseButton::Left),
        modifiers,
    );
    h.send_with(event.clone(), &mut hooks).unwrap();
    assert_eq!(
        hooks.seen.last(),
        Some(&(InputTarget::Search, event, geometry.text))
    );
    assert!(!h.view.render_state(&h.focus).focused);
    hooks.consume = Some(InputTarget::Navigation);
    let event = pointer(
        geometry.text.x + 1,
        geometry.text.y,
        PointerAction::Down(MouseButton::Left),
        modifiers,
    );
    h.send_with(event.clone(), &mut hooks).unwrap();
    assert_eq!(
        hooks.seen.last(),
        Some(&(InputTarget::Navigation, event, geometry.text))
    );
    assert_eq!(h.focus.captured, None);
    hooks.consume = None;
    h.send_with(
        mouse(
            geometry.text.x,
            geometry.text.y,
            PointerAction::Down(MouseButton::Left),
        ),
        &mut hooks,
    )
    .unwrap();
    assert!(!h.view.state.search_has_focus);
    assert!(h.view.render_state(&h.focus).focused);
}

#[test]
fn delayed_clipboard_paste_uses_request_revision_selection_and_focus_identity() {
    let mut h = Harness::new("abc", 23, 4);
    let old = h.paste_ticket();
    let current = h.paste_ticket();
    assert!(!h.complete(&old, "STALE").handled);
    assert!(h.complete(&current, "\t😀\n").handled);
    assert_eq!(
        h.document.snapshot().to_service_snapshot().text.as_ref(),
        "\t😀\nabc"
    );
    assert_eq!(h.document.revision().0, 1);
    assert!(!h.complete(&current, "DUPLICATE").handled);
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        h.document.snapshot().to_service_snapshot().text.as_ref(),
        "abc"
    );
    let ticket = h.paste_ticket();
    h.send(plain(Key::Right)).unwrap();
    assert!(!h.complete(&ticket, "MOVED").handled);
    let ticket = h.paste_ticket();
    h.focus.focused = Some(FocusId(2));
    assert!(!h.complete(&ticket, "WRONG FOCUS").handled);
    h.focus.focused = Some(h.view.id);
    h.send(InputEvent::Focus(false)).unwrap();
    h.send(InputEvent::Focus(true)).unwrap();
    assert!(!h.complete(&ticket, "OLD FOCUS").handled);
}

#[test]
fn raw_copy_cut_paste_history_readonly_and_unfocused_keys_share_core_guards() {
    let mut h = Harness::new("\t😀\x01", 23, 4);
    h.send(key(
        Key::Character('a'),
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    ))
    .unwrap();
    let outcome = h
        .send(key(
            Key::Character('C'),
            Modifiers {
                alt: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(matches!(&outcome.effects[0], EditorEffect::Copy(text) if text == "\t😀\x01"));
    h.document.set_read_only(true);
    for event in [
        plain(Key::Character('x')),
        plain(Key::Backspace),
        plain(Key::Delete),
        InputEvent::Paste("paste".into()),
        key(
            Key::Character('x'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        ),
        key(
            Key::Character('v'),
            Modifiers {
                alt: true,
                ..Default::default()
            },
        ),
        key(
            Key::Character('u'),
            Modifiers {
                alt: true,
                ..Default::default()
            },
        ),
    ] {
        assert!(matches!(h.send(event), Err(DomainError::ReadOnly)));
    }
    assert_eq!(h.document.revision().0, 0);
    h.document.set_read_only(false);
    h.send(InputEvent::Paste("RAW\n".into())).unwrap();
    assert_eq!(h.document.revision().0, 1);
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        h.document.snapshot().to_service_snapshot().text.as_ref(),
        "\t😀\x01"
    );
    h.focus.focused = Some(FocusId(2));
    assert!(!h.send(plain(Key::Character('X'))).unwrap().handled);
    assert!(!h.send(InputEvent::Paste("X".into())).unwrap().handled);
    h.focus.captured = Some(FocusId(2));
    assert!(
        !h.send(mouse(4, 1, PointerAction::Down(MouseButton::Left)))
            .unwrap()
            .handled
    );
}

#[test]
fn header_gutter_tiny_resize_and_logical_arrows_keep_coordinate_bounds() {
    for (width, height) in [(0, 0), (1, 1), (2, 2), (23, 4), (80, 24)] {
        let mut h = Harness::new("abcdefghi\nshort", width, height);
        h.send(plain(Key::PageDown)).unwrap();
        h.send(plain(Key::PageUp)).unwrap();
        h.send(InputEvent::Resize { width, height }).unwrap();
        h.render();
        let before = h.view.state.caret.selection;
        assert!(
            !h.send(mouse(0, 0, PointerAction::Down(MouseButton::Left)))
                .unwrap()
                .handled
        );
        assert_eq!(h.view.state.caret.selection, before);
    }
    let mut h = Harness::new("a".repeat(100).as_str(), 23, 4);
    h.send(plain(Key::Down)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(100)); // logical Down, not wrapped row
    h.send(plain(Key::Up)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(0));
    h.send(mouse(0, 3, PointerAction::Down(MouseButton::Left)))
        .unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(40));
    let geometry = h.geometry();
    let other = Document::new(
        DocumentId::fresh().unwrap(),
        h.document.origin().clone(),
        None,
        "different",
    );
    assert!(
        h.view
            .layout(
                &other,
                InputViewport {
                    geometry,
                    folds: &[]
                }
            )
            .is_err()
    );
    h.view
        .command(
            EditorCommand::Move {
                movement: Movement::DocumentEnd,
                extend: false,
            },
            &mut h.document,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(100));
}
