use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, WorkspaceGeneration,
    document::{Document, TextSnapshot},
    editor::search::{SearchReport, scan},
};
use kode_platform::jobs::{MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_tui::{
    editor::{
        input::{EditorEffect, EditorInput, InputViewport},
        render::EditorGeometry,
        search_bar::{SearchAction, SearchBar, SearchEffect, SearchField, schedule_scan},
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use std::{sync::Arc, time::Duration};

struct Harness {
    doc: Document,
    input: EditorInput,
    bar: SearchBar,
    focus: FocusState,
    area: Rect,
}
impl Harness {
    fn new(text: &str) -> Self {
        let doc = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "test".into(),
                key: "search-ui".into(),
                label: "buffer".into(),
            },
            None,
            text,
        );
        Self {
            input: EditorInput::new(FocusId(1), doc.id()),
            doc,
            bar: SearchBar::new(WorkspaceGeneration(1)),
            focus: FocusState {
                focused: Some(FocusId(1)),
                ..Default::default()
            },
            area: Rect::new(12, 8, 80, 12),
        }
    }
    fn geometry(&self) -> EditorGeometry {
        EditorGeometry::new(
            self.area,
            self.doc.snapshot().len_lines(),
            self.bar.preferred_height(),
        )
    }
    fn send(&mut self, event: InputEvent) -> kode_core::Result<()> {
        let geometry = self.geometry();
        let outcome = self.input.dispatch(
            &event,
            &mut self.doc,
            &mut self.focus,
            InputViewport {
                geometry,
                folds: &[],
            },
            &mut self.bar,
        )?;
        for effect in outcome.effects {
            match effect {
                EditorEffect::OpenSearch(selected) => {
                    self.bar.open(&self.doc.snapshot(), &selected)
                }
                EditorEffect::Changed { .. } => self.bar.document_changed(&self.doc.snapshot()),
                _ => {}
            }
        }
        Ok(())
    }
    fn request(&mut self) -> (JobTag, TextSnapshot, String) {
        self.bar
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                SearchEffect::Scan {
                    tag,
                    snapshot,
                    query,
                } => Some((tag, snapshot, query)),
                _ => None,
            })
            .expect("pending scan")
    }
    fn complete(&mut self) {
        let (tag, snapshot, query) = self.request();
        self.bar
            .complete_scan(
                tag,
                scan(&snapshot, &query, || false),
                &mut self.doc,
                &mut self.input.state,
            )
            .unwrap();
    }
    fn open(&mut self) {
        self.send(ctrl('f', false)).unwrap();
    }
    fn query(&mut self, query: &str) {
        self.bar.focused = SearchField::Find;
        self.send(ctrl('a', false)).unwrap();
        self.send(InputEvent::Paste(query.into())).unwrap();
    }
}
fn key(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers,
        repeat: false,
    })
}
fn plain(value: Key) -> InputEvent {
    key(value, Modifiers::default())
}
fn ctrl(ch: char, shift: bool) -> InputEvent {
    key(
        Key::Character(ch),
        Modifiers {
            ctrl: true,
            shift,
            ..Default::default()
        },
    )
}
fn mouse(x: u16, y: u16) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}

#[test]
fn source_uppercase_replace_click_prefill_tab_and_close_follow_editor_route() {
    let mut h = Harness::new("A.*B line");
    h.input.state.caret.selection.head.0 = 4;
    h.open();
    assert_eq!(h.bar.find.value, r"A\.\*B");
    assert_eq!(h.geometry().text.y, h.area.y + 3);
    h.query("");
    h.send(plain(Key::Character('A'))).unwrap();
    assert_eq!(h.bar.find.value, "A");
    let geometry = h.bar.geometry(h.geometry().search);
    h.send(mouse(geometry.replace.x, geometry.replace.y))
        .unwrap();
    h.send(plain(Key::Character('B'))).unwrap();
    assert_eq!(h.bar.replace.value, "B");
    assert_eq!(h.bar.find.value, "A");
    h.send(plain(Key::Tab)).unwrap();
    assert_eq!(h.bar.focused, SearchField::Find);
    h.send(mouse(geometry.close.x, geometry.close.y)).unwrap();
    assert!(!h.bar.visible);
    assert!(!h.input.state.search_has_focus);
    assert_eq!(h.geometry().text.y, h.area.y + 1);
    h.send(plain(Key::Character('X'))).unwrap();
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        "X line"
    );
}

