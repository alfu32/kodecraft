use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Result, TextRange, WorkspaceGeneration,
    document::Document,
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget,
    },
};
use kode_platform::jobs::{MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_tui::{
    editor::{
        input::HookContext,
        input::{EditorInput, InputViewport},
        popups::completion::{
            CompletionPopup, HostCompletionProvider, HostSuggestion, prepare, schedule,
        },
        render::EditorGeometry,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{sync::Arc, time::Duration};

struct Host(Vec<String>);
impl HostCompletionProvider for Host {
    fn suggestions(&self, _: &IntelligenceRequest, _: &str) -> Result<Vec<HostSuggestion>> {
        Ok(self
            .0
            .iter()
            .map(|label| HostSuggestion {
                label: label.clone(),
                insert_text: None,
                detail: Some("HTTP header".into()),
            })
            .collect())
    }
}
struct Provider(Vec<CompletionItem>);
impl IntelligenceProvider for Provider {
    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        Ok(self.0.clone())
    }
    fn definitions(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Err(DomainError::Unsupported("test has no navigation".into()))
    }
    fn references(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Err(DomainError::Unsupported("test has no references".into()))
    }
    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Err(DomainError::Unsupported("test has no highlights".into()))
    }
}
struct Harness {
    doc: Document,
    input: EditorInput,
    popup: CompletionPopup,
    focus: FocusState,
    area: Rect,
}
impl Harness {
    fn new(text: &str, cursor: usize) -> Self {
        let doc = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "test".into(),
                key: "completion".into(),
                label: "headers".into(),
            },
            None,
            text,
        );
        let mut input = EditorInput::new(FocusId(1), doc.id());
        input
            .state
            .caret
            .move_to(&doc.snapshot(), ByteOffset(cursor), false)
            .unwrap();
        Self {
            doc,
            input,
            popup: CompletionPopup::new(WorkspaceGeneration(1)),
            focus: FocusState {
                focused: Some(FocusId(1)),
                ..Default::default()
            },
            area: Rect::new(10, 5, 40, 8),
        }
    }
    fn geometry(&self) -> EditorGeometry {
        EditorGeometry::new(self.area, self.doc.snapshot().len_lines(), 0)
    }
    fn open(
        &mut self,
        host: Option<&dyn HostCompletionProvider>,
        provider: Option<&dyn IntelligenceProvider>,
    ) {
        let job = self
            .popup
            .begin(&self.doc.snapshot(), &self.input.state)
            .unwrap();
        self.popup
            .complete(
                job.tag,
                prepare(&job, host, provider, || false),
                &self.doc.snapshot(),
                &self.input.state,
                true,
            )
            .unwrap();
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
                &mut self.popup,
            )?
            .handled)
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
        self.popup.render(
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
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Modifiers::default(),
    })
}
fn item(label: &str, text: &str, range: TextRange) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        insert_text: text.into(),
        replacement_range: range,
        detail: Some("provider".into()),
        source: "fake".into(),
        kind: None,
    }
}

#[test]
fn original_long_list_navigation_scroll_and_selection_beyond_viewport() {
    let host = Host((0..24).map(|index| format!("Header{index}")).collect());
    let mut h = Harness::new("H", 1);
    h.open(Some(&host), None);
    assert!(h.popup.entries.len() >= 24);
    let buffer = h.render();
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| matches!(cell.symbol(), "█" | "│"))
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
        .popup
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    h.send(pointer(
        popup.text.x,
        popup.rows.y,
        PointerAction::Scroll {
            horizontal: 0,
            vertical: 2,
        },
    ))
    .unwrap();
    assert_eq!(h.popup.offset, 2);
    h.send(key(Key::Home)).unwrap();
    for _ in 0..6 {
        h.send(key(Key::Down)).unwrap();
    }
    assert_eq!(h.popup.selected, 6);
    assert!(h.popup.offset > 0);
    h.popup.auto_show_external = true;
    h.send(key(Key::Enter)).unwrap();
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        "Header6"
    );
    assert_eq!(h.doc.revision().0, 1);
    assert!(h.popup.entries.is_empty());
    assert!(
        h.popup
            .on_edit(&h.doc.snapshot(), &h.input.state, true)
            .unwrap()
            .is_none()
    );
    assert!(h.doc.undo(&mut h.input.state.caret).unwrap());
    assert_eq!(h.doc.snapshot().to_service_snapshot().text.as_ref(), "H");
}

