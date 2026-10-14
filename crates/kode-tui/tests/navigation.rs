use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, Result, TextRange,
    Utf16Column, WorkspaceGeneration,
    document::{Document, TextSnapshot, Utf16Position},
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget,
    },
};
use kode_platform::jobs::{Clock, Notifications, OwnerId, Scheduler};
use kode_tui::{
    editor::{
        input::{EditorInput, HookContext, InputViewport},
        popups::definition::{
            NavigationController, NavigationEffect, NavigationJob, NavigationOperation,
            NavigationResult, PreviewHighlighter, PreviewSourceReader, lexical_identifier, prepare,
            schedule,
        },
        popups::usages::compact_path,
        render::EditorGeometry,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
#[derive(Default)]
struct Provider {
    definitions: Vec<NavigationTarget>,
    references: Vec<NavigationTarget>,
}
impl IntelligenceProvider for Provider {
    fn definitions(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Ok(self.definitions.clone())
    }
    fn references(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Ok(self.references.clone())
    }
    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        Err(DomainError::Unsupported("no test completion".into()))
    }
    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Err(DomainError::Unsupported("no test highlighting".into()))
    }
}
#[derive(Default)]
struct Reader(HashMap<PathBuf, TextSnapshot>);
impl PreviewSourceReader for Reader {
    fn read(&self, target: &NavigationTarget, _: JobTag) -> Result<TextSnapshot> {
        self.0
            .get(&target.path)
            .cloned()
            .ok_or_else(|| DomainError::MissingResource(target.path.display().to_string()))
    }
}
struct Highlighter;
impl PreviewHighlighter for Highlighter {
    fn highlights(&self, _: &TextSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![HighlightSpan {
            range: range(0, 3),
            class: "keyword".into(),
        }])
    }
}
fn range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}
fn target(path: &str, line: usize) -> NavigationTarget {
    NavigationTarget {
        path: PathBuf::from(path),
        document: None,
        identifier_range: range(0, 3),
        location: Some(Utf16Position {
            line: LineIndex(line),
            column: Utf16Column(0),
        }),
        declaration_range: None,
        language: Some("c".into()),
        kind: None,
        name: Some("foo".into()),
        missing_source: false,
    }
}
fn document(path: &str, text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(PathBuf::from(path)),
        Some("c".into()),
        text,
    )
}
struct Harness {
    doc: Document,
    input: EditorInput,
    nav: NavigationController,
    clock: Arc<TestClock>,
    focus: FocusState,
    area: Rect,
}
impl Harness {
    fn new(text: &str) -> Self {
        let doc = document("/project/current.c", text);
        let clock = Arc::new(TestClock::default());
        Self {
            input: EditorInput::new(FocusId(1), doc.id()),
            doc,
            nav: NavigationController::new(
                WorkspaceGeneration(1),
                PathBuf::from("/project"),
                clock.clone(),
            ),
            clock,
            focus: FocusState {
                focused: Some(FocusId(1)),
                ..Default::default()
            },
            area: Rect::new(10, 5, 80, 20),
        }
    }
    fn geometry(&self) -> EditorGeometry {
        EditorGeometry::new(self.area, self.doc.snapshot().len_lines(), 0)
    }
    fn send(&mut self, event: InputEvent) -> Result<bool> {
        let geometry = self.geometry();
        Ok(self
            .input
            .dispatch(
                &event,
                &mut self.doc,
                &mut self.focus,
                InputViewport {
                    geometry,
                    folds: &[],
                },
                &mut self.nav,
            )?
            .handled)
    }
    fn job(&mut self) -> NavigationJob {
        self.nav
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                NavigationEffect::Run(job) => Some(job),
                _ => None,
            })
            .expect("navigation request")
    }
    fn complete(&mut self, job: &NavigationJob, provider: &Provider, reader: &Reader) {
        assert!(self.nav.complete(
            job.tag,
            prepare(job, provider, reader, Some(&Highlighter), || false),
            &self.doc.snapshot(),
            &self.input.state,
            true
        ));
    }
    fn render(&mut self) -> Buffer {
        let geometry = self.geometry();
        let layout = self
            .input
            .layout(
                &self.doc,
                InputViewport {
                    geometry,
                    folds: &[],
                },
            )
            .unwrap();
        let mut buffer = Buffer::empty(self.area);
        self.nav.render(
            HookContext {
                geometry,
                layout: &layout,
            },
            &self.input.state,
            &mut buffer,
            &Theme::compiled(),
        );
        buffer
    }
}
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn pointer(x: u16, y: u16, action: PointerAction, ctrl: bool) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Modifiers {
            ctrl,
            ..Default::default()
        },
    })
}