#[test]
fn next_all_replace_current_all_and_undo_refresh_counts() {
    let mut h = Harness::new("one two one two");
    h.open();
    h.query("one");
    h.complete();
    assert_eq!(h.bar.label(), "0/2");
    h.send(plain(Key::Enter)).unwrap();
    assert_eq!(h.bar.label(), "1/2");
    assert_eq!(
        h.input
            .state
            .caret
            .selected_text(&h.doc.snapshot())
            .unwrap(),
        "one"
    );
    h.send(key(
        Key::Enter,
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        h.bar.overlays(&h.doc.snapshot(), &Theme::compiled()).len(),
        2
    );
    h.send(plain(Key::Tab)).unwrap();
    h.send(plain(Key::Character('1'))).unwrap();
    h.send(ctrl('r', false)).unwrap();
    h.complete();
    assert_eq!(h.bar.results.report.as_ref().unwrap().matches.len(), 1);
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        "1 two one two"
    );
    h.send(ctrl('r', true)).unwrap();
    h.complete();
    assert_eq!(h.bar.label(), "no matches");
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        "1 two 1 two"
    );
    let area = h.geometry().text;
    h.send(mouse(area.x, area.y)).unwrap();
    h.send(InputEvent::Pointer(PointerInput {
        x: area.x,
        y: area.y,
        action: PointerAction::Up(MouseButton::Left),
        modifiers: Modifiers::default(),
    }))
    .unwrap();
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    h.complete();
    assert_eq!(h.bar.results.report.as_ref().unwrap().matches.len(), 1);
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        "1 two one two"
    );
}

