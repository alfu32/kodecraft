use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, Result, Selection, TextRange, document::Document,
};
use kode_platform::jobs::{CancellationToken, Clock};
use kode_tui::{
    editor::{
        input::{DOUBLE_CLICK_MS, EditorEffect, EditorInput, InputViewport},
        layout::CollapsedFold,
        render::EditorGeometry,
        words::WordJob,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
};
use ratatui::layout::Rect;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Harness {
    doc: Document,
    view: EditorInput,
    clock: Arc<FakeClock>,
    focus: FocusState,
    area: Rect,
    folds: Vec<CollapsedFold>,
    jobs: Vec<WordJob>,
}
impl Harness {
    fn new(text: &str) -> Self {
        let doc = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "test".into(),
                key: "word".into(),
                label: "word".into(),
            },
            None,
            text,
        );
        let clock = Arc::new(FakeClock::default());
        Self {
            view: EditorInput::with_clock(FocusId(21), doc.id(), clock.clone()),
            doc,
            clock,
            focus: FocusState {
                focused: Some(FocusId(21)),
                ..Default::default()
            },
            area: Rect::new(7, 5, 100, 20),
            folds: vec![],
            jobs: vec![],
        }
    }
    fn advance(&self, ms: u64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn geometry(&self) -> EditorGeometry {
        EditorGeometry::new(self.area, self.doc.snapshot().len_lines(), 0)
    }
    fn send(&mut self, event: InputEvent) -> Result<()> {
        let geometry = self.geometry();
        let outcome = self.view.dispatch(
            &event,
            &mut self.doc,
            &mut self.focus,
            InputViewport {
                geometry,
                folds: &self.folds,
            },
            &mut (),
        )?;
        for effect in outcome.effects {
            if let EditorEffect::SelectWord(job) = effect {
                self.jobs.push(job);
            }
        }
        Ok(())
    }
    fn cell(&mut self, byte: usize, inside: u16) -> (u16, u16) {
        let geometry = self.geometry();
        let layout = self
            .view
            .layout(
                &self.doc,
                InputViewport {
                    geometry,
                    folds: &self.folds,
                },
            )
            .unwrap();
        let (row, column) = layout.position_to_cell(ByteOffset(byte)).unwrap();
        (
            geometry.text.x + column as u16 + inside,
            geometry.text.y + row as u16,
        )
    }
    fn pointer(&mut self, cell: (u16, u16), action: PointerAction, alt: bool) {
        self.send(InputEvent::Pointer(PointerInput {
            x: cell.0,
            y: cell.1,
            action,
            modifiers: Modifiers {
                alt,
                ..Default::default()
            },
        }))
        .unwrap();
    }
    fn click(&mut self, cell: (u16, u16), alt: bool) {
        self.pointer(cell, PointerAction::Down(MouseButton::Left), alt);
        self.pointer(cell, PointerAction::Up(MouseButton::Left), alt);
    }
    fn double(&mut self, cell: (u16, u16), alt: bool) -> WordJob {
        self.click(cell, alt);
        self.advance(100);
        self.click(cell, alt);
        self.jobs.pop().expect("word job from double-click")
    }
    fn publish(&mut self, job: &WordJob, range: TextRange) -> bool {
        let geometry = self.geometry();
        self.view
            .complete_word(
                job.request,
                range,
                &self.doc,
                &self.focus,
                InputViewport {
                    geometry,
                    folds: &self.folds,
                },
            )
            .unwrap()
    }
    fn complete(&mut self, job: &WordJob) {
        let range = job.run(&CancellationToken::new()).unwrap();
        assert!(self.publish(job, range));
    }
    fn selected(&self) -> String {
        self.view
            .state
            .caret
            .selected_text(&self.doc.snapshot())
            .unwrap()
    }
}
fn selection(anchor: usize, head: usize) -> Selection {
    Selection {
        anchor: ByteOffset(anchor),
        head: ByteOffset(head),
    }
}
fn key(value: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key: value,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}

#[test]
fn ordinary_double_selects_whole_word_then_typing_and_undo_use_the_selected_range() {
    let mut h = Harness::new("foo bar baz");
    let cell = h.cell(5, 0);
    let job = h.double(cell, false);
    assert!(h.focus.captured.is_none());
    assert_eq!(h.doc.revision().0, 0);
    h.complete(&job);
    assert_eq!(h.selected(), "bar");
    assert_eq!(h.view.state.caret.cursor(), ByteOffset(7));
    h.send(InputEvent::Paste("X".into())).unwrap();
    assert_eq!(h.doc.snapshot().text().as_ref(), "foo X baz");
    h.doc.undo(&mut h.view.state.caret).unwrap();
    assert_eq!(h.selected(), "bar");
    assert_eq!(h.doc.snapshot().text().as_ref(), "foo bar baz");
}