#[test]
fn completion_sources_dedup_limit_authority_and_member_fallback_are_explicit() {
    let mut h = Harness::new("HeaderLocal HeaderHost HeaderOther", 6);
    let job = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    let host = Host(vec!["HeaderHost".into(), "HeaderHost".into()]);
    let provider = Provider(vec![
        item("HeaderProvider", "inserted", job.prefix),
        item("HeaderHost", "provider_duplicate", job.prefix),
    ]);
    let entries = prepare(&job, Some(&host), Some(&provider), || false).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["HeaderHost", "HeaderProvider"]
    );
    assert_eq!(entries[0].source, "host");
    assert_eq!(entries[1].insert_text, "inserted");
    assert_eq!(
        prepare(&job, None, None, || false)
            .unwrap()
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["HeaderLocal", "HeaderHost", "HeaderOther"]
    );
    let host = Host((0..100).map(|index| format!("Header{index}")).collect());
    assert_eq!(
        prepare(&job, Some(&host), None, || false).unwrap().len(),
        50
    );
    for text in ["customer.", "customer?.", "customer->", "customer::"] {
        let mut h = Harness::new(text, text.len());
        let job = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
        assert!(prepare(&job, None, None, || false).unwrap().is_empty());
    }
    let mut h = Harness::new("receiver.\nlocal_name", 10);
    let job = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    assert!(
        prepare(&job, None, None, || false)
            .unwrap()
            .iter()
            .any(|entry| entry.label == "local_name")
    );
}

#[test]
fn insertion_honors_provider_text_and_prefix_range_preserves_receiver_and_suffix() {
    for (text, cursor, expected) in [
        ("customer.", 9, "customer.name()"),
        ("customer.na", 11, "customer.name()"),
        ("customer?.naSuffix", 12, "customer?.name()Suffix"),
    ] {
        let mut h = Harness::new(text, cursor);
        let job = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
        let provider = Provider(vec![item("name", "name()", job.prefix)]);
        h.popup
            .complete(
                job.tag,
                prepare(&job, None, Some(&provider), || false),
                &h.doc.snapshot(),
                &h.input.state,
                true,
            )
            .unwrap();
        h.send(key(Key::Enter)).unwrap();
        assert_eq!(
            h.doc.snapshot().to_service_snapshot().text.as_ref(),
            expected
        );
        assert_eq!(h.doc.revision().0, 1);
    }
    let mut h = Harness::new("abc", 3);
    h.open(Some(&Host(vec!["xyz".into()])), None);
    h.doc.set_read_only(true);
    assert!(matches!(
        h.send(key(Key::Enter)),
        Err(DomainError::ReadOnly)
    ));
    assert_eq!(h.doc.snapshot().to_service_snapshot().text.as_ref(), "abc");
}

#[test]
fn requests_cannot_resurrect_dismissed_moved_edited_unfocused_or_new_workspace_popups() {
    let mut h = Harness::new("H", 1);
    let host = Host(vec!["Header".into()]);
    let old = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    h.popup.dismiss();
    assert!(
        !h.popup
            .complete(
                old.tag,
                prepare(&old, Some(&host), None, || false),
                &h.doc.snapshot(),
                &h.input.state,
                true
            )
            .unwrap()
    );
    let old = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    h.send(key(Key::Left)).unwrap();
    assert!(
        !h.popup
            .complete(
                old.tag,
                prepare(&old, Some(&host), None, || false),
                &h.doc.snapshot(),
                &h.input.state,
                true
            )
            .unwrap()
    );
    let old = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    h.send(key(Key::Character('X'))).unwrap();
    assert!(
        !h.popup
            .complete(
                old.tag,
                prepare(&old, Some(&host), None, || false),
                &h.doc.snapshot(),
                &h.input.state,
                true
            )
            .unwrap()
    );
    let old = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    assert!(
        !h.popup
            .complete(
                old.tag,
                prepare(&old, Some(&host), None, || false),
                &h.doc.snapshot(),
                &h.input.state,
                false
            )
            .unwrap()
    );
    h.popup.switch_workspace();
    assert!(
        !h.popup
            .complete(
                old.tag,
                prepare(&old, Some(&host), None, || false),
                &h.doc.snapshot(),
                &h.input.state,
                true
            )
            .unwrap()
    );
    assert!(
        h.popup
            .on_edit(&h.doc.snapshot(), &h.input.state, true)
            .unwrap()
            .is_none()
    );
    h.popup.auto_show_external = true;
    assert!(
        h.popup
            .on_edit(&h.doc.snapshot(), &h.input.state, true)
            .unwrap()
            .is_some()
    );
}