#[test]
fn ctrl_click_lexical_fallback_routes_definitions_and_declarations_to_references() {
    let mut h = Harness::new("foo\n");
    let geometry = h.geometry();
    h.focus.focused = Some(FocusId(2));
    assert!(
        h.send(pointer(
            geometry.text.x,
            geometry.text.y,
            PointerAction::Down(MouseButton::Left),
            true
        ))
        .unwrap()
    );
    assert_eq!(h.focus.focused, Some(h.input.id));
    assert_eq!(h.focus.captured, None);
    let job = h.job();
    assert_eq!(job.identifier.name, "foo");
    let provider = Provider {
        definitions: vec![target("/project/foo.c", 0)],
        ..Default::default()
    };
    h.complete(&job, &provider, &Reader::default());
    assert!(
        matches!(&h.nav.take_effects()[0],NavigationEffect::Navigate(target) if target.path == std::path::Path::new("/project/foo.c"))
    );
    h.nav
        .set_declaration_spans(&h.doc.snapshot(), vec![range(0, 3)])
        .unwrap();
    h.send(pointer(
        geometry.text.x,
        geometry.text.y,
        PointerAction::Down(MouseButton::Left),
        true,
    ))
    .unwrap();
    let job = h.job();
    assert!(job.declaration_hint);
    h.complete(
        &job,
        &Provider {
            references: vec![target("/project/use.c", 3)],
            ..Default::default()
        },
        &Reader::default(),
    );
    assert_eq!(
        h.nav.usages.as_ref().unwrap().entries[0].label,
        "use.c:4:1 | "
    );
    assert_eq!(
        lexical_identifier(&h.doc.snapshot(), ByteOffset(3))
            .unwrap()
            .unwrap()
            .name,
        "foo"
    );
}