#[test]
fn alt_double_restores_first_click_toggled_cursors_and_adds_words_without_duplicates() {
    let mut h = Harness::new("foo bar baz");
    h.view.state.caret.selection = selection(3, 0);
    h.view.state.caret.secondary = vec![selection(5, 5), selection(8, 8)];
    let cell = h.cell(5, 0);
    let job = h.double(cell, true);
    h.complete(&job);
    assert_eq!(h.selected(), "bar");
    assert_eq!(
        h.view.state.caret.secondary,
        vec![selection(3, 0), selection(8, 8)]
    );
    h.advance(500);
    let cell = h.cell(1, 0);
    let job = h.double(cell, true);
    h.complete(&job);
    assert_eq!(h.selected(), "foo");
    assert_eq!(
        h.view.state.caret.secondary,
        vec![selection(4, 7), selection(8, 8)]
    );
    let before = h.view.state.caret.clone();
    h.send(key(Key::Character('X'))).unwrap();
    assert_eq!(h.doc.snapshot().text().as_ref(), "X X Xbaz");
    assert_eq!(h.doc.history_depth(), (1, 0));
    h.doc.undo(&mut h.view.state.caret).unwrap();
    assert_eq!(h.view.state.caret, before);
}

#[test]
fn double_click_interval_release_modifier_and_repeated_pairs_are_deterministic() {
    for (delay, expected) in [(DOUBLE_CLICK_MS, true), (DOUBLE_CLICK_MS + 1, false)] {
        let mut h = Harness::new("word");
        let cell = h.cell(1, 0);
        h.click(cell, false);
        h.advance(delay);
        h.click(cell, false);
        assert_eq!(!h.jobs.is_empty(), expected);
    }
    let mut h = Harness::new("word");
    let cell = h.cell(1, 0);
    h.pointer(cell, PointerAction::Down(MouseButton::Left), false);
    h.pointer(cell, PointerAction::Down(MouseButton::Left), false);
    assert!(h.jobs.is_empty());
    h.pointer(cell, PointerAction::Up(MouseButton::Left), false);
    h.advance(500);
    h.click(cell, false);
    h.advance(100);
    h.click(cell, true);
    assert!(h.jobs.is_empty());
    h.advance(500);
    let job = h.double(cell, false);
    h.complete(&job);
    h.advance(100);
    h.click(cell, false);
    assert!(h.jobs.is_empty()); // third starts a new pair
    h.advance(100);
    h.click(cell, false);
    let job = h.jobs.pop().unwrap();
    h.complete(&job);
    assert_eq!(h.selected(), "word");
}

#[test]
fn painted_source_under_tab_wide_combining_emoji_and_wrapped_glyphs_is_used() {
    for (text, byte, inside, expected) in [
        ("éclair end", 2, 0, "éclair"),
        ("e\u{301}cole end", 1, 3, "e\u{301}cole"),
        ("世界next", 0, 4, "世"),
        ("😀next", 0, 4, "😀"),
        ("one\ttwo", 3, 2, "\t"),
    ] {
        let mut h = Harness::new(text);
        let cell = h.cell(byte, inside);
        let job = h.double(cell, false);
        h.complete(&job);
        assert_eq!(h.selected(), expected);
    }
    let mut h = Harness::new("prefix abcdefghijklmnop suffix");
    h.area.width = 13; // ten text cells; middle of a word is wrapped
    let cell = h.cell(12, 0);
    let job = h.double(cell, false);
    h.complete(&job);
    assert_eq!(h.selected(), "abcdefghijklmnop");
    let mut h = Harness::new("head {\n body\n} tailword");
    h.folds = vec![CollapsedFold {
        start: kode_core::LineIndex(0),
        end: kode_core::LineIndex(2),
    }];
    let byte = "head {\n body\n} ".len();
    let cell = h.cell(byte, 0);
    let job = h.double(cell, false);
    h.complete(&job);
    assert_eq!(h.selected(), "tailword");
}

#[test]
fn whitespace_punctuation_empty_and_readonly_selection_have_no_mutation_path() {
    for (text, byte, expected) in [
        ("a  b", 1, "  "),
        ("a::b", 1, ":"),
        ("", 0, ""),
        ("a\n\n", 2, ""),
    ] {
        let mut h = Harness::new(text);
        let cell = h.cell(byte, 0);
        let job = h.double(cell, false);
        h.doc.set_read_only(true);
        h.complete(&job);
        assert_eq!(h.selected(), expected);
        assert!(h.send(InputEvent::Paste("MUTATE".into())).is_err());
        assert_eq!(h.doc.snapshot().text().as_ref(), text);
    }
}