#[test]
fn popup_track_click_row_click_outside_click_and_tiny_bounds_do_not_edit_underneath() {
    let host = Host((0..24).map(|index| format!("Header{index}")).collect());
    let mut h = Harness::new("H", 1);
    h.open(Some(&host), None);
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
        .popup
        .geometry(
            HookContext {
                geometry,
                layout: &layout,
            },
            &h.input.state,
        )
        .unwrap();
    h.send(pointer(
        popup.track.x,
        popup.track.bottom() - 1,
        PointerAction::Down(MouseButton::Left),
    ))
    .unwrap();
    assert!(h.popup.offset > 0);
    let selected = h.popup.entries[h.popup.offset].insert_text.clone();
    h.send(pointer(
        popup.text.x,
        popup.rows.y,
        PointerAction::Down(MouseButton::Left),
    ))
    .unwrap();
    assert_eq!(
        h.doc.snapshot().to_service_snapshot().text.as_ref(),
        selected
    );
    h.open(Some(&host), None);
    let before = h.input.state.caret.selection;
    assert!(
        h.send(pointer(0, 0, PointerAction::Down(MouseButton::Left)))
            .unwrap()
    );
    assert!(h.popup.entries.is_empty());
    assert_eq!(h.input.state.caret.selection, before);
    assert_eq!(h.focus.captured, None);
    for (width, height) in [(0, 0), (1, 1), (2, 2), (20, 6), (40, 8)] {
        h.area = Rect::new(0, 0, width, height);
        h.open(Some(&host), None);
        h.render();
        h.send(key(Key::End)).unwrap();
        h.send(key(Key::PageUp)).unwrap();
        h.send(key(Key::PageDown)).unwrap();
    }
}

#[tokio::test]
async fn actual_completion_worker_calls_injected_host_and_drains_shutdown() {
    let mut h = Harness::new("H", 1);
    let job = h.popup.begin(&h.doc.snapshot(), &h.input.state).unwrap();
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule(
        &mut scheduler,
        OwnerId(1),
        job,
        Some(Arc::new(Host(vec!["Header".into()]))),
        None,
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        h.popup
            .complete(
                result.tag,
                result.result,
                &h.doc.snapshot(),
                &h.input.state,
                true
            )
            .unwrap()
    );
    assert_eq!(h.popup.entries[0].label, "Header");
    assert!(scheduler.shutdown().await.is_empty());
}

#[test]
fn native_completion_items_render_and_apply_unicode_member_and_qualified_import_replacements() {
    use kode_intelligence::{
        completion::{CompletionContext, CompletionEngine},
        frontend::kotlin::KotlinSemanticAdapter,
        ids::SemanticIds,
        resolver::SemanticResolver,
        snapshot::SemanticSnapshot,
    };
    use kode_platform::jobs::CancellationToken;
    use kode_syntax::{keywords::KeywordProvider, registry::GrammarRegistry};
    for (marked, label, inserted) in [
        (
            "class Box { val café: Int = 0 }\nfun use(box: Box?) { box?.ca/*caret*/ + 12 }\n",
            "café",
            "box?.café + 12",
        ),
        (
            "package demo\nimport demo.Cu/*caret*/\nclass Customer\n",
            "Customer",
            "import demo.Customer\n",
        ),
    ] {
        let caret = marked.find("/*caret*/").unwrap();
        let source = marked.replace("/*caret*/", "");
        let mut harness = Harness::new(&source, caret);
        let delta = KotlinSemanticAdapter::new(&GrammarRegistry::compiled())
            .unwrap()
            .extract(
                "Native.kt",
                &harness.doc.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new(),
            )
            .unwrap();
        let snapshot = SemanticSnapshot::new(
            1,
            SemanticResolver::resolve([delta], &CancellationToken::new())
                .unwrap()
                .project,
        )
        .unwrap();
        let context = CompletionContext::at(
            SemanticIds::file("Native.kt"),
            "kotlin",
            &source,
            ByteOffset(caret),
        )
        .unwrap();
        let candidates = CompletionEngine::native(Arc::new(KeywordProvider::compiled().unwrap()))
            .complete(&context, &snapshot, 50, &CancellationToken::new())
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].label, label);
        let provider = Provider(candidates.iter().map(|c| c.item(&context)).collect());
        harness.open(None, Some(&provider));
        harness.render();
        assert!(
            harness
                .popup
                .accept(&mut harness.doc, &mut harness.input.state)
                .unwrap()
        );
        assert!(harness.doc.snapshot().text().contains(inserted));
        assert!(harness.popup.entries.is_empty());
        assert!(harness.doc.undo(&mut harness.input.state.caret).unwrap());
        assert_eq!(harness.doc.snapshot().text().as_ref(), source);
    }
}