#[test]
fn hover_timer_without_further_mouse_movement_shows_multiple_targets_and_exact_controls() {
    let mut h = Harness::new("foo\n");
    let geometry = h.geometry();
    h.send(pointer(
        geometry.text.x,
        geometry.text.y,
        PointerAction::Move,
        false,
    ))
    .unwrap();
    h.clock.0.store(499, Ordering::SeqCst);
    h.nav.tick(&h.doc.snapshot(), &h.input.state);
    assert!(h.nav.take_effects().is_empty());
    h.clock.0.store(500, Ordering::SeqCst);
    h.nav.tick(&h.doc.snapshot(), &h.input.state);
    let job = h.job();
    let provider = Provider {
        definitions: vec![
            target("/project/first.c", 0),
            target("/project/second.c", 0),
        ],
        ..Default::default()
    };
    let reader = Reader(HashMap::from([
        (
            PathBuf::from("/project/first.c"),
            document("/project/first.c", "int foo(void);\n").snapshot(),
        ),
        (
            PathBuf::from("/project/second.c"),
            document("/project/second.c", "int foo(int x);\n").snapshot(),
        ),
    ]));
    h.complete(&job, &provider, &reader);
    h.render();
    let layout = h
        .input
        .layout(
            &h.doc,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup = h
        .nav
        .definition
        .as_ref()
        .unwrap()
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    h.send(pointer(
        popup.next.x,
        popup.next.y,
        PointerAction::Move,
        false,
    ))
    .unwrap();
    let buffer = h.render();
    assert!(
        buffer[(popup.next.x, popup.next.y)]
            .modifier
            .contains(Modifier::UNDERLINED)
    );
    assert!(
        !buffer[(popup.next.x - 1, popup.next.y)]
            .modifier
            .contains(Modifier::UNDERLINED)
    );
    h.send(pointer(
        popup.next.x,
        popup.next.y,
        PointerAction::Down(MouseButton::Left),
        false,
    ))
    .unwrap();
    assert_eq!(h.nav.definition.as_ref().unwrap().selected, 1);
    let popup = h
        .nav
        .definition
        .as_ref()
        .unwrap()
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    h.send(pointer(
        popup.location.x,
        popup.location.y,
        PointerAction::Down(MouseButton::Left),
        false,
    ))
    .unwrap();
    assert!(
        matches!(&h.nav.take_effects()[0],NavigationEffect::Navigate(target) if target.path == std::path::Path::new("/project/second.c"))
    );
}

#[test]
fn current_unsaved_source_declaration_preview_is_bounded_and_syntax_colored() {
    let mut h = Harness::new(&format!(
        "foo\n{}",
        (0..40)
            .map(|line| format!("unsaved line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    ));
    let mut definition = target("/project/current.c", 0);
    definition.document = Some(h.doc.id());
    definition.declaration_range = Some(range(0, h.doc.snapshot().len_bytes()));
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::Hover,
        )
        .unwrap();
    let job = h.job();
    h.complete(
        &job,
        &Provider {
            definitions: vec![definition],
            ..Default::default()
        },
        &Reader::default(),
    );
    let popup = h.nav.definition.as_ref().unwrap();
    let preview = popup.targets[0].preview.as_ref().unwrap();
    assert_eq!(preview.lines, 24);
    assert_eq!(preview.snapshot.id, h.doc.id());
    assert_eq!(
        preview
            .snapshot
            .line_content(LineIndex(1))
            .unwrap()
            .to_string(),
        "unsaved line 0"
    );
    let geometry = h.geometry();
    let layout = h
        .input
        .layout(
            &h.doc,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup_geometry = h
        .nav
        .definition
        .as_ref()
        .unwrap()
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    let buffer = h.render();
    assert_eq!(
        buffer[(popup_geometry.source.x, popup_geometry.source.y)].fg,
        Theme::compiled().style("keyword").fg.unwrap()
    );
}

#[test]
fn missing_source_retains_location_navigation_and_outside_click_dismisses_before_tick() {
    let mut h = Harness::new("foo\n");
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::Hover,
        )
        .unwrap();
    let job = h.job();
    h.complete(
        &job,
        &Provider {
            definitions: vec![target("/project/missing.c", 7)],
            ..Default::default()
        },
        &Reader::default(),
    );
    assert!(
        h.nav.definition.as_ref().unwrap().targets[0]
            .preview
            .is_err()
    );
    assert_eq!(
        h.nav.definition.as_ref().unwrap().targets[0].location,
        "missing.c:8"
    );
    let buffer = h.render();
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("source unavailable"));
    assert!(
        h.send(pointer(0, 0, PointerAction::Down(MouseButton::Left), false))
            .unwrap()
    );
    assert!(h.nav.definition.is_none());
    h.clock.0.store(2000, Ordering::SeqCst);
    h.nav.tick(&h.doc.snapshot(), &h.input.state);
    assert!(h.nav.take_effects().is_empty());
    assert!(!h.nav.complete(
        job.tag,
        Ok(NavigationResult::Empty),
        &h.doc.snapshot(),
        &h.input.state,
        true
    ));
}

#[test]
fn usage_popup_captures_keys_wheel_and_all_results_preserves_caret_and_padding_hover() {
    let mut h = Harness::new(&format!("foo\n{}", "line\n".repeat(30)));
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::References,
        )
        .unwrap();
    let job = h.job();
    let provider = Provider {
        references: (0..12)
            .map(|index| target(&format!("/project/use{index}.c"), index))
            .collect(),
        ..Default::default()
    };
    h.complete(&job, &provider, &Reader::default());
    h.render();
    let before = h.input.state.caret.selection;
    let geometry = h.geometry();
    let layout = h
        .input
        .layout(
            &h.doc,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup = h
        .nav
        .usages
        .as_ref()
        .unwrap()
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    h.send(pointer(
        popup.area.x,
        popup.rows.y,
        PointerAction::Move,
        false,
    ))
    .unwrap();
    assert_eq!(h.nav.usages.as_ref().unwrap().hovered, None);
    h.send(pointer(
        0,
        0,
        PointerAction::Scroll {
            horizontal: 0,
            vertical: 1,
        },
        false,
    ))
    .unwrap();
    assert_eq!(h.nav.usages.as_ref().unwrap().offset, 1);
    h.send(key(Key::Character('X'))).unwrap();
    assert_eq!(h.doc.revision().0, 0);
    h.send(key(Key::PageDown)).unwrap();
    h.send(key(Key::End)).unwrap();
    assert_eq!(h.nav.usages.as_ref().unwrap().selected, 11);
    assert_eq!(h.input.state.caret.selection, before);
    let buffer = h.render();
    assert!(
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains("use11.c:12")
    );
    h.send(key(Key::Enter)).unwrap();
    assert!(
        matches!(&h.nav.take_effects()[0],NavigationEffect::Navigate(target) if target.path == std::path::Path::new("/project/use11.c"))
    );
}

#[test]
fn usage_track_repositions_then_drag_uses_new_offset_and_release_outside_stops_capture() {
    let mut h = Harness::new("foo");
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::References,
        )
        .unwrap();
    let job = h.job();
    h.complete(
        &job,
        &Provider {
            references: (0..24)
                .map(|index| target(&format!("/project/use{index}.c"), index))
                .collect(),
            ..Default::default()
        },
        &Reader::default(),
    );
    let geometry = h.geometry();
    let layout = h
        .input
        .layout(
            &h.doc,
            InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup = h
        .nav
        .usages
        .as_ref()
        .unwrap()
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    let y = popup.track.y + 3;
    h.send(pointer(
        popup.track.x,
        y,
        PointerAction::Down(MouseButton::Left),
        false,
    ))
    .unwrap();
    let offset = h.nav.usages.as_ref().unwrap().offset;
    assert!(offset > 0);
    h.send(pointer(
        popup.track.x,
        y + 1,
        PointerAction::Drag(MouseButton::Left),
        false,
    ))
    .unwrap();
    assert!(h.nav.usages.as_ref().unwrap().offset > offset);
    h.send(pointer(0, 0, PointerAction::Up(MouseButton::Left), false))
        .unwrap();
    assert!(!h.nav.usages.as_ref().unwrap().is_dragging());
    let offset = h.nav.usages.as_ref().unwrap().offset;
    h.send(pointer(popup.track.x, y + 3, PointerAction::Move, false))
        .unwrap();
    assert_eq!(h.nav.usages.as_ref().unwrap().offset, offset);
}

#[test]
fn stale_context_focus_workspace_and_tiny_geometry_keep_navigation_and_preview_safe() {
    let mut h = Harness::new("foo");
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::Hover,
        )
        .unwrap();
    let job = h.job();
    h.send(key(Key::Right)).unwrap();
    assert!(!h.nav.complete(
        job.tag,
        Ok(NavigationResult::Empty),
        &h.doc.snapshot(),
        &h.input.state,
        true
    ));
    h.nav.switch_workspace();
    assert!(!h.nav.complete(
        job.tag,
        Ok(NavigationResult::Empty),
        &h.doc.snapshot(),
        &h.input.state,
        true
    ));
    for (width, height) in [(0, 0), (1, 1), (2, 2), (10, 4), (80, 20)] {
        h.area = Rect::new(0, 0, width, height);
        h.nav
            .begin(
                &h.doc.snapshot(),
                &h.input.state,
                ByteOffset(0),
                NavigationOperation::Hover,
            )
            .unwrap();
        let job = h.job();
        h.complete(
            &job,
            &Provider {
                definitions: vec![target("/project/missing.c", 0)],
                ..Default::default()
            },
            &Reader::default(),
        );
        h.render();
    }
    h.send(key(Key::Character('A'))).unwrap();
    assert!(h.nav.definition.is_none());
    assert_eq!(h.doc.revision().0, 1); // definition-only permits source editing
    assert_eq!(
        compact_path("src/verylongcomponent/deep/package/sourcefilename.c", 48),
        "src/v/d/package/sourcefilename.c"
    );
}

#[tokio::test]
async fn actual_navigation_worker_uses_injected_services_and_drains_shutdown() {
    let mut h = Harness::new("foo");
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::Click,
        )
        .unwrap();
    let job = h.job();
    let mut scheduler = Scheduler::new(Notifications::new(h.clock.clone()));
    schedule(
        &mut scheduler,
        OwnerId(1),
        job,
        Arc::new(Provider {
            definitions: vec![target("/project/foo.c", 0)],
            ..Default::default()
        }),
        Arc::new(Reader::default()),
        None,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    assert!(h.nav.complete(
        result.tag,
        result.result,
        &h.doc.snapshot(),
        &h.input.state,
        true
    ));
    assert!(matches!(
        &h.nav.take_effects()[0],
        NavigationEffect::Navigate(_)
    ));
    assert!(scheduler.shutdown().await.is_empty());
}

#[tokio::test]
async fn native_preview_reader_loads_real_source_in_the_worker_and_keeps_file_bytes() {
    use kode_platform::files::NativeDocumentIo;
    use kode_tui::editor::popups::definition::NativePreviewSourceReader;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("foo.c");
    std::fs::write(&path, b"\xef\xbb\xbfint foo(void);\r\n").unwrap();
    let mut h = Harness::new("foo");
    h.nav
        .begin(
            &h.doc.snapshot(),
            &h.input.state,
            ByteOffset(0),
            NavigationOperation::Hover,
        )
        .unwrap();
    let job = h.job();
    let mut hit = target(path.to_str().unwrap(), 0);
    hit.identifier_range = range(4, 7);
    let mut scheduler = Scheduler::new(Notifications::new(h.clock.clone()));
    schedule(
        &mut scheduler,
        OwnerId(1),
        job,
        Arc::new(Provider {
            definitions: vec![hit],
            ..Default::default()
        }),
        Arc::new(NativePreviewSourceReader {
            io: Arc::new(NativeDocumentIo),
        }),
        Some(Arc::new(Highlighter)),
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    assert!(h.nav.complete(
        result.tag,
        result.result,
        &h.doc.snapshot(),
        &h.input.state,
        true
    ));
    let preview = h.nav.definition.as_ref().unwrap().targets[0]
        .preview
        .as_ref()
        .unwrap();
    assert_eq!(preview.lines, 1);
    assert_eq!(
        preview
            .snapshot
            .line_content(LineIndex(0))
            .unwrap()
            .to_string(),
        "int foo(void);"
    );
    assert_eq!(
        std::fs::read(path).unwrap(),
        b"\xef\xbb\xbfint foo(void);\r\n"
    );
    assert!(scheduler.shutdown().await.is_empty());
}
