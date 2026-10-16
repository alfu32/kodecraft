use crossterm::event::MouseButton;
use kode_core::{
    DomainError,
    document::bytes::{ByteBuffer, ByteIndex},
};
use kode_platform::jobs::CancellationToken;
use kode_tui::{
    focus::FocusState,
    input::terminal::*,
    theme::Theme,
    viewers::hex::{
        HexView, HexWork,
        search::{self, Direction, SearchMode},
    },
};
use ratatui::{buffer::Buffer, layout::Rect};

fn click_label(view: &mut HexView, focus: &mut FocusState, label: &str) {
    let buffer = paint(view, focus, 100, 20);
    for y in 0..20 {
        let line: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
        if let Some(x) = line.find(label) {
            assert!(view.dispatch(
                &InputEvent::Pointer(PointerInput {
                    x: x as u16,
                    y,
                    action: PointerAction::Down(MouseButton::Left),
                    modifiers: Modifiers::default()
                }),
                focus
            ));
            return;
        }
    }
    panic!("missing hex control: {label}");
}

fn key(key: Key, ctrl: bool, shift: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl,
            shift,
            ..Default::default()
        },
        repeat: false,
    })
}
fn paint(view: &mut HexView, focus: &FocusState, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    view.render(area, &mut buffer, &Theme::compiled(), focus);
    buffer
}
fn open(bytes: Vec<u8>) -> (HexView, FocusState) {
    let mut view = HexView::new("bytes.bin".into(), "binary".into(), bytes).unwrap();
    let focus = FocusState {
        focused: Some(view.id),
        ..Default::default()
    };
    paint(&mut view, &focus, 100, 20);
    (view, focus)
}
fn query(view: &mut HexView, focus: &mut FocusState, value: &str) {
    view.dispatch(&key(Key::Character('f'), true, false), focus);
    view.dispatch(&key(Key::Character('a'), true, false), focus);
    view.dispatch(&InputEvent::Paste(value.into()), focus);
}
fn complete(view: &mut HexView) {
    let work = view.take_work();
    assert_eq!(work.len(), 1);
    let HexWork::Search(job) = work.into_iter().next().unwrap() else {
        panic!("expected actual search work");
    };
    let request = job.request;
    let result = job.run(&CancellationToken::new());
    assert!(view.complete_search(request, result));
}

#[test]
fn binary_pattern_format_utf8_and_validation_are_explicit() {
    assert_eq!(
        SearchMode::Text.pattern("é界\0").unwrap(),
        "é界\0".as_bytes()
    );
    for input in ["00 ff C3 A9", "00ffC3A9", "00\tff\nC3 A9"] {
        assert_eq!(SearchMode::Hex.pattern(input).unwrap(), [0, 255, 195, 169]);
    }
    assert_eq!(SearchMode::Hex.pattern("0 fF"), Err("Odd hex digit count"));
    for input in ["0xFF", "GG", "??", "éF"] {
        assert_eq!(SearchMode::Hex.pattern(input), Err("Use hex bytes: 00 FF"));
    }
    for input in ["", " \t"] {
        assert_eq!(SearchMode::Hex.pattern(input), Err("Enter a pattern"));
    }
    assert_eq!(
        SearchMode::Text.pattern(&"x".repeat(4097)),
        Err("Pattern too long")
    );
}

