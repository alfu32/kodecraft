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
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
struct SingleClickClock(AtomicU64);
impl kode_platform::jobs::Clock for SingleClickClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Harness {
    document: Document,
    view: EditorInput,
    focus: FocusState,
    area: Rect,
    search: u16,
    clock: Arc<SingleClickClock>,
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
        let clock = Arc::new(SingleClickClock(AtomicU64::new(0)));
        let view = EditorInput::with_clock(FocusId(1), document.id(), clock.clone());
        Self {
            document,
            view,
            focus: FocusState {
                focused: Some(FocusId(1)),
                ..Default::default()
            },
            area: Rect::new(0, 0, width, height),
            search: 0,
            clock,
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
        self.clock.0.fetch_add(1000, Ordering::SeqCst);
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
            &Theme::compiled(),
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

fn at_byte(h: &mut Harness, byte: usize, action: PointerAction, alt: bool) -> InputEvent {
    let geometry = h.geometry();
    let layout = h
        .view
        .layout(
            &h.document,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let (row, column) = layout.position_to_cell(ByteOffset(byte)).unwrap();
    pointer(
        geometry.text.x + column as u16,
        geometry.text.y + row as u16,
        action,
        Modifiers {
            alt,
            ..Default::default()
        },
    )
}
fn click_byte(h: &mut Harness, byte: usize, alt: bool) {
    let down = at_byte(h, byte, PointerAction::Down(MouseButton::Left), alt);
    h.send(down).unwrap();
    let up = at_byte(h, byte, PointerAction::Up(MouseButton::Left), alt);
    h.send(up).unwrap();
}

#[test]
fn alt_click_toggles_primary_secondary_and_final_cursor_then_edits_and_undoes_together() {
    let mut h = Harness::new("abcd", 30, 8);
    click_byte(&mut h, 1, false);
    click_byte(&mut h, 3, true);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(3));
    assert_eq!(h.view.state.caret.secondary[0].head, ByteOffset(1));
    let before = h.view.state.caret.clone();
    h.send(plain(Key::Character('Ω'))).unwrap();
    assert_eq!(h.document.snapshot().text().as_ref(), "aΩbcΩd");
    assert_eq!(h.document.history_depth(), (1, 0));
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.document.snapshot().text().as_ref(), "abcd");
    assert_eq!(h.view.state.caret, before);
    click_byte(&mut h, 3, true);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(1));
    assert!(h.view.state.caret.secondary.is_empty());
    click_byte(&mut h, 1, true);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(1));
    click_byte(&mut h, 3, true);
    click_byte(&mut h, 1, true);
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(3));
    assert!(h.view.state.caret.secondary.is_empty());
    click_byte(&mut h, 0, true);
    click_byte(&mut h, 2, false);
    assert!(h.view.state.caret.secondary.is_empty());
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(2));
}