#[test]
fn pending_enter_waits_for_current_scan_and_changed_closed_edited_results_are_rejected() {
    let mut h = Harness::new("one two");
    h.open();
    h.query("one");
    let old = h.request();
    h.query("two");
    let current = h.request();
    h.send(plain(Key::Enter)).unwrap();
    assert!(
        !h.bar
            .complete_scan(
                old.0,
                scan(&old.1, &old.2, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    assert!(
        h.bar
            .complete_scan(
                current.0,
                scan(&current.1, &current.2, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    assert_eq!(
        h.input
            .state
            .caret
            .selected_text(&h.doc.snapshot())
            .unwrap(),
        "two"
    );
    h.query("one");
    let stale = h.request();
    let area = h.geometry().text;
    h.send(mouse(area.x, area.y)).unwrap();
    h.send(plain(Key::Character('X'))).unwrap();
    assert!(
        !h.bar
            .complete_scan(
                stale.0,
                scan(&stale.1, &stale.2, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    let late = h.request();
    h.bar.close(&mut h.input.state);
    assert!(
        !h.bar
            .complete_scan(
                late.0,
                scan(&late.1, &late.2, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    assert!(
        h.bar
            .action(SearchAction::Next, &mut h.doc, &mut h.input.state)
            .is_err()
    );
}

#[test]
fn regex_errors_and_readonly_replacement_are_visible_and_keep_document() {
    let mut h = Harness::new("text");
    h.open();
    h.query("[");
    h.complete();
    assert_eq!(h.bar.label(), "regex error");
    assert!(h.bar.error.as_ref().is_some_and(|error| !error.is_empty()));
    let area = h.geometry().search;
    let geometry = h.bar.geometry(area);
    let mut buffer = Buffer::empty(h.area);
    h.bar.render(area, &mut buffer, &Theme::compiled(), true);
    assert_eq!(
        buffer[(geometry.status.x, geometry.status.y)].fg,
        Color::Rgb(255, 95, 95)
    );
    h.query("text");
    h.complete();
    h.doc.set_read_only(true);
    assert!(matches!(
        h.send(ctrl('r', true)),
        Err(DomainError::ReadOnly)
    ));
    assert_eq!(h.doc.snapshot().to_service_snapshot().text.as_ref(), "text");
    h.send(plain(Key::Enter)).unwrap();
    assert_eq!(
        h.input
            .state
            .caret
            .selected_text(&h.doc.snapshot())
            .unwrap(),
        "text"
    );
}

#[test]
fn field_clipboard_focus_loss_and_tiny_geometry_keep_valid_state() {
    let mut h = Harness::new("text");
    h.open();
    h.query("AB");
    h.send(ctrl('c', false)).unwrap();
    assert!(
        h.bar
            .take_effects()
            .iter()
            .any(|effect| matches!(effect, SearchEffect::Copy(text) if text == "AB"))
    );
    h.send(ctrl('v', false)).unwrap();
    let ticket = h
        .bar
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            SearchEffect::ReadClipboard(ticket) => Some(ticket),
            _ => None,
        })
        .unwrap();
    h.send(InputEvent::Focus(false)).unwrap();
    h.send(InputEvent::Focus(true)).unwrap();
    assert!(
        !h.bar
            .complete_paste(
                &ticket,
                "STALE",
                &h.doc,
                &h.input.state,
                h.input.has_focus(&h.focus)
            )
            .unwrap()
    );
    h.send(ctrl('v', false)).unwrap();
    let ticket = h
        .bar
        .take_effects()
        .into_iter()
        .find_map(|effect| match effect {
            SearchEffect::ReadClipboard(ticket) => Some(ticket),
            _ => None,
        })
        .unwrap();
    assert!(
        h.bar
            .complete_paste(
                &ticket,
                "中",
                &h.doc,
                &h.input.state,
                h.input.has_focus(&h.focus)
            )
            .unwrap()
    );
    assert_eq!(h.bar.find.value, "AB中");
    for (width, height) in [(0, 0), (1, 1), (2, 2), (20, 6), (80, 24)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        h.bar.render(area, &mut buffer, &Theme::compiled(), true);
        let geometry = h.bar.geometry(area);
        assert_eq!(geometry.find.intersection(geometry.close).width, 0);
    }
}

#[tokio::test]
async fn real_search_worker_queue_publishes_report_and_drains_shutdown() {
    let mut h = Harness::new("one\ntwo\none");
    h.open();
    h.query("one");
    let (tag, snapshot, query) = h.request();
    let mut scheduler: Scheduler<SearchReport> =
        Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_scan(&mut scheduler, OwnerId(1), tag, snapshot, query).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        h.bar
            .complete_scan(result.tag, result.result, &mut h.doc, &mut h.input.state)
            .unwrap()
    );
    assert_eq!(h.bar.label(), "0/2");
    assert!(scheduler.shutdown().await.is_empty());
}

#[test]
fn alt_enter_waits_for_current_matches_selects_all_and_returns_to_multi_editing() {
    let mut h = Harness::new("é one one\none");
    h.open();
    h.query("one");
    h.send(key(
        Key::Enter,
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(h.input.state.search_has_focus);
    h.complete();
    assert!(!h.input.state.search_has_focus);
    assert_eq!(h.input.state.caret.selections().count(), 3);
    assert!(
        h.input.state.caret.selections().all(|selection| h
            .doc
            .snapshot()
            .slice(selection.range())
            .unwrap()
            == "one")
    );
    let before = h.input.state.caret.clone();
    h.send(InputEvent::Paste("界".into())).unwrap();
    assert_eq!(h.doc.snapshot().text().as_ref(), "é 界 界\n界");
    assert_eq!(h.doc.history_depth(), (1, 0));
    h.send(key(
        Key::Character('u'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(h.doc.snapshot().text().as_ref(), "é one one\none");
    assert_eq!(h.input.state.caret, before);
}

#[test]
fn pending_select_all_cannot_survive_query_focus_close_or_invalid_no_match() {
    let mut h = Harness::new("one two one");
    h.open();
    h.query("one");
    let (old_tag, old_snapshot, old_query) = h.request();
    let all = key(
        Key::Enter,
        Modifiers {
            alt: true,
            ..Default::default()
        },
    );
    h.send(all.clone()).unwrap();
    h.query("two");
    assert!(
        !h.bar
            .complete_scan(
                old_tag,
                scan(&old_snapshot, &old_query, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    h.complete();
    assert!(h.input.state.caret.secondary.is_empty());
    h.query("one");
    h.send(all.clone()).unwrap();
    h.send(InputEvent::Focus(false)).unwrap();
    h.complete();
    assert!(h.input.state.caret.secondary.is_empty());
    h.send(InputEvent::Focus(true)).unwrap();
    for query in ["absent", "("] {
        h.query(query);
        h.complete();
        h.send(all.clone()).unwrap();
        assert!(h.input.state.caret.secondary.is_empty());
        assert!(h.input.state.search_has_focus);
    }
    h.query("one");
    let (tag, snapshot, query) = h.request();
    h.send(all).unwrap();
    h.send(plain(Key::Escape)).unwrap();
    assert!(
        !h.bar
            .complete_scan(
                tag,
                scan(&snapshot, &query, || false),
                &mut h.doc,
                &mut h.input.state
            )
            .unwrap()
    );
    assert!(h.input.state.caret.secondary.is_empty());
}