#[test]
fn all_small_binary_inputs_match_independent_windows_oracle_in_both_directions() {
    for size in 0..=8 {
        for encoded in 0..(1 << size) {
            let bytes: Vec<_> = (0..size).map(|bit| ((encoded >> bit) & 1) as u8).collect();
            for length in 1..=4 {
                for pattern_bits in 0..(1 << length) {
                    let pattern: Vec<_> = (0..length)
                        .map(|bit| ((pattern_bits >> bit) & 1) as u8)
                        .collect();
                    let matches: Vec<_> = bytes
                        .windows(length)
                        .enumerate()
                        .filter_map(|(index, window)| (window == pattern).then_some(index))
                        .collect();
                    for from in 0..=size + 1 {
                        let split = from.min(size);
                        let next = matches.iter().copied().find(|index| *index >= split);
                        let previous = matches.iter().copied().rev().find(|index| *index < split);
                        for (direction, preferred, fallback) in [
                            (Direction::Next, next, matches.first().copied()),
                            (Direction::Previous, previous, matches.last().copied()),
                        ] {
                            let report =
                                search::find(&bytes, &pattern, from, direction, || false).unwrap();
                            assert_eq!(
                                report.range,
                                preferred.or(fallback).map(|start| start..start + length)
                            );
                            assert_eq!(report.wrapped, preferred.is_none() && fallback.is_some());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn repetitive_large_search_cancels_during_scan_and_job_obeys_both_tokens() {
    let bytes = vec![b'a'; 2 * 1024 * 1024];
    let mut pattern = vec![b'a'; 4096];
    pattern[4095] = b'b';
    let mut polls = 0;
    assert!(matches!(
        search::find(&bytes, &pattern, 0, Direction::Next, || {
            polls += 1;
            polls > 3
        }),
        Err(DomainError::Cancelled)
    ));
    assert_eq!(polls, 4);
    assert!(
        search::find(&bytes, &pattern, 0, Direction::Next, || false)
            .unwrap()
            .range
            .is_none()
    );
    let source = ByteBuffer::new(bytes);
    for cancel_scheduler in [false, true] {
        let cancel = CancellationToken::new();
        let scheduler = CancellationToken::new();
        if cancel_scheduler {
            scheduler.cancel();
        } else {
            cancel.cancel();
        }
        let job = search::SearchJob {
            request: kode_core::RequestId::fresh().unwrap(),
            source: source.clone(),
            pattern: pattern.clone(),
            from: 0,
            direction: Direction::Next,
            cancel,
        };
        assert!(matches!(job.run(&scheduler), Err(DomainError::Cancelled)));
    }
}

#[test]
fn layouts_retain_byte_anchor_selection_edits_and_reveal_narrow_columns() {
    let (mut view, mut focus) = open((0..=255).collect());
    view.buffer.start_selection(ByteIndex(12));
    view.buffer.select_to(ByteIndex(20));
    view.dispatch(&key(Key::Delete, false, false), &mut focus);
    assert!(view.modified);
    view.buffer.start_selection(ByteIndex(112));
    view.buffer.select_to(ByteIndex(115));
    let selection = view.buffer.selection();
    let bytes = view.buffer.bytes().to_vec();
    view.scroll_row = 7;
    assert!(view.set_bytes_per_row(32));
    assert_eq!(view.scroll_row, 3);
    assert!(view.set_bytes_per_row(8));
    assert_eq!(view.scroll_row, 14);
    assert!(view.set_bytes_per_row(16));
    assert_eq!(view.scroll_row, 7);
    assert!(!view.set_bytes_per_row(0));
    assert_eq!(view.buffer.selection(), selection);
    assert_eq!(view.buffer.bytes(), bytes);
    assert!(view.modified);
    view.set_bytes_per_row(32);
    view.buffer.move_to(ByteIndex(31), false);
    view.dispatch(&key(Key::Left, false, false), &mut focus);
    let painted = paint(&mut view, &focus, 23, 10);
    assert!((10..23).any(|x| view.hit_test(x, 1) == Some(ByteIndex(30))));
    assert_eq!(painted[(0, 1)].symbol(), "0");
    let top = view.scroll_row;
    paint(&mut view, &focus, 150, 10);
    assert_eq!(view.scroll_row, top);
}

#[test]
fn text_and_hex_find_select_overlap_cross_rows_and_eof_without_mutating_bytes() {
    let mut bytes = vec![0; 15];
    bytes.extend_from_slice("é界".as_bytes());
    bytes.extend_from_slice(b"aaa");
    let original = bytes.clone();
    let (mut view, mut focus) = open(bytes);
    query(&mut view, &mut focus, "é界");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(15..20));
    assert_eq!(focus.focused, Some(view.find.id));
    view.dispatch(&key(Key::Enter, false, true), &mut focus);
    complete(&mut view);
    assert!(view.find.status.ends_with("[wrap]"));
    query(&mut view, &mut focus, "aa");
    view.buffer.move_to(ByteIndex(0), false);
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(20..22));
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(21..23));
    view.dispatch(&key(Key::Enter, false, true), &mut focus);
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(20..22));
    query(&mut view, &mut focus, "00 00");
    view.dispatch(&key(Key::Tab, false, false), &mut focus);
    view.buffer.move_to(ByteIndex(0), false);
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(0..2));
    let selection = view.buffer.selection();
    query(&mut view, &mut focus, "ff");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    assert_eq!(view.find.status, "No match");
    assert_eq!(view.buffer.selection(), selection);
    query(&mut view, &mut focus, "F");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    assert_eq!(view.find.status, "Odd hex digit count");
    assert!(!view.searching());
    assert!(view.take_work().is_empty());
    assert_eq!(view.buffer.bytes(), original);
    assert!(!view.modified);
}

#[test]
fn changed_query_bytes_caret_stop_close_detach_and_drop_reject_old_results() {
    for change in 0..6 {
        let (mut view, mut focus) = open(b"hello hello".to_vec());
        query(&mut view, &mut focus, "hello");
        view.dispatch(&key(Key::Enter, false, false), &mut focus);
        let HexWork::Search(job) = view.take_work().pop().unwrap() else {
            panic!("search");
        };
        let request = job.request;
        let result = search::find(job.source.bytes(), &job.pattern, 0, Direction::Next, || {
            false
        });
        match change {
            0 => {
                view.dispatch(&InputEvent::Paste("x".into()), &mut focus);
            }
            1 => {
                view.buffer.delete_forward();
            }
            2 => view.buffer.move_to(ByteIndex(3), false),
            3 => {
                view.dispatch(&key(Key::Escape, false, false), &mut focus);
            }
            4 => view.detach(&mut focus),
            _ => {
                drop(view);
                assert!(job.cancel.is_cancelled());
                continue;
            }
        }
        let selection = view.buffer.selection();
        assert!(!view.complete_search(request, result));
        assert_eq!(view.buffer.selection(), selection);
        assert!(!view.searching());
    }
}

#[test]
fn old_completion_cannot_discard_the_newer_pending_search() {
    let (mut view, mut focus) = open(b"hello bye".to_vec());
    query(&mut view, &mut focus, "hello");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    let HexWork::Search(old) = view.take_work().pop().unwrap() else {
        panic!("search");
    };
    let old_request = old.request;
    let result = old.run(&CancellationToken::new());
    query(&mut view, &mut focus, "bye");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    assert!(!view.complete_search(old_request, result));
    assert!(view.searching());
    complete(&mut view);
    assert_eq!(view.buffer.selection(), Some(6..9));
}

#[test]
fn repeat_from_body_keeps_focus_and_escape_cancels_the_worker() {
    let (mut view, mut focus) = open(b"hello hello".to_vec());
    query(&mut view, &mut focus, "hello");
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    complete(&mut view);
    let selection = view.buffer.selection();
    view.dispatch(&key(Key::Escape, false, false), &mut focus);
    view.dispatch(&key(Key::Function(3), false, false), &mut focus);
    assert!(view.find.visible && view.searching());
    assert_eq!(focus.focused, Some(view.id));
    let HexWork::Search(job) = view.take_work().pop().unwrap() else {
        panic!("search");
    };
    view.dispatch(&key(Key::Escape, false, false), &mut focus);
    assert!(!view.find.visible && !view.searching());
    assert!(job.cancel.is_cancelled());
    assert_eq!(view.buffer.selection(), selection);
}

#[test]
fn native_clipboard_paste_is_scoped_to_visible_field_mode_value_and_focus() {
    let (mut view, mut focus) = open(vec![0; 32]);
    query(&mut view, &mut focus, "first");
    view.dispatch(&key(Key::Character('v'), true, false), &mut focus);
    let HexWork::Paste(proof) = view.take_work().pop().unwrap() else {
        panic!("paste");
    };
    view.dispatch(&key(Key::Tab, false, false), &mut focus);
    assert!(!view.complete_clipboard(&proof, Ok("late".into()), &focus));
    view.dispatch(&key(Key::Character('v'), true, false), &mut focus);
    let HexWork::Paste(proof) = view.take_work().pop().unwrap() else {
        panic!("paste");
    };
    assert!(view.complete_clipboard(&proof, Ok("00 FF".into()), &focus));
    assert_eq!(view.find.field.value, "first00 FF");
    assert_eq!(view.buffer.bytes(), [0; 32]);
    view.dispatch(&key(Key::Character('v'), true, false), &mut focus);
    let HexWork::Paste(proof) = view.take_work().pop().unwrap() else {
        panic!("paste");
    };
    view.dispatch(&key(Key::Escape, false, false), &mut focus);
    view.dispatch(&key(Key::Character('f'), true, false), &mut focus);
    assert!(!view.complete_clipboard(&proof, Ok("stale reopen".into()), &focus));
}

#[test]
fn header_mouse_controls_stop_and_tiny_find_rendering_preserve_selection_and_bytes() {
    let (mut view, mut focus) = open((0..=255).collect());
    view.buffer.start_selection(ByteIndex(15));
    view.buffer.select_to(ByteIndex(18));
    let selection = view.buffer.selection();
    for width in [8, 32, 16] {
        click_label(&mut view, &mut focus, &format!("[{width}]"));
        assert_eq!(view.bytes_per_row(), width);
        let buffer = paint(&mut view, &focus, 100, 20);
        let second_row: String = (0..8).map(|x| buffer[(x, 2)].symbol()).collect();
        assert_eq!(second_row, format!("{width:08X}"));
        assert_eq!(view.buffer.selection(), selection);
    }
    click_label(&mut view, &mut focus, "[Find]");
    assert!(view.find.visible);
    assert_eq!(focus.focused, Some(view.find.id));
    view.dispatch(&InputEvent::Paste("not found".into()), &mut focus);
    view.dispatch(&key(Key::Enter, false, false), &mut focus);
    let HexWork::Search(job) = view.take_work().pop().unwrap() else {
        panic!("search");
    };
    click_label(&mut view, &mut focus, "[Stop]");
    assert!(job.cancel.is_cancelled());
    assert!(!view.searching());
    assert_eq!(view.find.status, "Search stopped");
    for width in [0, 1, 6, 12, 23] {
        for height in [0, 1, 2, 4, 8] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 20));
            buffer[(0, 0)].set_symbol("X");
            view.render(
                Rect::new(2, 3, width, height),
                &mut buffer,
                &Theme::compiled(),
                &focus,
            );
            assert_eq!(buffer[(0, 0)].symbol(), "X");
        }
    }
    assert_eq!(view.buffer.selection(), selection);
    assert_eq!(view.buffer.bytes(), &(0..=255).collect::<Vec<_>>());
}