#[test]
fn alt_drag_adds_reverse_selection_normalizes_overlap_and_releases_outside_without_loss() {
    let mut h = Harness::new("abcdefgh", 30, 8);
    click_byte(&mut h, 1, false);
    click_byte(&mut h, 3, true);
    for (byte, action) in [
        (6, PointerAction::Down(MouseButton::Left)),
        (2, PointerAction::Drag(MouseButton::Left)),
        (2, PointerAction::Up(MouseButton::Left)),
    ] {
        let event = at_byte(&mut h, byte, action, true);
        h.send(event).unwrap();
    }
    assert_eq!(
        h.view.state.caret.selection,
        Selection {
            anchor: ByteOffset(6),
            head: ByteOffset(2)
        }
    );
    assert_eq!(h.view.state.caret.secondary.len(), 1); // cursor 3 was inside the new range
    assert_eq!(h.view.state.caret.secondary[0].head, ByteOffset(1));
    assert!(h.focus.captured.is_none());
    h.send(InputEvent::Paste("X".into())).unwrap();
    assert_eq!(h.document.snapshot().text().as_ref(), "aXbXgh");
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    let event = at_byte(&mut h, 7, PointerAction::Down(MouseButton::Left), true);
    h.send(event).unwrap();
    let geometry = h.geometry();
    h.send(pointer(
        geometry.text.right() + 5,
        geometry.text.y,
        PointerAction::Up(MouseButton::Left),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(h.focus.captured.is_none());
    assert_eq!(
        h.view.state.caret.selection.range(),
        kode_core::TextRange::new(ByteOffset(7), ByteOffset(8)).unwrap()
    );
    assert_eq!(h.view.state.caret.secondary.len(), 2);
    let event = at_byte(&mut h, 0, PointerAction::Down(MouseButton::Left), true);
    h.send(event).unwrap();
    h.send(InputEvent::Focus(false)).unwrap();
    assert!(!h.view.is_dragging(&h.focus));
    assert!(h.focus.captured.is_none());
    assert!(h.view.state.caret.validate(&h.document.snapshot()).is_ok());
}

#[test]
fn visual_vertical_cursors_follow_wraps_clamp_boundaries_and_escape_keeps_primary() {
    let mut h = Harness::new("abcdefghij0123456789\nxy\nz", 13, 8); // 10 text cells
    click_byte(&mut h, 2, false);
    let add = |key_value| {
        key(
            key_value,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
        )
    };
    h.send(add(Key::Down)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(12));
    h.send(add(Key::Down)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(23));
    h.send(add(Key::Down)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(25));
    let before = h.view.state.caret.clone();
    h.send(add(Key::Down)).unwrap();
    assert_eq!(h.view.state.caret, before);
    assert_eq!(h.view.state.caret.secondary.len(), 3);
    h.send(add(Key::Up)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(22));
    assert_eq!(h.view.state.caret.secondary.len(), 4);
    h.send(plain(Key::Escape)).unwrap();
    assert!(h.view.state.caret.secondary.is_empty());
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(22));
}

#[test]
fn multi_paste_proof_copy_cut_navigation_and_cursor_paint_use_the_entire_set() {
    let mut h = Harness::new("ab cd ef", 30, 8);
    h.view.state.caret.selection = Selection {
        anchor: ByteOffset(8),
        head: ByteOffset(6),
    };
    h.view.state.caret.secondary = vec![Selection {
        anchor: ByteOffset(0),
        head: ByteOffset(2),
    }];
    let outcome = h
        .send(key(
            Key::Character('c'),
            Modifiers {
                ctrl: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(
        outcome
            .effects
            .iter()
            .any(|effect| matches!(effect, EditorEffect::Copy(text) if text == "ab\nef"))
    );
    let ticket = h.paste_ticket();
    h.view.state.caret.secondary.clear(); // primary is unchanged
    assert!(!h.complete(&ticket, "STALE").handled);
    h.view.state.caret.secondary = ticket.secondary.clone();
    let ticket = h.paste_ticket();
    h.complete(&ticket, "Ω");
    assert_eq!(h.document.snapshot().text().as_ref(), "Ω cd Ω");
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    h.send(key(
        Key::Character('x'),
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.document.snapshot().text().as_ref(), " cd ");
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    h.send(plain(Key::Left)).unwrap();
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(5));
    assert_eq!(h.view.state.caret.secondary[0].head, ByteOffset(1));
    let geometry = h.geometry();
    let buffer = h.render();
    for column in [1, 5] {
        assert_eq!(
            buffer[(geometry.text.x + column, geometry.text.y)].bg,
            Theme::compiled().style("code-cursor").bg.unwrap()
        );
    }
}

#[test]
fn visual_additions_skip_collapsed_rows_and_preserve_valid_tab_and_unicode_boundaries() {
    let mut h = Harness::new("{\n\t😀\n}\n\t界e\u{301}\nnext", 20, 8);
    let geometry = h.geometry();
    let folds = [kode_tui::editor::layout::CollapsedFold {
        start: LineIndex(0),
        end: LineIndex(2),
    }];
    let input = key(
        Key::Down,
        Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    );
    h.view
        .dispatch(
            &input,
            &mut h.document,
            &mut h.focus,
            InputViewport {
                geometry,
                folds: &folds,
            },
            &mut (),
        )
        .unwrap();
    assert_eq!(
        h.document
            .snapshot()
            .byte_to_position(h.view.state.caret.cursor())
            .unwrap()
            .line,
        LineIndex(3)
    );
    h.view.state.caret.validate(&h.document.snapshot()).unwrap();
    for _ in 0..8 {
        h.send(input.clone()).unwrap();
    }
    h.view.state.caret.validate(&h.document.snapshot()).unwrap();
    assert!(h.view.state.caret.secondary.len() >= 2);
    h.send(InputEvent::Paste("X".into())).unwrap();
    assert_eq!(h.document.history_depth(), (1, 0));
}

#[test]
fn stale_additive_drag_is_disposed_after_shared_document_change_and_invalid_move_is_atomic() {
    let mut h = Harness::new("abcdefgh\n", 30, 8);
    click_byte(&mut h, 1, false);
    let down = at_byte(&mut h, 6, PointerAction::Down(MouseButton::Left), true);
    h.send(down).unwrap();
    h.document
        .apply_edit(
            h.document.revision(),
            kode_core::TextRange::new(ByteOffset(0), ByteOffset(8)).unwrap(),
            "é",
            kode_core::commands::EditKind::Programmatic,
        )
        .unwrap();
    let geometry = h.geometry();
    h.send(pointer(
        geometry.text.x + 3,
        geometry.text.y,
        PointerAction::Up(MouseButton::Left),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(h.focus.captured.is_none());
    assert!(!h.view.is_dragging(&h.focus));
    h.view.state.caret.validate(&h.document.snapshot()).unwrap();
    let before = h.view.state.caret.clone();
    assert!(
        h.view
            .command(
                EditorCommand::MoveTo {
                    position: ByteOffset(1),
                    extend: false
                },
                &mut h.document,
                InputViewport {
                    geometry,
                    folds: &[]
                }
            )
            .is_err()
    );
    assert_eq!(h.view.state.caret, before);
}

#[test]
fn empty_document_multiple_cursor_gestures_keep_one_caret_and_readonly_guards() {
    let mut h = Harness::new("", 10, 5);
    click_byte(&mut h, 0, true);
    h.send(key(
        Key::Down,
        Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    ))
    .unwrap();
    h.send(key(
        Key::Up,
        Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.view.state.caret.selections().count(), 1);
    h.document.set_read_only(true);
    assert!(matches!(
        h.send(InputEvent::Paste("X".into())),
        Err(DomainError::ReadOnly)
    ));
    assert_eq!(h.document.snapshot().text().as_ref(), "");
    h.document.set_read_only(false);
    h.send(plain(Key::Character('é'))).unwrap();
    assert_eq!(h.document.snapshot().text().as_ref(), "é");
    assert_eq!(h.document.history_depth(), (1, 0));
}