#[test]
fn stale_results_and_cancellation_cannot_overwrite_a_changed_or_newer_selection() {
    for change in 0..8 {
        let mut h = Harness::new("word next");
        let cell = h.cell(1, 0);
        let job = h.double(cell, false);
        let range = job.run(&CancellationToken::new()).unwrap();
        match change {
            0 => h.send(key(Key::Right)).unwrap(),
            1 => h.send(InputEvent::Paste("X".into())).unwrap(),
            2 => h.send(InputEvent::Focus(false)).unwrap(),
            3 => {
                h.area.width -= 1;
                h.send(InputEvent::Resize {
                    width: 99,
                    height: 20,
                })
                .unwrap();
            }
            4 => h.view.state.caret.secondary.push(selection(8, 8)),
            5 => h.focus.focused = Some(FocusId(999)),
            6 => h.view.state.scroll_rows = 1,
            _ => h.pointer(
                cell,
                PointerAction::Scroll {
                    vertical: 1,
                    horizontal: 0,
                },
                false,
            ),
        }
        let before = h.view.state.caret.clone();
        assert!(!h.publish(&job, range));
        assert_eq!(h.view.state.caret, before);
    }
    let mut h = Harness::new("word next");
    let cell = h.cell(1, 0);
    let old = h.double(cell, false);
    let old_range = old.run(&CancellationToken::new()).unwrap();
    h.advance(500);
    let other = h.cell(6, 0);
    let new = h.double(other, false);
    assert!(!h.publish(&old, old_range));
    h.complete(&new);
    assert_eq!(h.selected(), "next");
    let cell = h.cell(1, 0);
    h.advance(500);
    let job = h.double(cell, false);
    let token = job.cancel.clone();
    drop(h);
    assert!(token.is_cancelled());
}

#[test]
fn drag_resize_and_intervening_keys_break_a_click_pair_and_both_worker_tokens_cancel() {
    for change in 0..4 {
        let mut h = Harness::new("word next");
        let cell = h.cell(1, 0);
        h.click(cell, false);
        match change {
            0 => h.send(key(Key::Escape)).unwrap(),
            1 => h
                .send(InputEvent::Resize {
                    width: 100,
                    height: 20,
                })
                .unwrap(),
            2 => {
                h.pointer(cell, PointerAction::Down(MouseButton::Left), false);
                h.pointer(
                    (cell.0 + 1, cell.1),
                    PointerAction::Drag(MouseButton::Left),
                    false,
                );
                h.pointer(cell, PointerAction::Up(MouseButton::Left), false);
            }
            _ => h.pointer(cell, PointerAction::Down(MouseButton::Right), false),
        }
        h.advance(50);
        h.click(cell, false);
        assert!(
            h.jobs.iter().all(|job| job.cancel.is_cancelled()),
            "case {change}"
        );
    }
    let mut h = Harness::new("word");
    let cell = h.cell(1, 0);
    let job = h.double(cell, false);
    let scheduler = CancellationToken::new();
    scheduler.cancel();
    assert!(job.run(&scheduler).is_err());
    job.cancel.cancel();
    assert!(job.run(&CancellationToken::new()).is_err());
}

#[test]
fn canceling_pending_alt_word_restores_the_first_click_without_duplicate_cursors() {
    let mut h = Harness::new("foo bar");
    h.view.state.caret.selection = selection(1, 1);
    h.view.state.caret.secondary = vec![selection(5, 5)];
    let cell = h.cell(5, 0);
    h.click(cell, true);
    let after_first = h.view.state.caret.clone();
    h.advance(50);
    h.click(cell, true);
    let job = h.jobs.pop().unwrap();
    h.view.cancel_pointer_work();
    assert!(job.cancel.is_cancelled());
    assert_eq!(h.view.state.caret, after_first);
    assert_eq!(h.view.state.caret.selections().count(), 1);
}

#[test]
fn zero_and_tiny_panes_ignore_nontext_double_clicks_without_out_of_bounds_hits() {
    for width in 0..=8 {
        for height in 0..=4 {
            let mut h = Harness::new("é😀\n");
            h.area = Rect::new(7, 5, width, height);
            let cell = (7, 5);
            h.click(cell, false);
            h.advance(20);
            h.click(cell, false);
            assert!(h.jobs.is_empty());
            h.view.state.caret.validate(&h.doc.snapshot()).unwrap();
        }
    }
}
