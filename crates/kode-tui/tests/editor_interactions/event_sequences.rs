use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, LineIndex, Result, Selection, TextRange,
    Utf16Column, WorkspaceGeneration,
    commands::EditorCommand,
    document::{Document, Utf16Position},
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget,
    },
};
use kode_platform::clipboard::{ClipboardService, NativeClipboard};
use kode_tui::{
    editor::host::{
        EditorDocuments, EditorView, EditorWork, HostDescriptor, HostRole, QueuedEditorHost,
    },
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    widgets::dialog::{DialogAction, TextDialog},
};
use ratatui::layout::Rect;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct ClockZero;
impl kode_platform::jobs::Clock for ClockZero {
    fn now_millis(&self) -> u64 {
        0
    }
}

struct ReplayClock(Arc<AtomicU64>);
impl kode_platform::jobs::Clock for ReplayClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct HeadlessClipboard;
impl NativeClipboard for HeadlessClipboard {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        Err("clipboard is unavailable in the replay".into())
    }

    fn set_text(&mut self, _: &str) -> std::result::Result<(), String> {
        Err("clipboard is unavailable in the replay".into())
    }
}

struct ReplayHostCompletion;
impl kode_tui::editor::popups::completion::HostCompletionProvider for ReplayHostCompletion {
    fn suggestions(
        &self,
        _: &IntelligenceRequest,
        prefix: &str,
    ) -> Result<Vec<kode_tui::editor::popups::completion::HostSuggestion>> {
        assert_eq!(prefix, "pri");
        Ok(vec![
            kode_tui::editor::popups::completion::HostSuggestion {
                label: "host-first".into(),
                insert_text: Some("first".into()),
                detail: Some("host detail".into()),
            },
            kode_tui::editor::popups::completion::HostSuggestion {
                label: "shared".into(),
                insert_text: None,
                detail: Some("host wins".into()),
            },
        ])
    }
}

struct ReplayReceiverCompletion;
impl kode_tui::editor::popups::completion::HostCompletionProvider for ReplayReceiverCompletion {
    fn suggestions(
        &self,
        _: &IntelligenceRequest,
        prefix: &str,
    ) -> Result<Vec<kode_tui::editor::popups::completion::HostSuggestion>> {
        assert_eq!(prefix, "pr");
        Ok(vec![kode_tui::editor::popups::completion::HostSuggestion {
            label: "property".into(),
            insert_text: Some("property".into()),
            detail: Some("receiver completion".into()),
        }])
    }
}

struct ReplayIntelligenceCompletion;
impl IntelligenceProvider for ReplayIntelligenceCompletion {
    fn definitions(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Ok(vec![])
    }

    fn references(&self, _: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        Ok(vec![])
    }

    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        let replacement_range = TextRange::new(ByteOffset(0), ByteOffset(3))?;
        let mut entries = vec![CompletionItem {
            label: "shared".into(),
            insert_text: "provider duplicate".into(),
            replacement_range,
            detail: Some("provider duplicate detail".into()),
            source: "intelligence".into(),
            kind: None,
        }];
        entries.extend((0..80).map(|index| CompletionItem {
            label: format!("provider-{index:02}"),
            insert_text: format!("insert-{index:02}"),
            replacement_range,
            detail: Some(format!("provider detail {index:02}")),
            source: "intelligence".into(),
            kind: None,
        }));
        Ok(entries)
    }

    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![])
    }
}

struct ReplayReferences(Vec<kode_core::intelligence::NavigationTarget>);
impl IntelligenceProvider for ReplayReferences {
    fn definitions(
        &self,
        _: &IntelligenceRequest,
    ) -> Result<Vec<kode_core::intelligence::NavigationTarget>> {
        Ok(vec![])
    }

    fn references(
        &self,
        _: &IntelligenceRequest,
    ) -> Result<Vec<kode_core::intelligence::NavigationTarget>> {
        Ok(self.0.clone())
    }

    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        Ok(vec![])
    }

    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![])
    }
}

struct ReplayDefinitions(Vec<kode_core::intelligence::NavigationTarget>);
impl IntelligenceProvider for ReplayDefinitions {
    fn definitions(
        &self,
        _: &IntelligenceRequest,
    ) -> Result<Vec<kode_core::intelligence::NavigationTarget>> {
        Ok(self.0.clone())
    }

    fn references(
        &self,
        _: &IntelligenceRequest,
    ) -> Result<Vec<kode_core::intelligence::NavigationTarget>> {
        Ok(vec![])
    }

    fn completions(&self, _: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        Ok(vec![])
    }

    fn highlights(&self, _: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![])
    }
}

struct ReplayNavigationSource(kode_core::document::TextSnapshot);
impl kode_tui::editor::popups::definition::PreviewSourceReader for ReplayNavigationSource {
    fn read(
        &self,
        _: &kode_core::intelligence::NavigationTarget,
        _: kode_core::JobTag,
    ) -> Result<kode_core::document::TextSnapshot> {
        Ok(self.0.clone())
    }
}

struct ReplayMissingNavigationSource;
impl kode_tui::editor::popups::definition::PreviewSourceReader for ReplayMissingNavigationSource {
    fn read(
        &self,
        target: &kode_core::intelligence::NavigationTarget,
        _: kode_core::JobTag,
    ) -> Result<kode_core::document::TextSnapshot> {
        Err(DomainError::MissingResource(
            target.path.display().to_string(),
        ))
    }
}

struct ReplayPreviewHighlighter;
impl kode_tui::editor::popups::definition::PreviewHighlighter for ReplayPreviewHighlighter {
    fn highlights(&self, _: &kode_core::document::TextSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(vec![HighlightSpan {
            range: TextRange::new(ByteOffset(0), ByteOffset(3))?,
            class: "semantic.keyword".into(),
        }])
    }
}

fn open_host(
    role: HostRole,
    text: &str,
    key: &str,
) -> (EditorDocuments, EditorView<QueuedEditorHost>, FocusState) {
    open_host_with_clipboard(
        role,
        text,
        key,
        Arc::new(ClipboardService::new(Box::new(HeadlessClipboard))),
    )
}

fn open_host_with_clipboard(
    role: HostRole,
    text: &str,
    key: &str,
    clipboard: Arc<ClipboardService>,
) -> (EditorDocuments, EditorView<QueuedEditorHost>, FocusState) {
    open_host_with_clock(role, text, key, clipboard, Arc::new(ClockZero))
}

fn open_host_with_clock(
    role: HostRole,
    text: &str,
    key: &str,
    clipboard: Arc<ClipboardService>,
    clock: Arc<dyn kode_platform::jobs::Clock>,
) -> (EditorDocuments, EditorView<QueuedEditorHost>, FocusState) {
    let descriptor = HostDescriptor::new(
        role,
        "editor-interaction replay".into(),
        key.into(),
        key.into(),
        PathBuf::from("/fixture"),
    );
    let mut documents = EditorDocuments::default();
    let id = if matches!(role, HostRole::File | HostRole::ProjectSearch) {
        documents.insert(
            Document::new(
                DocumentId::fresh().unwrap(),
                DocumentOrigin::File(PathBuf::from(format!("/fixture/{key}"))),
                Some("text".into()),
                text,
            ),
            &descriptor,
            WorkspaceGeneration(1),
            None,
        )
    } else {
        documents.create_virtual(&descriptor, WorkspaceGeneration(1), text)
    }
    .unwrap();
    let view = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard,
        clock,
    )
    .unwrap();
    let focus = FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    };
    (documents, view, focus)
}

fn open_file(text: &str, key: &str) -> (EditorDocuments, EditorView<QueuedEditorHost>, FocusState) {
    open_host(HostRole::File, text, key)
}

fn open_file_with_language(
    text: &str,
    language: &str,
    key: &str,
) -> (EditorDocuments, EditorView<QueuedEditorHost>, FocusState) {
    let descriptor = HostDescriptor::new(
        HostRole::File,
        "editor-interaction replay".into(),
        key.into(),
        key.into(),
        PathBuf::from("/fixture"),
    );
    let mut documents = EditorDocuments::default();
    let id = documents
        .insert(
            Document::new(
                DocumentId::fresh().unwrap(),
                DocumentOrigin::File(PathBuf::from(format!("/fixture/{key}"))),
                Some(language.into()),
                text,
            ),
            &descriptor,
            WorkspaceGeneration(1),
            None,
        )
        .unwrap();
    let view = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        Arc::new(ClipboardService::new(Box::new(HeadlessClipboard))),
        Arc::new(ClockZero),
    )
    .unwrap();
    let focus = FocusState {
        focused: Some(view.input.id),
        ..Default::default()
    };
    (documents, view, focus)
}

fn event(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}

#[test]
fn empty_editor_renders_on_every_host_and_shifted_typing_preserves_case() {
    use kode_tui::theme::Theme;
    use ratatui::buffer::Buffer;

    let roles = [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ];
    let area = Rect::new(0, 0, 40, 10);
    let theme = Theme::compiled();
    for role in roles {
        let (documents, mut view, focus) = open_host(role, "", "empty-host.txt");
        let document = documents.document(view.document_id()).unwrap();
        let geometry = view.geometry(&documents, area).unwrap();
        let layout = view
            .input
            .layout(
                document,
                kode_tui::editor::input::InputViewport {
                    geometry,
                    folds: &[],
                },
            )
            .unwrap();
        assert_eq!(document.snapshot().len_lines(), 1, "{role:?}");
        assert_eq!(layout.rows.len(), 1, "{role:?}");
        assert_eq!(view.input.state.caret.cursor(), ByteOffset(0), "{role:?}");
        let mut buffer = Buffer::empty(area);
        view.render(&documents, &focus, area, &mut buffer, &theme, &[])
            .unwrap();
    }

    let editable = [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ];
    let shifted_uppercase = InputEvent::Key(KeyInput {
        key: Key::Character('A'),
        modifiers: Modifiers {
            shift: true,
            ..Default::default()
        },
        repeat: false,
    });
    for role in editable {
        let (mut documents, mut view, mut focus) = open_host(role, "", "shifted.txt");
        let document = documents.document(view.document_id()).unwrap();
        let revision = document.revision();
        assert!(
            view.dispatch(&shifted_uppercase, &mut documents, &mut focus, area)
                .unwrap()
        );
        let document = documents.document(view.document_id()).unwrap();
        assert_eq!(document.snapshot().text().as_ref(), "A", "{role:?}");
        assert_eq!(view.input.state.caret.cursor(), ByteOffset(1), "{role:?}");
        assert_ne!(document.revision(), revision, "{role:?}");
        assert!(document.is_dirty(), "{role:?}");
    }
}

#[test]
fn cloned_documents_keep_views_histories_and_worker_snapshots_independent() {
    let initial = "base";
    let (mut first_documents, mut first_view, mut first_focus) =
        open_file(initial, "snapshot-clone.txt");
    let id = first_view.document_id();
    let worker_snapshot = first_documents.document(id).unwrap().snapshot();
    let cloned_document = first_documents.document(id).unwrap().clone();

    let descriptor = HostDescriptor::new(
        HostRole::File,
        "editor-interaction replay".into(),
        "snapshot-clone-copy.txt".into(),
        "snapshot clone copy".into(),
        PathBuf::from("/fixture"),
    );
    let mut second_documents = EditorDocuments::default();
    let cloned_id = second_documents
        .insert(cloned_document, &descriptor, WorkspaceGeneration(1), None)
        .unwrap();
    assert_eq!(cloned_id, id);
    let clipboard = Arc::new(ClipboardService::new(Box::new(HeadlessClipboard)));
    let mut second_view = EditorView::new(
        cloned_id,
        &second_documents,
        QueuedEditorHost::new(descriptor),
        WorkspaceGeneration(1),
        clipboard,
        Arc::new(ClockZero),
    )
    .unwrap();
    let mut second_focus = FocusState {
        focused: Some(second_view.input.id),
        ..Default::default()
    };
    second_view
        .input
        .state
        .caret
        .move_to(&worker_snapshot, ByteOffset(2), false)
        .unwrap();

    let area = Rect::new(0, 0, 40, 10);
    assert!(
        first_view
            .dispatch(
                &event(Key::Character('A')),
                &mut first_documents,
                &mut first_focus,
                area,
            )
            .unwrap()
    );
    assert!(
        second_view
            .dispatch(
                &event(Key::Character('B')),
                &mut second_documents,
                &mut second_focus,
                area,
            )
            .unwrap()
    );
    assert_eq!(source(&first_documents, &first_view), "Abase");
    assert_eq!(source(&second_documents, &second_view), "baBse");
    assert_eq!(worker_snapshot.text().as_ref(), initial);
    assert_eq!(
        worker_snapshot.to_service_snapshot().revision,
        kode_core::Revision(0)
    );

    first_view
        .command(EditorCommand::Undo, &mut first_documents, area)
        .unwrap();
    assert_eq!(source(&first_documents, &first_view), initial);
    assert_eq!(source(&second_documents, &second_view), "baBse");
    second_view
        .command(EditorCommand::Undo, &mut second_documents, area)
        .unwrap();
    assert_eq!(source(&second_documents, &second_view), initial);
    first_view
        .command(EditorCommand::Redo, &mut first_documents, area)
        .unwrap();
    second_view
        .command(EditorCommand::Redo, &mut second_documents, area)
        .unwrap();
    assert_eq!(source(&first_documents, &first_view), "Abase");
    assert_eq!(source(&second_documents, &second_view), "baBse");
    assert_eq!(worker_snapshot.text().as_ref(), initial);
}

#[test]
fn wrapped_page_navigation_moves_visual_rows_and_keeps_selection_column() {
    let initial = "\t".repeat(100);
    let (mut documents, mut view, mut focus) = open_file(&initial, "wrapped-pages.txt");
    let area = Rect::new(0, 0, 23, 4);
    let page_down = InputEvent::Key(KeyInput {
        key: Key::PageDown,
        modifiers: Modifiers {
            shift: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&page_down, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.caret.selection.anchor, ByteOffset(0));
    assert_eq!(view.input.state.caret.selection.head, ByteOffset(15));
    assert_eq!(view.input.state.scroll_rows, 3);
    assert_eq!(
        view.input
            .state
            .caret
            .selected_text(&documents.document(view.document_id()).unwrap().snapshot())
            .unwrap(),
        "\t".repeat(15)
    );

    assert!(
        view.dispatch(&event(Key::PageUp), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.caret.cursor(), ByteOffset(0));
    assert!(view.input.state.caret.selection.is_empty());
    assert_eq!(view.input.state.scroll_rows, 0);
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn wrapped_wheel_scroll_uses_alt_multiplier_and_navigation_reveals_caret() {
    let initial = (0..80)
        .map(|line| format!("line {line:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut documents, mut view, mut focus) = open_file(&initial, "wheel-scroll.txt");
    let area = Rect::new(0, 0, 30, 8);
    let geometry = view.geometry(&documents, area).unwrap();
    let scroll = |vertical, alt| {
        InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Scroll {
                horizontal: 0,
                vertical,
            },
            modifiers: Modifiers {
                alt,
                ..Default::default()
            },
        })
    };
    assert!(
        view.dispatch(&scroll(1, false), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.scroll_rows, 3);
    assert_eq!(view.input.state.caret.cursor(), ByteOffset(0));
    assert!(
        view.dispatch(&scroll(1, true), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.scroll_rows, 12);
    assert_eq!(view.input.state.caret.cursor(), ByteOffset(0));

    assert!(
        view.dispatch(&event(Key::Right), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.caret.cursor(), ByteOffset(1));
    assert_eq!(view.input.state.scroll_rows, 0);

    assert!(
        view.dispatch(&scroll(i16::MAX, false), &mut documents, &mut focus, area)
            .unwrap()
    );
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let max_scroll = layout
        .rows
        .len()
        .saturating_sub(geometry.text.height as usize);
    assert_eq!(view.input.state.scroll_rows, max_scroll);
    assert!(
        view.dispatch(&scroll(i16::MIN, false), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.scroll_rows, 0);
    assert_eq!(source(&documents, &view), initial);

    for _ in 0..79 {
        assert!(
            view.dispatch(&event(Key::Down), &mut documents, &mut focus, area)
                .unwrap()
        );
    }
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let caret_row = layout
        .position_to_cell(view.input.state.caret.cursor())
        .unwrap()
        .0;
    assert!(caret_row >= view.input.state.scroll_rows);
    assert!(caret_row < view.input.state.scroll_rows + geometry.text.height as usize);
    assert!(
        view.dispatch(
            &event(Key::Character('!')),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(
        source(&documents, &view),
        initial.replacen("line 79", "l!ine 79", 1)
    );
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let caret_row = layout
        .position_to_cell(view.input.state.caret.cursor())
        .unwrap()
        .0;
    assert!(caret_row >= view.input.state.scroll_rows);
    assert!(caret_row < view.input.state.scroll_rows + geometry.text.height as usize);
}

#[test]
fn search_open_and_close_recompute_editor_geometry_without_coordinate_drift() {
    let initial = "first line\nsecond line";
    let (mut documents, mut view, mut focus) = open_file(initial, "search-geometry.txt");
    let area = Rect::new(4, 3, 36, 12);
    let before = view.geometry(&documents, area).unwrap();
    assert_eq!(before.search.height, 0);
    let open_search = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&open_search, &mut documents, &mut focus, area)
            .unwrap()
    );
    let searching = view.geometry(&documents, area).unwrap();
    assert_eq!(searching.search.height, 2);
    assert_eq!(searching.text.height, before.text.height - 2);
    assert!(view.input.state.search_has_focus);

    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    let after = view.geometry(&documents, area).unwrap();
    assert_eq!(after.header, before.header);
    assert_eq!(after.gutter, before.gutter);
    assert_eq!(after.minimap, before.minimap);
    assert_eq!(after.search, before.search);
    assert_eq!(after.text, before.text);
    assert!(!view.input.state.search_has_focus);
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn ctrl_f_prefills_escaped_selection_and_preserves_existing_query_without_selection() {
    let initial = "A.*B and other";
    let (mut documents, mut view, mut focus) = open_file(initial, "find-prefill.txt");
    let area = Rect::new(0, 0, 50, 12);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    view.input.state.caret.selection = Selection {
        anchor: ByteOffset(0),
        head: ByteOffset(4),
    };
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, r"A\.\*B");

    view.input.state.caret.selection = Selection {
        anchor: ByteOffset(0),
        head: ByteOffset(0),
    };
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, r"A\.\*B");
    view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
        .unwrap();
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, r"A\.\*B");
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn search_fields_keep_uppercase_keyboard_and_mouse_focus_routing() {
    use kode_tui::editor::search_bar::SearchField;

    let initial = "source stays unchanged";
    let (mut documents, mut view, mut focus) = open_file(initial, "search-fields.txt");
    let area = Rect::new(0, 0, 50, 12);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    let search_area = view.geometry(&documents, area).unwrap().search;
    let geometry = view.search.geometry(search_area);
    assert!(geometry.find.right() <= search_area.right());
    assert!(geometry.replace.right() <= search_area.right());
    assert_eq!(view.search.focused, SearchField::Find);

    let shifted = |character| {
        InputEvent::Key(KeyInput {
            key: Key::Character(character),
            modifiers: Modifiers {
                shift: true,
                ..Default::default()
            },
            repeat: false,
        })
    };
    for character in ['A', 'B'] {
        assert!(
            view.dispatch(&shifted(character), &mut documents, &mut focus, area)
                .unwrap()
        );
    }
    assert_eq!(view.search.find.value, "AB");
    assert!(
        view.dispatch(&event(Key::Left), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Backspace), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, "B");
    assert!(
        view.dispatch(&event(Key::Home), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Delete), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, "");
    assert!(
        view.dispatch(
            &event(Key::Character('C')),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert_eq!(view.search.find.value, "C");

    assert!(
        view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.focused, SearchField::Replace);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: geometry.replace.x,
                y: geometry.replace.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.search.focused, SearchField::Replace);
    assert!(
        view.dispatch(&shifted('X'), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.replace.value, "X");
    assert!(
        view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.focused, SearchField::Find);
    assert_eq!(view.search.find.value, "C");
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn find_next_waits_for_current_scan_and_cycles_through_matches() {
    let initial = "one two one";
    let (mut documents, mut view, mut focus) = open_file(initial, "find-next.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    for character in ['o', 'n', 'e'] {
        assert!(
            view.dispatch(
                &event(Key::Character(character)),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
        );
    }
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(query, "one");
    assert!(
        view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.input.state.caret.selection.is_empty());

    let result = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, result, &mut documents, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "1/2");
    assert_eq!(
        view.input.state.caret.selection,
        Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(3),
        }
    );
    assert!(
        view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "2/2");
    assert_eq!(
        view.input.state.caret.selection,
        Selection {
            anchor: ByteOffset(8),
            head: ByteOffset(11),
        }
    );
    assert!(
        view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "1/2");
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn clicking_search_close_restores_editor_and_keeps_query_for_reopen() {
    let initial = "draft";
    let (mut documents, mut view, mut focus) = open_file(initial, "search-close.txt");
    let area = Rect::new(0, 0, 40, 10);
    let before = view.geometry(&documents, area).unwrap();
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    for character in ['k', 'e', 'e', 'p'] {
        assert!(
            view.dispatch(
                &event(Key::Character(character)),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
        );
    }
    let search_area = view.geometry(&documents, area).unwrap().search;
    let close = view.search.geometry(search_area).close;
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: close.x,
                y: close.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(!view.search.visible);
    assert!(!view.input.state.search_has_focus);
    assert_eq!(view.geometry(&documents, area).unwrap().text, before.text);
    assert_eq!(view.search.find.value, "keep");
    assert!(
        view.dispatch(
            &event(Key::Character('X')),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "Xdraft");

    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, "keep");
    assert!(view.search.visible);
}

#[test]
fn regex_replace_all_uses_capture_groups_as_one_undo_transaction() {
    let initial = "one two one";
    let (mut documents, mut view, mut focus) = open_file(initial, "regex-replace.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("(one)".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("[$1]".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(query, "(one)");
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Key(KeyInput {
                key: Key::Character('r'),
                modifiers: Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                },
                repeat: false,
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "[one] two [one]");
    assert_eq!(
        documents
            .document(view.document_id())
            .unwrap()
            .history_depth(),
        (1, 0)
    );
    view.command(EditorCommand::Undo, &mut documents, area)
        .unwrap();
    assert_eq!(source(&documents, &view), initial);
    assert_eq!(
        documents
            .document(view.document_id())
            .unwrap()
            .history_depth(),
        (0, 1)
    );
    view.command(EditorCommand::Redo, &mut documents, area)
        .unwrap();
    assert_eq!(source(&documents, &view), "[one] two [one]");
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(query, "(one)");
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "0/2");
    let current = documents.document(view.document_id()).unwrap().snapshot();
    let report = view.search.results.report.as_ref().unwrap();
    assert!(report.is_current(&current));
    assert_eq!(report.matches.len(), 2);
}

#[test]
fn regex_replace_current_changes_only_the_active_match_as_one_undo_unit() {
    let initial = "one two one";
    let (mut documents, mut view, mut focus) = open_file(initial, "regex-replace-current.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("(one)".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "1/2");
    assert!(
        view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("$1!".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Key(KeyInput {
                key: Key::Character('r'),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "one! two one");
    assert_eq!(
        documents
            .document(view.document_id())
            .unwrap()
            .history_depth(),
        (1, 0)
    );
    view.command(EditorCommand::Undo, &mut documents, area)
        .unwrap();
    assert_eq!(source(&documents, &view), initial);
    view.command(EditorCommand::Redo, &mut documents, area)
        .unwrap();
    assert_eq!(source(&documents, &view), "one! two one");
}

#[test]
fn stale_search_scan_is_rejected_after_document_edit() {
    let (mut documents, mut view, mut focus) = open_file("one two one", "stale-search.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("one".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );

    let (stale_tag, stale_snapshot, stale_query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(stale_query, "one");
    let stale_report = kode_core::editor::search::scan(&stale_snapshot, &stale_query, || false);

    view.programmatic_replace(TextRange::caret(ByteOffset(0)), "X", &mut documents, area)
        .unwrap();
    assert_eq!(source(&documents, &view), "Xone two one");
    assert!(
        !view
            .complete_search(stale_tag, stale_report, &mut documents, area)
            .unwrap()
    );
    assert!(view.search.results.report.is_none());

    let (current_tag, current_snapshot, current_query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(current_query, "one");
    let current_report =
        kode_core::editor::search::scan(&current_snapshot, &current_query, || false);
    assert!(
        view.complete_search(current_tag, current_report, &mut documents, area)
            .unwrap()
    );
    let matches = &view.search.results.report.as_ref().unwrap().matches;
    assert_eq!(matches.len(), 2);
    assert_eq!(
        matches[0].range,
        TextRange::new(ByteOffset(1), ByteOffset(4)).unwrap()
    );
    assert_eq!(
        matches[1].range,
        TextRange::new(ByteOffset(9), ByteOffset(12)).unwrap()
    );
}

#[test]
fn invalid_regex_is_visible_and_editing_recovers_without_reopening_find() {
    let initial = "one two";
    let (mut documents, mut view, mut focus) = open_file(initial, "invalid-regex.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("[".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    assert_eq!(view.search.label(), "regex error");
    assert!(
        view.search
            .error
            .as_ref()
            .is_some_and(|error| !error.is_empty())
    );
    assert_eq!(source(&documents, &view), initial);

    assert!(
        view.dispatch(&event(Key::Home), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Delete), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.search.find.value, "");
    assert!(
        view.dispatch(
            &InputEvent::Paste("one".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(query, "one");
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    assert!(view.search.error.is_none());
    assert_eq!(view.search.label(), "0/1");
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn empty_zero_width_and_no_match_queries_are_finite_and_never_replace_source() {
    let initial = "abc";
    let (mut documents, mut view, mut focus) = open_file(initial, "empty-search.txt");
    let area = Rect::new(0, 0, 40, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    let ctrl_a = InputEvent::Key(KeyInput {
        key: Key::Character('a'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("^".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("X".into()),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );

    for query in ["^", "", "missing"] {
        if view.search.focused == kode_tui::editor::search_bar::SearchField::Replace {
            assert!(
                view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
                    .unwrap()
            );
        }
        assert!(
            view.dispatch(&ctrl_a, &mut documents, &mut focus, area)
                .unwrap()
        );
        assert!(
            view.dispatch(
                &InputEvent::Paste(query.into()),
                &mut documents,
                &mut focus,
                area
            )
            .unwrap()
        );
        let (tag, snapshot, latest_query) = view
            .take_work()
            .into_iter()
            .filter_map(|work| match work {
                EditorWork::Search {
                    tag,
                    snapshot,
                    query,
                } => Some((tag, snapshot, query)),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(latest_query, query);
        let report = kode_core::editor::search::scan(&snapshot, &latest_query, || false);
        assert!(
            view.complete_search(tag, report, &mut documents, area)
                .unwrap()
        );
        assert_eq!(view.search.label(), "no matches");
        assert!(
            view.search
                .results
                .report
                .as_ref()
                .unwrap()
                .matches
                .is_empty()
        );

        if view.search.focused == kode_tui::editor::search_bar::SearchField::Find {
            assert!(
                view.dispatch(&event(Key::Tab), &mut documents, &mut focus, area)
                    .unwrap()
            );
        }
        assert!(
            view.dispatch(
                &InputEvent::Key(KeyInput {
                    key: Key::Character('r'),
                    modifiers: Modifiers {
                        ctrl: true,
                        shift: true,
                        ..Default::default()
                    },
                    repeat: false,
                }),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
        );
        assert_eq!(source(&documents, &view), initial);
        assert_eq!(
            documents
                .document(view.document_id())
                .unwrap()
                .history_depth(),
            (0, 0)
        );
    }
}

#[test]
fn multiline_search_highlights_exact_cells_across_wrapped_logical_lines() {
    use kode_tui::theme::Theme;
    use ratatui::buffer::Buffer;

    let initial = format!("{}\n{}", "a".repeat(25), "b".repeat(25));
    let (mut documents, mut view, mut focus) = open_file(&initial, "wrapped-multiline-search.txt");
    let area = Rect::new(0, 0, 15, 10);
    let ctrl_f = InputEvent::Key(KeyInput {
        key: Key::Character('f'),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_f, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Paste("aaa\\nbbb".into()),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let (tag, snapshot, query) = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            EditorWork::Search {
                tag,
                snapshot,
                query,
            } => Some((tag, snapshot, query)),
            _ => None,
        })
        .next_back()
        .unwrap();
    assert_eq!(query, r"aaa\nbbb");
    let report = kode_core::editor::search::scan(&snapshot, &query, || false);
    assert!(
        view.complete_search(tag, report, &mut documents, area)
            .unwrap()
    );
    let report = view.search.results.report.as_ref().unwrap();
    assert_eq!(report.matches.len(), 1);
    assert_eq!(
        report.matches[0].range,
        TextRange {
            start: ByteOffset(22),
            end: ByteOffset(29),
        }
    );
    let theme = Theme::compiled();
    let mut buffer = Buffer::empty(area);
    view.render(&documents, &focus, area, &mut buffer, &theme, &[])
        .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    let layout = view
        .input
        .layout(
            documents.document(view.document_id()).unwrap(),
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    for byte in [22, 23, 24, 26, 27, 28] {
        let (row, column) = layout.position_to_cell(ByteOffset(byte)).unwrap();
        assert_eq!(
            buffer[(
                geometry.text.x + column as u16,
                geometry.text.y + row as u16
            )]
                .bg,
            theme.style("code-search-match").bg.unwrap(),
            "source byte {byte}"
        );
    }
    assert_eq!(source(&documents, &view), initial);
}

fn source(documents: &EditorDocuments, view: &EditorView<QueuedEditorHost>) -> String {
    documents
        .document(view.document_id())
        .unwrap()
        .snapshot()
        .text()
        .to_string()
}

fn definition_geometry(
    documents: &EditorDocuments,
    view: &mut EditorView<QueuedEditorHost>,
    area: Rect,
) -> kode_tui::editor::popups::definition::DefinitionGeometry {
    let geometry = view.geometry(documents, area).unwrap();
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    view.navigation
        .definition
        .as_ref()
        .unwrap()
        .geometry(
            kode_tui::editor::input::HookContext {
                geometry,
                layout: &layout,
            },
            &view.input.state,
        )
        .unwrap()
}

fn complete_hover_preview_through_host(
    documents: &mut EditorDocuments,
    view: &mut EditorView<QueuedEditorHost>,
    focus: &mut FocusState,
    now: &AtomicU64,
    area: Rect,
) {
    let geometry = view.geometry(documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }),
        documents,
        focus,
        area,
    )
    .unwrap();
    now.fetch_add(500, Ordering::SeqCst);
    view.tick(documents, focus, area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .expect("hover timer schedules a navigation job");
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/preview-source.c")),
            Some("c".into()),
            "int foo(void);\n",
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        Some(&ReplayPreviewHighlighter),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), documents, true)
            .unwrap()
    );
}

fn complete_usage_popup_through_host(
    documents: &mut EditorDocuments,
    view: &mut EditorView<QueuedEditorHost>,
    focus: &mut FocusState,
    area: Rect,
) {
    view.navigation.enabled = true;
    view.navigation
        .set_declaration_spans(
            &documents.document(view.document_id()).unwrap().snapshot(),
            vec![TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap()],
        )
        .unwrap();
    let geometry = view.geometry(documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Down(MouseButton::Left),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
        }),
        documents,
        focus,
        area,
    )
    .unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .expect("declaration click schedules reference navigation");
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/uses.c")),
            Some("c".into()),
            "foo();\n",
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        None,
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), documents, true)
            .unwrap()
    );
}

fn replay_newline_aliases(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let expected = sequence["expected_text"].as_str().unwrap();
    let cursor = sequence["cursor_byte"].as_u64().unwrap() as usize;
    let area = Rect::new(0, 0, 80, 24);

    // `terminal_input` verifies Enter, Return, and CR normalize to this shared
    // event. Replay that event through every editable host and enforce the
    // mutation guard on every read-only host.
    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let key = format!("newline-aliases-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        let snapshot = documents.document(view.document_id()).unwrap().snapshot();
        view.input
            .state
            .caret
            .move_to(&snapshot, ByteOffset(cursor), false)
            .unwrap();
        for _spelling in sequence["events"].as_array().unwrap() {
            assert!(
                view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
                    .unwrap(),
                "{role:?} did not consume Enter"
            );
        }
        assert_eq!(source(&documents, &view), expected, "{role:?}");
        assert_eq!(
            view.input.state.caret.cursor().0,
            expected.len(),
            "{role:?}"
        );
    }

    for role in [
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("readonly-newline-aliases-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for _ in sequence["events"].as_array().unwrap() {
            assert!(
                matches!(
                    view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area),
                    Err(kode_core::DomainError::ReadOnly)
                ),
                "{role:?} accepted Enter"
            );
        }
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(
            !documents.document(view.document_id()).unwrap().is_dirty(),
            "{role:?}"
        );
    }
}

fn replay_boundary_deletes(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let area = Rect::new(0, 0, 80, 24);
    let selection = Selection {
        anchor: ByteOffset(sequence["expected_selection"]["anchor"].as_u64().unwrap() as usize),
        head: ByteOffset(sequence["expected_selection"]["head"].as_u64().unwrap() as usize),
    };

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let key = format!("boundary-deletes-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for step in sequence["events"].as_array().unwrap() {
            if let Some(position) = step["move_to"].as_u64() {
                view.command(
                    EditorCommand::MoveTo {
                        position: ByteOffset(position as usize),
                        extend: false,
                    },
                    &mut documents,
                    area,
                )
                .unwrap();
            } else if let Some(key) = step["key"].as_str() {
                let (input, expected) = match key {
                    "backspace" => (event(Key::Backspace), step["expected_text"].as_str()),
                    "delete" => (event(Key::Delete), step["expected_text"].as_str()),
                    "shift-left" => (
                        InputEvent::Key(KeyInput {
                            key: Key::Left,
                            modifiers: Modifiers {
                                shift: true,
                                ..Default::default()
                            },
                            repeat: false,
                        }),
                        None,
                    ),
                    other => panic!("unknown boundary-delete key: {other}"),
                };
                assert!(
                    view.dispatch(&input, &mut documents, &mut focus, area)
                        .unwrap()
                );
                if let Some(expected) = expected {
                    assert_eq!(source(&documents, &view), expected, "{role:?} {key}");
                    assert_eq!(
                        documents
                            .document(view.document_id())
                            .unwrap()
                            .history_depth(),
                        (1, 0),
                        "{role:?} {key} should be one undo step"
                    );
                }
            } else if step["command"] == "undo" {
                view.command(EditorCommand::Undo, &mut documents, area)
                    .unwrap();
                assert_eq!(source(&documents, &view), step["expected_text"]);
            } else {
                panic!("unknown boundary-delete operation: {step}");
            }
        }
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert_eq!(view.input.state.caret.selection, selection, "{role:?}");
    }

    for role in [
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("readonly-boundary-deletes-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for (position, key) in [(3, Key::Backspace), (1, Key::Delete)] {
            view.command(
                EditorCommand::MoveTo {
                    position: ByteOffset(position),
                    extend: false,
                },
                &mut documents,
                area,
            )
            .unwrap();
            assert!(
                matches!(
                    view.dispatch(&event(key), &mut documents, &mut focus, area),
                    Err(kode_core::DomainError::ReadOnly)
                ),
                "{role:?} accepted {key:?}"
            );
            assert_eq!(source(&documents, &view), initial, "{role:?}");
            assert!(!documents.document(view.document_id()).unwrap().is_dirty());
        }
    }
}

fn replay_logical_movement(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let area = Rect::new(0, 0, 80, 24);

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("logical-movement-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for step in sequence["events"].as_array().unwrap() {
            if let Some(position) = step["move_to"].as_u64() {
                view.command(
                    EditorCommand::MoveTo {
                        position: ByteOffset(position as usize),
                        extend: false,
                    },
                    &mut documents,
                    area,
                )
                .unwrap();
            } else if let Some(name) = step["key"].as_str() {
                let key = match name {
                    "left" => Key::Left,
                    "right" => Key::Right,
                    "up" => Key::Up,
                    "down" => Key::Down,
                    other => panic!("unknown logical movement key: {other}"),
                };
                assert!(
                    view.dispatch(&event(key), &mut documents, &mut focus, area)
                        .unwrap()
                );
                assert_eq!(
                    view.input.state.caret.cursor().0,
                    step["cursor_byte"].as_u64().unwrap() as usize,
                    "{role:?} {name}"
                );
            } else {
                panic!("unknown logical movement operation: {step}");
            }
        }
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

fn replay_line_word_navigation(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let area = Rect::new(0, 0, 80, 24);

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("line-word-navigation-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for step in sequence["events"].as_array().unwrap() {
            if let Some(position) = step["move_to"].as_u64() {
                view.command(
                    EditorCommand::MoveTo {
                        position: ByteOffset(position as usize),
                        extend: false,
                    },
                    &mut documents,
                    area,
                )
                .unwrap();
                continue;
            }
            let name = step["key"].as_str().unwrap();
            let (key, modifiers) = match name {
                "home" => (Key::Home, Modifiers::default()),
                "end" => (Key::End, Modifiers::default()),
                "shift-end" => (
                    Key::End,
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                ),
                "ctrl-left" => (
                    Key::Left,
                    Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                "ctrl-right" => (
                    Key::Right,
                    Modifiers {
                        ctrl: true,
                        ..Default::default()
                    },
                ),
                other => panic!("unknown line/word navigation key: {other}"),
            };
            let input = InputEvent::Key(KeyInput {
                key,
                modifiers,
                repeat: false,
            });
            assert!(
                view.dispatch(&input, &mut documents, &mut focus, area)
                    .unwrap()
            );
            assert_eq!(
                view.input.state.caret.cursor().0,
                step["cursor_byte"].as_u64().unwrap() as usize,
                "{role:?} {name}"
            );
            if !step["selection"].is_null() {
                let expected = Selection {
                    anchor: ByteOffset(step["selection"]["anchor"].as_u64().unwrap() as usize),
                    head: ByteOffset(step["selection"]["head"].as_u64().unwrap() as usize),
                };
                assert_eq!(view.input.state.caret.selection, expected, "{role:?}");
            } else if name == "home" {
                assert_eq!(
                    view.input.state.caret.selection,
                    Selection {
                        anchor: ByteOffset(step["cursor_byte"].as_u64().unwrap() as usize),
                        head: ByteOffset(step["cursor_byte"].as_u64().unwrap() as usize),
                    },
                    "{role:?}"
                );
            }
        }
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

fn replay_source_scalar_movement(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let area = Rect::new(0, 0, 80, 24);

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("source-scalar-movement-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for step in sequence["events"].as_array().unwrap() {
            if let Some(position) = step["move_to"].as_u64() {
                view.command(
                    EditorCommand::MoveTo {
                        position: ByteOffset(position as usize),
                        extend: false,
                    },
                    &mut documents,
                    area,
                )
                .unwrap();
                continue;
            }
            let key = match step["key"].as_str().unwrap() {
                "left" => Key::Left,
                "right" => Key::Right,
                other => panic!("unknown scalar movement key: {other}"),
            };
            assert!(
                view.dispatch(&event(key), &mut documents, &mut focus, area)
                    .unwrap()
            );
            let position = ByteOffset(step["cursor_byte"].as_u64().unwrap() as usize);
            assert_eq!(view.input.state.caret.cursor(), position, "{role:?}");
            let geometry = view.geometry(&documents, area).unwrap();
            let document = documents.document(view.document_id()).unwrap();
            let layout = view
                .input
                .layout(
                    document,
                    kode_tui::editor::input::InputViewport {
                        geometry,
                        folds: &[],
                    },
                )
                .unwrap();
            assert!(
                layout.position_to_cell(position).is_ok(),
                "{role:?} {position:?}"
            );
        }
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

fn replay_select_all(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let expected = Selection {
        anchor: ByteOffset(sequence["expected_selection"]["anchor"].as_u64().unwrap() as usize),
        head: ByteOffset(sequence["expected_selection"]["head"].as_u64().unwrap() as usize),
    };
    let area = Rect::new(0, 0, 80, 24);

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("{}-{role:?}", sequence["name"].as_str().unwrap());
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for _ in sequence["events"].as_array().unwrap() {
            let input = InputEvent::Key(KeyInput {
                key: Key::Character('a'),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
            });
            assert!(
                view.dispatch(&input, &mut documents, &mut focus, area)
                    .unwrap()
            );
        }
        assert_eq!(view.input.state.caret.selection, expected, "{role:?}");
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

fn replay_history(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let area = Rect::new(0, 0, 80, 24);

    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let key = format!("{}-{role:?}", sequence["name"].as_str().unwrap());
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        for step in sequence["events"].as_array().unwrap() {
            if let Some(position) = step["move_to"].as_u64() {
                view.command(
                    EditorCommand::MoveTo {
                        position: ByteOffset(position as usize),
                        extend: false,
                    },
                    &mut documents,
                    area,
                )
                .unwrap();
            } else if let Some(insert) = step["insert_text"].as_str() {
                for ch in insert.chars() {
                    assert!(
                        view.dispatch(&event(Key::Character(ch)), &mut documents, &mut focus, area)
                            .unwrap()
                    );
                }
            } else {
                match step["command"].as_str().unwrap() {
                    "undo" => view
                        .command(EditorCommand::Undo, &mut documents, area)
                        .unwrap(),
                    "redo" => view
                        .command(EditorCommand::Redo, &mut documents, area)
                        .unwrap(),
                    other => panic!("unknown history command: {other}"),
                }
            }
            if let Some(expected) = step["expected_text"].as_str() {
                assert_eq!(source(&documents, &view), expected, "{role:?}");
                assert_eq!(
                    view.input.state.caret.cursor().0,
                    step["cursor_byte"].as_u64().unwrap() as usize,
                    "{role:?}"
                );
                let history = [
                    step["history"][0].as_u64().unwrap() as usize,
                    step["history"][1].as_u64().unwrap() as usize,
                ];
                assert_eq!(
                    documents
                        .document(view.document_id())
                        .unwrap()
                        .history_depth(),
                    (history[0], history[1]),
                    "{role:?}"
                );
            }
        }
        let expected = sequence["events"].as_array().unwrap().last().unwrap();
        assert_eq!(source(&documents, &view), expected["expected_text"]);
        assert_eq!(
            documents
                .document(view.document_id())
                .unwrap()
                .history_depth(),
            (
                expected["history"][0].as_u64().unwrap() as usize,
                expected["history"][1].as_u64().unwrap() as usize,
            )
        );
        assert_eq!(
            documents.document(view.document_id()).unwrap().is_dirty(),
            source(&documents, &view) != initial,
            "{role:?}"
        );
    }
}

fn replay_reverse_selection_paste(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let expected = sequence["expected_text"].as_str().unwrap();
    let selection = Selection {
        anchor: ByteOffset(sequence["selection"]["anchor"].as_u64().unwrap() as usize),
        head: ByteOffset(sequence["selection"]["head"].as_u64().unwrap() as usize),
    };
    let replacement = sequence["events"][0]["paste"].as_str().unwrap();
    let (mut documents, mut view, mut focus) = open_file(initial, "reverse-paste.txt");
    view.input.state.caret.selection = selection;
    let area = Rect::new(0, 0, 80, 24);

    assert!(
        view.dispatch(
            &InputEvent::Paste(replacement.into()),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), expected);
    for operation in sequence["events"].as_array().unwrap().iter().skip(1) {
        match operation.as_str().unwrap() {
            "undo" => {
                view.command(EditorCommand::Undo, &mut documents, area)
                    .unwrap();
                assert_eq!(source(&documents, &view), initial);
                assert_eq!(view.input.state.caret.selection, selection);
            }
            "redo" => {
                view.command(EditorCommand::Redo, &mut documents, area)
                    .unwrap();
                assert_eq!(source(&documents, &view), expected);
                assert_eq!(
                    view.input.state.caret.cursor(),
                    ByteOffset(selection.range().start.0 + replacement.len())
                );
            }
            other => panic!("unsupported fixture replay operation: {other}"),
        }
    }
    assert_eq!(source(&documents, &view), expected);
}

fn replay_popup_outside_click() {
    let (mut documents, mut view, mut focus) = open_file("", "popup-outside.txt");
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    let pending = view.completion.begin(&snapshot, &view.input.state).unwrap();
    assert!(
        view.completion
            .complete(
                pending.tag,
                Ok(vec![CompletionItem {
                    label: "candidate".into(),
                    insert_text: "candidate".into(),
                    detail: Some("fixture completion".into()),
                    kind: None,
                    replacement_range: TextRange {
                        start: ByteOffset(0),
                        end: ByteOffset(0),
                    },
                    source: "editor interaction fixture".into(),
                }]),
                &snapshot,
                &view.input.state,
                true,
            )
            .unwrap()
    );
    let before = view.input.state.caret.selection;
    let area = Rect::new(0, 0, 80, 24);
    let outside = InputEvent::Pointer(PointerInput {
        x: 79,
        y: 23,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&outside, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    assert_eq!(view.input.state.caret.selection, before);
    assert_eq!(source(&documents, &view), "");
}

#[test]
fn hover_preview_uses_unsaved_document_declaration_range_and_line_fallback() {
    let initial = format!(
        "foo\n{}",
        (0..30)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let now = Arc::new(AtomicU64::new(0));
    let (mut documents, mut view, mut focus) = open_host_with_clock(
        HostRole::File,
        &initial,
        "unsaved-preview.c",
        Arc::new(ClipboardService::new(Box::new(HeadlessClipboard))),
        Arc::new(ReplayClock(Arc::clone(&now))),
    );
    let edit_at = "foo\nline-0".len();
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(edit_at),
            extend: false,
        },
        &mut documents,
        Rect::new(0, 0, 70, 14),
    )
    .unwrap();
    assert!(
        view.dispatch(
            &InputEvent::Paste("-edited".into()),
            &mut documents,
            &mut focus,
            Rect::new(0, 0, 70, 14),
        )
        .unwrap()
    );
    let edited_text = source(&documents, &view);
    assert!(edited_text.contains("line-0-edited"));
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/unsaved-preview.c"),
            document: Some(view.document_id()),
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(0),
                column: Utf16Column(0),
            }),
            declaration_range: Some(
                TextRange::new(ByteOffset(0), ByteOffset(snapshot.len_bytes())).unwrap(),
            ),
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    let area = Rect::new(0, 0, 70, 14);
    let geometry = view.geometry(&documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }),
        &mut documents,
        &mut focus,
        area,
    )
    .unwrap();
    now.store(500, Ordering::SeqCst);
    view.tick(&documents, &focus, area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .expect("hover timer requests unsaved document definition");
    let provider = view.host.intelligence.clone().unwrap();
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &ReplayMissingNavigationSource,
        Some(&ReplayPreviewHighlighter),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    let preview = view.navigation.definition.as_ref().unwrap().targets[0]
        .preview
        .as_ref()
        .unwrap();
    assert_eq!(preview.snapshot.id, view.document_id());
    assert_eq!(
        preview.lines,
        kode_tui::editor::popups::definition::MAX_PREVIEW_LINES
    );
    assert_eq!(preview.snapshot.text().as_ref(), edited_text);
    assert_eq!(
        preview
            .snapshot
            .line_content(LineIndex(1))
            .unwrap()
            .to_string(),
        "line-0-edited"
    );
    assert!(
        preview
            .highlights
            .iter()
            .any(|span| span.class == "semantic.keyword")
    );
}

#[test]
fn missing_definition_source_renders_feedback_and_location_remains_navigable() {
    let area = Rect::new(0, 0, 70, 14);
    let now = Arc::new(AtomicU64::new(0));
    let (mut documents, mut view, mut focus) = open_host_with_clock(
        HostRole::File,
        "foo\n",
        "missing-preview.c",
        Arc::new(ClipboardService::new(Box::new(HeadlessClipboard))),
        Arc::new(ReplayClock(Arc::clone(&now))),
    );
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/missing.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(1),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    let geometry = view.geometry(&documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }),
        &mut documents,
        &mut focus,
        area,
    )
    .unwrap();
    now.store(500, Ordering::SeqCst);
    view.tick(&documents, &focus, area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.intelligence.clone().unwrap();
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &ReplayMissingNavigationSource,
        None,
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    let target = &view.navigation.definition.as_ref().unwrap().targets[0];
    assert_eq!(target.location, "src/missing.c:2");
    assert!(target.preview.is_err());
    let popup = definition_geometry(&documents, &mut view, area);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    view.render(
        &documents,
        &focus,
        area,
        &mut buffer,
        &kode_tui::theme::Theme::compiled(),
        &[],
    )
    .unwrap();
    let rendered: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(rendered.contains("source unavailable"));
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.location.x,
                y: popup.location.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.definition.is_none());
    assert!(view
        .take_work()
        .into_iter()
        .any(|work| matches!(work, EditorWork::Navigate(target) if target.path == std::path::Path::new("/fixture/src/missing.c") && target.location.is_some_and(|location| location.line == LineIndex(1)))));
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn definition_preview_controls_hover_cycle_clamp_and_navigate_selected_location() {
    let now = Arc::new(AtomicU64::new(0));
    let clipboard = Arc::new(ClipboardService::new(Box::new(HeadlessClipboard)));
    let (mut documents, mut view, mut focus) = open_host_with_clock(
        HostRole::File,
        "foo\n",
        "preview-controls.c",
        clipboard,
        Arc::new(ReplayClock(Arc::clone(&now))),
    );
    let targets = [("first.c", 0usize), ("second.c", 3usize)]
        .into_iter()
        .map(|(name, line)| kode_core::intelligence::NavigationTarget {
            path: PathBuf::from(format!("/fixture/src/{name}")),
            document: None,
            identifier_range: TextRange::new(ByteOffset(line * 7), ByteOffset(line * 7 + 3))
                .unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(line),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        })
        .collect();
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(targets)));
    view.navigation.enabled = true;
    let area = Rect::new(0, 0, 70, 14);
    let geometry = view.geometry(&documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }),
        &mut documents,
        &mut focus,
        area,
    )
    .unwrap();
    now.store(500, Ordering::SeqCst);
    view.tick(&documents, &focus, area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/first.c")),
            Some("c".into()),
            &"foo();\n".repeat(8),
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        Some(&ReplayPreviewHighlighter),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    assert_eq!(
        view.navigation.definition.as_ref().unwrap().targets.len(),
        2
    );

    let popup = definition_geometry(&documents, &mut view, area);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.next.x,
                y: popup.next.y,
                action: PointerAction::Move,
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(
        view.navigation.definition.as_ref().unwrap().hovered,
        Some(kode_tui::editor::popups::definition::DefinitionControl::Next)
    );
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.next.x,
                y: popup.next.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.navigation.definition.as_ref().unwrap().selected, 1);

    let popup = definition_geometry(&documents, &mut view, area);
    assert_eq!(
        view.navigation.definition.as_ref().unwrap().targets[1].location,
        "src/second.c:4"
    );
    for _ in 0..2 {
        assert!(
            view.dispatch(
                &InputEvent::Pointer(PointerInput {
                    x: popup.next.x,
                    y: popup.next.y,
                    action: PointerAction::Down(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
        );
    }
    assert_eq!(view.navigation.definition.as_ref().unwrap().selected, 1);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.previous.x,
                y: popup.previous.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.navigation.definition.as_ref().unwrap().selected, 0);
    let popup = definition_geometry(&documents, &mut view, area);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.previous.x,
                y: popup.previous.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.navigation.definition.as_ref().unwrap().selected, 0);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.next.x,
                y: popup.next.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let popup = definition_geometry(&documents, &mut view, area);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.location.x,
                y: popup.location.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.definition.is_none());
    assert!(view
        .take_work()
        .into_iter()
        .any(|work| matches!(work, EditorWork::Navigate(target) if target.path == std::path::Path::new("/fixture/src/second.c") && target.location.is_some_and(|location| location.line == LineIndex(3)))));
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn definition_preview_owns_pointer_while_other_keys_keep_editor_routing() {
    let now = Arc::new(AtomicU64::new(0));
    let area = Rect::new(0, 0, 70, 14);
    let (mut documents, mut view, mut focus) = open_host_with_clock(
        HostRole::File,
        "foo\n",
        "preview-pointer-routing.c",
        Arc::new(ClipboardService::new(Box::new(HeadlessClipboard))),
        Arc::new(ReplayClock(Arc::clone(&now))),
    );
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/definition.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(4), ByteOffset(7)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(0),
                column: Utf16Column(4),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    complete_hover_preview_through_host(&mut documents, &mut view, &mut focus, &now, area);
    let selection = view.input.state.caret.selection;
    let popup = definition_geometry(&documents, &mut view, area);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.source.x,
                y: popup.source.y,
                action: PointerAction::Move,
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.source.x,
                y: popup.source.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.definition.is_some());
    assert_eq!(view.input.state.caret.selection, selection);
    assert_eq!(source(&documents, &view), "foo\n");
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: area.right() - 1,
                y: area.bottom() - 1,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.definition.is_none());
    assert_eq!(view.input.state.caret.selection, selection);

    complete_hover_preview_through_host(&mut documents, &mut view, &mut focus, &now, area);
    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.navigation.definition.is_none());

    complete_hover_preview_through_host(&mut documents, &mut view, &mut focus, &now, area);
    assert!(
        view.dispatch(
            &event(Key::Character('x')),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "xfoo\n");
    assert!(view.navigation.definition.is_none());
}

#[test]
fn definition_preview_clips_at_edges_and_remains_safe_at_tiny_viewports() {
    let mut text = (0..24)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    text.push_str(&format!("{}foo", "x".repeat(60)));
    let (mut documents, mut view, mut focus) = open_file(&text, "definition-edge.c");
    let identifier_start = text.len() - 3;
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(text.len()),
            extend: false,
        },
        &mut documents,
        Rect::new(0, 0, 80, 20),
    )
    .unwrap();
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/first.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(0),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/second.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(1),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    assert!(
        view.navigation
            .begin(
                &snapshot,
                &view.input.state,
                ByteOffset(identifier_start),
                kode_tui::editor::popups::definition::NavigationOperation::Hover,
            )
            .unwrap()
    );
    let full_area = Rect::new(0, 0, 80, 20);
    view.tick(&documents, &focus, full_area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/first.c")),
            Some("c".into()),
            &"int foo(void);\n".repeat(8),
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        Some(&ReplayPreviewHighlighter),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );

    for (width, height) in [(80, 20), (12, 5), (1, 1), (20, 6)] {
        let area = Rect::new(0, 0, width, height);
        let geometry = view.geometry(&documents, area).unwrap();
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        view.render(
            &documents,
            &focus,
            area,
            &mut buffer,
            &kode_tui::theme::Theme::compiled(),
            &[],
        )
        .unwrap();
        let document = documents.document(view.document_id()).unwrap();
        let layout = view
            .input
            .layout(
                document,
                kode_tui::editor::input::InputViewport {
                    geometry,
                    folds: &[],
                },
            )
            .unwrap();
        if let Some(popup) = view.navigation.definition.as_ref().unwrap().geometry(
            kode_tui::editor::input::HookContext {
                geometry,
                layout: &layout,
            },
            &view.input.state,
        ) {
            assert_eq!(popup.area.intersection(geometry.text), popup.area);
            for control in [popup.previous, popup.next, popup.location, popup.source] {
                assert_eq!(control.intersection(popup.area), control);
            }
            if (width, height) == (20, 6) {
                assert!(popup.next.width > 0);
                assert!(
                    view.dispatch(
                        &InputEvent::Pointer(PointerInput {
                            x: popup.next.x,
                            y: popup.next.y,
                            action: PointerAction::Down(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        }),
                        &mut documents,
                        &mut focus,
                        area,
                    )
                    .unwrap()
                );
                assert_eq!(view.navigation.definition.as_ref().unwrap().selected, 1);
                let popup = definition_geometry(&documents, &mut view, area);
                assert!(
                    view.dispatch(
                        &InputEvent::Pointer(PointerInput {
                            x: popup.location.x,
                            y: popup.location.y,
                            action: PointerAction::Down(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        }),
                        &mut documents,
                        &mut focus,
                        area,
                    )
                    .unwrap()
                );
                assert!(view
                    .take_work()
                    .into_iter()
                    .any(|work| matches!(work, EditorWork::Navigate(target) if target.path == std::path::Path::new("/fixture/src/second.c"))));
            }
        }
    }
}

#[test]
fn usage_popup_escape_and_outside_click_dismiss_immediately_without_clickthrough() {
    let area = Rect::new(0, 0, 70, 14);
    let (mut documents, mut view, mut focus) = open_file("foo\n", "usage-dismiss.c");
    view.host.intelligence = Some(Arc::new(ReplayReferences(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/uses.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(0),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));

    complete_usage_popup_through_host(&mut documents, &mut view, &mut focus, area);
    assert!(view.navigation.usages.is_some());
    let selection = view.input.state.caret.selection;
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: area.right() - 1,
                y: area.bottom() - 1,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.usages.is_none());
    assert_eq!(view.input.state.caret.selection, selection);
    assert_eq!(source(&documents, &view), "foo\n");

    complete_usage_popup_through_host(&mut documents, &mut view, &mut focus, area);
    assert!(view.navigation.usages.is_some());
    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.navigation.usages.is_none());
    assert_eq!(view.input.state.caret.selection, selection);
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn hover_preview_timer_publishes_bounded_highlighted_definition_without_mouse_motion() {
    let now = Arc::new(AtomicU64::new(0));
    let clipboard = Arc::new(ClipboardService::new(Box::new(HeadlessClipboard)));
    let (mut documents, mut view, mut focus) = open_host_with_clock(
        HostRole::File,
        "foo\n",
        "hover-preview.c",
        clipboard,
        Arc::new(ReplayClock(Arc::clone(&now))),
    );
    let target = kode_core::intelligence::NavigationTarget {
        path: PathBuf::from("/fixture/src/definition.c"),
        document: None,
        identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
        location: Some(Utf16Position {
            line: LineIndex(0),
            column: Utf16Column(0),
        }),
        declaration_range: Some(TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap()),
        language: Some("c".into()),
        kind: None,
        name: Some("foo".into()),
        missing_source: false,
    };
    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![target])));
    view.navigation.enabled = true;
    let area = Rect::new(0, 0, 70, 14);
    let geometry = view.geometry(&documents, area).unwrap();
    view.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: geometry.text.x,
            y: geometry.text.y,
            action: PointerAction::Move,
            modifiers: Modifiers::default(),
        }),
        &mut documents,
        &mut focus,
        area,
    )
    .unwrap();
    view.take_work();

    now.store(499, Ordering::SeqCst);
    view.tick(&documents, &focus, area).unwrap();
    assert!(
        view.take_work()
            .into_iter()
            .all(|work| !matches!(work, EditorWork::Navigation(_)))
    );

    now.store(500, Ordering::SeqCst);
    view.tick(&documents, &focus, area).unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .expect("hover timer schedules navigation without more pointer movement");
    assert_eq!(
        job.operation,
        kode_tui::editor::popups::definition::NavigationOperation::Hover
    );
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/definition.c")),
            Some("c".into()),
            &"foo();\n".repeat(30),
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        Some(&ReplayPreviewHighlighter),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    let popup = view.navigation.definition.as_ref().unwrap();
    assert_eq!(popup.targets.len(), 1);
    assert_eq!(popup.targets[0].location, "src/definition.c:1");
    let preview = popup.targets[0].preview.as_ref().unwrap();
    assert!(preview.lines <= kode_tui::editor::popups::definition::MAX_PREVIEW_LINES);
    assert!(
        preview
            .highlights
            .iter()
            .any(|span| span.class == "semantic.keyword")
    );
    assert!(view.navigation.usages.is_none());
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn usage_popup_navigation_scrolls_and_enters_selected_reference_through_host() {
    let area = Rect::new(0, 0, 70, 14);
    let (mut documents, mut view, mut focus) = open_file("foo\n", "usage-navigation.c");
    let targets = (0..12)
        .map(|line| kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/use.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(line * 7), ByteOffset(line * 7 + 3))
                .unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(line),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        })
        .collect();
    view.host.intelligence = Some(Arc::new(ReplayReferences(targets)));
    view.navigation.enabled = true;
    view.navigation
        .set_declaration_spans(
            &documents.document(view.document_id()).unwrap().snapshot(),
            vec![TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap()],
        )
        .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: geometry.text.x,
                y: geometry.text.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/use.c")),
            Some("c".into()),
            &"foo();\n".repeat(12),
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        None,
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    assert_eq!(view.navigation.usages.as_ref().unwrap().entries.len(), 12);

    let geometry = view.geometry(&documents, area).unwrap();
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup = view
        .navigation
        .usages
        .as_ref()
        .unwrap()
        .geometry(
            kode_tui::editor::input::HookContext {
                geometry,
                layout: &layout,
            },
            &view.input.state,
        )
        .unwrap();
    assert!(popup.track.width > 0);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.area.x,
                y: popup.area.y,
                action: PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 1,
                },
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.navigation.usages.as_ref().unwrap().offset, 1);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.track.x,
                y: popup.track.bottom() - 1,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let track_offset = view.navigation.usages.as_ref().unwrap().offset;
    assert!(track_offset > 1);
    assert!(view.navigation.usages.as_ref().unwrap().is_dragging());
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.track.x,
                y: popup.track.y,
                action: PointerAction::Drag(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.navigation.usages.as_ref().unwrap().offset < track_offset);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.track.x,
                y: popup.track.y,
                action: PointerAction::Up(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(!view.navigation.usages.as_ref().unwrap().is_dragging());
    assert!(
        view.dispatch(
            &event(Key::Character('x')),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "foo\n");
    assert!(
        view.dispatch(&event(Key::Home), &mut documents, &mut focus, area)
            .unwrap()
    );

    assert!(
        view.dispatch(&event(Key::Down), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.navigation.usages.as_ref().unwrap().selected, 1);
    assert!(
        view.dispatch(&event(Key::Up), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.navigation.usages.as_ref().unwrap().selected, 0);
    assert!(
        view.dispatch(&event(Key::Down), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::PageDown), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.navigation.usages.as_ref().unwrap().selected > 1);
    assert!(
        view.dispatch(&event(Key::End), &mut documents, &mut focus, area)
            .unwrap()
    );
    let usages = view.navigation.usages.as_ref().unwrap();
    assert_eq!(usages.selected, 11);
    assert!(usages.offset > 0);
    assert!(
        view.dispatch(&event(Key::PageUp), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.navigation.usages.as_ref().unwrap().selected < 11);
    assert!(
        view.dispatch(&event(Key::Home), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.navigation.usages.as_ref().unwrap().selected, 0);
    assert!(
        view.dispatch(&event(Key::End), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.navigation.usages.is_none());
    assert!(view
        .take_work()
        .into_iter()
        .any(|work| matches!(work, EditorWork::Navigate(target) if target.location.is_some_and(|location| location.line == LineIndex(11)))));
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn declaration_ctrl_click_publishes_project_relative_usage_rows_through_editor_host() {
    let area = Rect::new(0, 0, 70, 14);
    let (mut documents, mut view, mut focus) = open_file("foo\n", "declaration-usage.c");
    view.host.intelligence = Some(Arc::new(ReplayReferences(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/use.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(7), ByteOffset(10)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(1),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("foo".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    view.navigation
        .set_declaration_spans(
            &documents.document(view.document_id()).unwrap().snapshot(),
            vec![TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap()],
        )
        .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: geometry.text.x,
                y: geometry.text.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .expect("declaration click schedules references query");
    assert_eq!(
        job.operation,
        kode_tui::editor::popups::definition::NavigationOperation::Click
    );
    assert!(job.declaration_hint);
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/use.c")),
            Some("c".into()),
            "header\nfoo();\n",
        )
        .snapshot(),
    );
    let result = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        None,
        || false,
    )
    .unwrap();
    assert!(
        view.complete_navigation(job.tag, Ok(result), &documents, true)
            .unwrap()
    );
    let usages = view.navigation.usages.as_ref().unwrap();
    assert_eq!(usages.entries.len(), 1);
    assert_eq!(usages.entries[0].label, "src/use.c:2:1 | foo();");
    assert_eq!(
        usages.entries[0].target.path,
        PathBuf::from("/fixture/src/use.c")
    );
    assert_eq!(source(&documents, &view), "foo\n");
}

#[test]
fn late_completion_and_navigation_results_do_not_resurrect_changed_host_context() {
    let area = Rect::new(0, 0, 60, 14);
    let (mut documents, mut view, mut focus) = open_file("pri\nfoo\n", "late-host-results.c");
    view.host.external = Some(Arc::new(ReplayHostCompletion));
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.external.clone().unwrap();
    let prepared =
        kode_tui::editor::popups::completion::prepare(&job, Some(provider.as_ref()), None, || {
            false
        })
        .unwrap();
    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        !view
            .complete_completion(job.tag, Ok(prepared), &documents, &focus)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());

    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let prepared =
        kode_tui::editor::popups::completion::prepare(&job, Some(provider.as_ref()), None, || {
            false
        })
        .unwrap();
    assert!(
        view.dispatch(&event(Key::Left), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        !view
            .complete_completion(job.tag, Ok(prepared), &documents, &focus)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());

    view.host.intelligence = Some(Arc::new(ReplayDefinitions(vec![
        kode_core::intelligence::NavigationTarget {
            path: PathBuf::from("/fixture/src/foo.c"),
            document: None,
            identifier_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
            location: Some(Utf16Position {
                line: LineIndex(0),
                column: Utf16Column(0),
            }),
            declaration_range: None,
            language: Some("c".into()),
            kind: None,
            name: Some("pri".into()),
            missing_source: false,
        },
    ])));
    view.navigation.enabled = true;
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(1),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: view.geometry(&documents, area).unwrap().text.x,
                y: view.geometry(&documents, area).unwrap().text.y,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Navigation(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let provider = view.host.intelligence.clone().unwrap();
    let reader = ReplayNavigationSource(
        Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::File(PathBuf::from("/fixture/src/foo.c")),
            Some("c".into()),
            "int pri(void);\n",
        )
        .snapshot(),
    );
    let prepared = kode_tui::editor::popups::definition::prepare(
        &job,
        provider.as_ref(),
        &reader,
        None,
        || false,
    )
    .unwrap();
    assert!(
        view.dispatch(&event(Key::Right), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(
        !view
            .complete_navigation(job.tag, Ok(prepared), &documents, true)
            .unwrap()
    );
    assert!(view.navigation.definition.is_none());
    assert!(view.navigation.usages.is_none());
    assert!(
        !view
            .take_work()
            .into_iter()
            .any(|work| matches!(work, EditorWork::Navigate(_)))
    );
}

#[test]
fn completion_escape_outside_click_and_edit_dismiss_without_clickthrough() {
    fn show_completion(
        documents: &mut EditorDocuments,
        view: &mut EditorView<QueuedEditorHost>,
        focus: &mut FocusState,
        area: Rect,
    ) {
        view.host.external = Some(Arc::new(ReplayHostCompletion));
        view.command(
            EditorCommand::MoveTo {
                position: ByteOffset(3),
                extend: false,
            },
            documents,
            area,
        )
        .unwrap();
        let ctrl_space = InputEvent::Key(KeyInput {
            key: Key::Character(' '),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        });
        assert!(view.dispatch(&ctrl_space, documents, focus, area).unwrap());
        let job = view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::Completion(job) => Some(job),
                _ => None,
            })
            .expect("explicit completion request schedules worker job");
        let host = view.host.external.clone().unwrap();
        let entries =
            kode_tui::editor::popups::completion::prepare(&job, Some(host.as_ref()), None, || {
                false
            })
            .unwrap();
        assert!(
            view.complete_completion(job.tag, Ok(entries), documents, focus)
                .unwrap()
        );
    }

    let area = Rect::new(0, 0, 50, 12);
    let (mut documents, mut view, mut focus) = open_file("pri", "completion-escape.rs");
    show_completion(&mut documents, &mut view, &mut focus, area);
    assert!(!view.completion.entries.is_empty());
    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    assert_eq!(source(&documents, &view), "pri");

    let (mut documents, mut view, mut focus) = open_file("pri", "completion-outside.rs");
    show_completion(&mut documents, &mut view, &mut focus, area);
    let selection = view.input.state.caret.selection;
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: area.right() - 1,
                y: area.bottom() - 1,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    assert_eq!(view.input.state.caret.selection, selection);
    assert_eq!(source(&documents, &view), "pri");

    let (mut documents, mut view, mut focus) = open_file("pri", "completion-edit.rs");
    show_completion(&mut documents, &mut view, &mut focus, area);
    assert!(
        view.dispatch(
            &event(Key::Character('x')),
            &mut documents,
            &mut focus,
            area
        )
        .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    assert_eq!(source(&documents, &view), "prix");
}

#[test]
fn ctrl_space_and_terminal_nul_request_completion_and_empty_result_closes_popup() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let (mut documents, mut view, mut focus) = open_file("pri", "ctrl-space-completion.rs");
    let area = Rect::new(0, 0, 40, 10);
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let first = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("Ctrl+Space schedules completion");
    let item = CompletionItem {
        label: "println".into(),
        insert_text: "println".into(),
        replacement_range: TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap(),
        detail: Some("fixture".into()),
        source: "fixture".into(),
        kind: None,
    };
    assert!(
        view.complete_completion(first.tag, Ok(vec![item]), &documents, &focus)
            .unwrap()
    );
    assert_eq!(view.completion.entries.len(), 1);

    assert!(
        view.dispatch(&event(Key::Escape), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    let nul = kode_tui::input::terminal::normalize(Event::Key(KeyEvent::new(
        KeyCode::Null,
        KeyModifiers::NONE,
    )))
    .expect("terminal NUL normalizes to Ctrl+Space");
    assert!(
        view.dispatch(&nul, &mut documents, &mut focus, area)
            .unwrap()
    );
    let second = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("normalized terminal NUL schedules completion");
    assert!(
        view.complete_completion(second.tag, Ok(vec![]), &documents, &focus)
            .unwrap()
    );
    assert!(view.completion.entries.is_empty());
    assert_eq!(source(&documents, &view), "pri");
    assert_eq!(
        documents
            .document(view.document_id())
            .unwrap()
            .history_depth(),
        (0, 0)
    );
}

#[test]
fn external_completion_auto_show_obeys_host_configuration() {
    let area = Rect::new(0, 0, 50, 12);
    let (mut documents, mut view, mut focus) = open_host(HostRole::File, "", "file-completion.rs");
    view.host.external = Some(Arc::new(ReplayHostCompletion));
    assert!(
        view.dispatch(
            &InputEvent::Paste("pri".into()),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(
        view.take_work()
            .into_iter()
            .all(|work| !matches!(work, EditorWork::Completion(_)))
    );

    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("file host waits for explicit completion request");
    let host = view.host.external.clone().unwrap();
    let entries =
        kode_tui::editor::popups::completion::prepare(&job, Some(host.as_ref()), None, || false)
            .unwrap();
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );
    assert_eq!(view.completion.entries[0].label, "host-first");

    let (mut documents, mut view, mut focus) =
        open_host(HostRole::RestHeaders, "", "rest-headers-completion");
    view.host.external = Some(Arc::new(ReplayHostCompletion));
    view.host.descriptor.auto_show_external = true;
    assert!(
        view.dispatch(
            &InputEvent::Paste("pri".into()),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .rev()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("REST header host auto-shows external completion");
    let host = view.host.external.clone().unwrap();
    let entries =
        kode_tui::editor::popups::completion::prepare(&job, Some(host.as_ref()), None, || false)
            .unwrap();
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );
    assert_eq!(view.completion.entries[0].label, "host-first");
    assert_eq!(view.completion.entries[0].source, "host");
    assert_eq!(source(&documents, &view), "pri");
}

#[test]
fn completion_popup_wheel_and_track_scroll_through_editor_host() {
    let (mut documents, mut view, mut focus) = open_file("pri", "completion-scroll.rs");
    let area = Rect::new(0, 0, 50, 12);
    view.host.intelligence = Some(Arc::new(ReplayIntelligenceCompletion));
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("completion request schedules worker job");
    let provider = view.host.intelligence.clone().unwrap();
    let entries =
        kode_tui::editor::popups::completion::prepare(&job, None, Some(provider.as_ref()), || {
            false
        })
        .unwrap();
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );

    let geometry = view.geometry(&documents, area).unwrap();
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let popup = view
        .completion
        .geometry(
            kode_tui::editor::input::HookContext {
                geometry,
                layout: &layout,
            },
            &view.input.state,
        )
        .expect("long result list displays a scrollbar");
    assert!(popup.track.width > 0);
    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.text.x,
                y: popup.rows.y,
                action: PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 2,
                },
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.completion.offset, 2);

    assert!(
        view.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: popup.track.x,
                y: popup.track.bottom() - 1,
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(view.completion.offset > 2);
    assert_eq!(view.completion.selected, view.completion.offset);
    assert_eq!(source(&documents, &view), "pri");
}

#[test]
fn completion_accept_replaces_prefix_and_preserves_receiver_and_suffix() {
    let area = Rect::new(0, 0, 50, 12);
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    for receiver in [".", "?.", "->", "::"] {
        let initial = format!("customer{receiver}pr_tail");
        let (mut documents, mut view, mut focus) =
            open_file(&initial, &format!("completion-receiver-{receiver}.rs"));
        view.host.external = Some(Arc::new(ReplayReceiverCompletion));
        view.command(
            EditorCommand::MoveTo {
                position: ByteOffset(format!("customer{receiver}pr").len()),
                extend: false,
            },
            &mut documents,
            area,
        )
        .unwrap();
        assert!(
            view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
                .unwrap()
        );
        let job = view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::Completion(job) => Some(job),
                _ => None,
            })
            .expect("completion request schedules worker job");
        let host = view.host.external.clone().unwrap();
        let entries =
            kode_tui::editor::popups::completion::prepare(&job, Some(host.as_ref()), None, || {
                false
            })
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            view.complete_completion(job.tag, Ok(entries), &documents, &focus)
                .unwrap()
        );
        assert!(
            view.dispatch(&event(Key::Enter), &mut documents, &mut focus, area)
                .unwrap()
        );
        assert_eq!(
            source(&documents, &view),
            format!("customer{receiver}property_tail")
        );
        assert_eq!(
            documents
                .document(view.document_id())
                .unwrap()
                .history_depth(),
            (1, 0)
        );
    }
}

#[test]
fn completion_keyboard_navigation_keeps_long_list_selection_visible() {
    let (mut documents, mut view, mut focus) = open_file("pri", "completion-navigation.rs");
    let area = Rect::new(0, 0, 50, 12);
    view.host.intelligence = Some(Arc::new(ReplayIntelligenceCompletion));
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("completion request schedules worker job");
    let provider = view.host.intelligence.clone().unwrap();
    let entries =
        kode_tui::editor::popups::completion::prepare(&job, None, Some(provider.as_ref()), || {
            false
        })
        .unwrap();
    assert_eq!(entries.len(), 50);
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );

    for _ in 0..20 {
        assert!(
            view.dispatch(&event(Key::Down), &mut documents, &mut focus, area)
                .unwrap()
        );
    }
    assert_eq!(view.completion.selected, 20);
    assert!(view.completion.offset > 0);
    assert!(view.completion.selected >= view.completion.offset);

    assert!(
        view.dispatch(&event(Key::PageDown), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.completion.selected > 20);
    assert!(view.completion.selected >= view.completion.offset);
    assert!(
        view.dispatch(&event(Key::End), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.completion.selected, 49);
    assert!(view.completion.selected >= view.completion.offset);
    assert!(
        view.dispatch(&event(Key::PageUp), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.completion.selected < 49);
    assert!(view.completion.selected >= view.completion.offset);
    assert!(
        view.dispatch(&event(Key::Home), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.completion.selected, 0);
    assert_eq!(view.completion.offset, 0);
    assert_eq!(source(&documents, &view), "pri");
}

#[test]
fn completion_host_results_precede_intelligence_deduplicate_and_cap_at_fifty() {
    let (mut documents, mut view, mut focus) = open_file("pri", "completion-order.rs");
    let area = Rect::new(0, 0, 50, 12);
    view.host.external = Some(Arc::new(ReplayHostCompletion));
    view.host.intelligence = Some(Arc::new(ReplayIntelligenceCompletion));
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(3),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .expect("explicit completion request schedules worker job");
    let host = view.host.external.clone().unwrap();
    let intelligence = view.host.intelligence.clone().unwrap();
    let entries = kode_tui::editor::popups::completion::prepare(
        &job,
        Some(host.as_ref()),
        Some(intelligence.as_ref()),
        || false,
    )
    .unwrap();
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );

    let entries = &view.completion.entries;
    assert_eq!(entries.len(), 50);
    assert_eq!(entries[0].label, "host-first");
    assert_eq!(entries[0].insert_text, "first");
    assert_eq!(entries[0].detail.as_deref(), Some("host detail"));
    assert_eq!(entries[0].source, "host");
    assert_eq!(entries[1].label, "shared");
    assert_eq!(entries[1].detail.as_deref(), Some("host wins"));
    assert_eq!(entries[1].source, "host");
    assert_eq!(entries[2].label, "provider-00");
    assert_eq!(entries[49].label, "provider-47");
    assert!(
        entries[2..]
            .iter()
            .all(|entry| entry.source == "intelligence")
    );
    assert_eq!(source(&documents, &view), "pri");
}

#[test]
fn member_context_blocks_local_completion_fallback_but_plain_prefix_keeps_it() {
    let area = Rect::new(0, 0, 50, 12);
    let ctrl_space = InputEvent::Key(KeyInput {
        key: Key::Character(' '),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    });
    for source_text in [
        "customer.pr",
        "customer?.pr",
        "customer->pr",
        "customer::pr",
    ] {
        let (mut documents, mut view, mut focus) =
            open_file(source_text, &format!("member-context-{source_text}.txt"));
        view.command(
            EditorCommand::MoveTo {
                position: ByteOffset(source_text.len()),
                extend: false,
            },
            &mut documents,
            area,
        )
        .unwrap();
        assert!(
            view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
                .unwrap()
        );
        let job = view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::Completion(job) => Some(job),
                _ => None,
            })
            .unwrap();
        let entries =
            kode_tui::editor::popups::completion::prepare(&job, None, None, || false).unwrap();
        assert!(entries.is_empty(), "{source_text}");
        assert!(
            view.complete_completion(job.tag, Ok(entries), &documents, &focus)
                .unwrap()
        );
        assert!(view.completion.entries.is_empty(), "{source_text}");
    }

    let (mut documents, mut view, mut focus) = open_file("printable", "local-fallback.txt");
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(2),
            extend: false,
        },
        &mut documents,
        area,
    )
    .unwrap();
    assert!(
        view.dispatch(&ctrl_space, &mut documents, &mut focus, area)
            .unwrap()
    );
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Completion(job) => Some(job),
            _ => None,
        })
        .unwrap();
    let entries =
        kode_tui::editor::popups::completion::prepare(&job, None, None, || false).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| { entry.label == "printable" && entry.source == "local" })
    );
    assert!(
        view.complete_completion(job.tag, Ok(entries), &documents, &focus)
            .unwrap()
    );
    assert!(
        view.completion
            .entries
            .iter()
            .any(|entry| entry.label == "printable" && entry.source == "local")
    );
}

fn replay_wrapped_pointer_selection(sequence: &Value) {
    let initial = sequence["text"].as_str().unwrap();
    let (mut documents, mut view, mut focus) = open_file(initial, "wrapped-pointer.txt");
    let area = Rect::new(0, 0, 20, 6);
    let geometry = view.geometry(&documents, area).unwrap();
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    assert!(layout.rows.len() > 1);
    let expected_anchor = layout.cell_to_position(0, 0);
    let expected_head = layout.cell_to_position(1, (geometry.text.width - 1) as usize);
    assert!(expected_head > expected_anchor);
    let down = InputEvent::Pointer(PointerInput {
        x: geometry.text.x,
        y: geometry.text.y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&down, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(focus.captured, Some(view.input.id));
    let drag = InputEvent::Pointer(PointerInput {
        x: geometry.text.right() - 1,
        y: geometry.text.y + 1,
        action: PointerAction::Drag(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&drag, &mut documents, &mut focus, area)
            .unwrap()
    );
    let selection = view.input.state.caret.selection;
    assert_eq!(
        selection,
        Selection {
            anchor: expected_anchor,
            head: expected_head,
        }
    );
    let release = InputEvent::Pointer(PointerInput {
        x: area.right() + 5,
        y: area.bottom() + 5,
        action: PointerAction::Up(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&release, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(focus.captured, None);
    assert_eq!(view.input.state.caret.selection, selection);
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn clicking_wrapped_source_uses_the_rendered_cell_map_and_focuses_editor() {
    let initial = "\t".repeat(100);
    let (mut documents, mut view, mut focus) = open_file(&initial, "wrapped-click.txt");
    let area = Rect::new(0, 0, 23, 6);
    let geometry = view.geometry(&documents, area).unwrap();
    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    let expected = layout.cell_to_position(1, 5);
    let click = |action| {
        InputEvent::Pointer(PointerInput {
            x: geometry.text.x + 5,
            y: geometry.text.y + 1,
            action,
            modifiers: Modifiers::default(),
        })
    };
    assert!(
        view.dispatch(
            &click(PointerAction::Down(MouseButton::Left)),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert!(
        view.dispatch(
            &click(PointerAction::Up(MouseButton::Left)),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(view.input.state.caret.cursor(), expected);
    assert_eq!(focus.focused, Some(view.input.id));
    assert!(!view.input.state.search_has_focus);
    assert_eq!(source(&documents, &view), initial);
}

#[test]
fn rendered_tab_marker_keeps_four_styled_cells_on_one_source_tab() {
    use kode_tui::theme::Theme;
    use ratatui::buffer::Buffer;

    let (mut documents, mut view, mut focus) = open_file("a\tb", "tab-render.txt");
    let area = Rect::new(0, 0, 23, 6);
    let geometry = view.geometry(&documents, area).unwrap();
    let mut buffer = Buffer::empty(area);
    let theme = Theme::compiled();
    view.render(&documents, &focus, area, &mut buffer, &theme, &[])
        .unwrap();
    let visible: String = (geometry.text.x..geometry.text.x + 6)
        .map(|x| buffer[(x, geometry.text.y)].symbol())
        .collect();
    assert_eq!(visible, "a|-->b");
    let tab_style = theme.style("code-tab").fg.unwrap();
    for column in 1..5 {
        assert_eq!(
            buffer[(geometry.text.x + column, geometry.text.y)].fg,
            tab_style
        );
    }

    let document = documents.document(view.document_id()).unwrap();
    let layout = view
        .input
        .layout(
            document,
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &[],
            },
        )
        .unwrap();
    assert_eq!(layout.position_to_cell(ByteOffset(1)).unwrap(), (0, 1));
    for column in 2..5 {
        assert_eq!(layout.cell_to_position(0, column), ByteOffset(2));
    }
    assert_eq!(layout.position_to_cell(ByteOffset(2)).unwrap(), (0, 5));
    assert_eq!(source(&documents, &view), "a\tb");

    let click = InputEvent::Pointer(PointerInput {
        x: geometry.text.x + 3,
        y: geometry.text.y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&click, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.input.state.caret.cursor(), ByteOffset(2));
    assert!(
        view.dispatch(
            &event(Key::Character('X')),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "a\tXb");
}

#[test]
fn overlapping_lexical_and_semantic_spans_render_once_with_semantic_precedence() {
    use kode_syntax::model::{GrammarMode, GrammarStatus};
    use kode_tui::editor::overlays::{OverlayReport, PaintSpan};
    use ratatui::style::Color;

    let text = "fn alpha";
    let (documents, mut view, _focus) = open_file(text, "overlapping-overlays.rs");
    let area = Rect::new(0, 0, 40, 8);
    view.overlays.enable();
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    let covered = TextRange::new(ByteOffset(0), ByteOffset(text.len())).unwrap();
    let job = view
        .overlays
        .request(
            &snapshot,
            covered,
            covered,
            WorkspaceGeneration(1),
            "rust".into(),
        )
        .unwrap()
        .unwrap();
    assert!(
        view.overlays
            .complete(
                job.tag,
                Ok(OverlayReport {
                    tag: job.tag,
                    range: covered,
                    lexical: vec![PaintSpan {
                        range: covered,
                        class: "fixture.lexical".into(),
                        color: Some([220, 20, 60]),
                        priority: 10,
                    }],
                    semantic: Some(vec![PaintSpan {
                        range: TextRange::new(ByteOffset(3), ByteOffset(8)).unwrap(),
                        class: "fixture.semantic".into(),
                        color: Some([20, 180, 90]),
                        priority: 30,
                    }]),
                    status: GrammarStatus {
                        language: "rust".into(),
                        mode: GrammarMode::Plain,
                        message: None,
                    },
                    notes: vec![],
                }),
                &snapshot,
            )
            .unwrap()
    );

    let theme = kode_tui::theme::Theme::compiled();
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    view.render(
        &documents,
        &FocusState::default(),
        area,
        &mut buffer,
        &theme,
        &[],
    )
    .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    let rendered: String = (0..text.chars().count())
        .map(|column| {
            buffer[(geometry.text.x + column as u16, geometry.text.y)]
                .symbol()
                .to_owned()
        })
        .collect();
    assert_eq!(rendered, text);
    assert_eq!(
        buffer[(geometry.text.x, geometry.text.y)].fg,
        Color::Rgb(220, 20, 60)
    );
    assert_eq!(
        buffer[(geometry.text.x + 3, geometry.text.y)].fg,
        Color::Rgb(20, 180, 90)
    );
    let composed = view.overlays.overlays(&theme);
    assert_eq!(composed.len(), 2);
    assert_eq!(
        composed[0].range,
        TextRange::new(ByteOffset(0), ByteOffset(3)).unwrap()
    );
    assert_eq!(
        composed[1].range,
        TextRange::new(ByteOffset(3), ByteOffset(8)).unwrap()
    );
}

#[test]
fn host_fold_marker_ignores_comment_braces_and_toggles_without_editing_source() {
    let text = "fn f() {\n  // } fake\n  work();\n}";
    let (mut documents, mut view, mut focus) = open_file(text, "host-fold.rs");
    let area = Rect::new(0, 0, 60, 10);
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    let report = kode_tui::editor::folding::discover(&snapshot, || false).unwrap();
    assert_eq!(report.regions.len(), 1);
    assert_eq!(report.regions[0].start, kode_core::LineIndex(0));
    assert_eq!(report.regions[0].end, kode_core::LineIndex(3));
    assert!(view.visuals.folds.publish(report, &snapshot));

    let mut buffer = ratatui::buffer::Buffer::empty(area);
    view.render(
        &documents,
        &focus,
        area,
        &mut buffer,
        &kode_tui::theme::Theme::compiled(),
        &[],
    )
    .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    assert_eq!(
        buffer[(geometry.gutter.right() - 1, geometry.text.y)].symbol(),
        "-"
    );
    let click_gutter = || {
        InputEvent::Pointer(PointerInput {
            x: geometry.gutter.x,
            y: geometry.text.y,
            action: PointerAction::Down(MouseButton::Left),
            modifiers: Modifiers::default(),
        })
    };
    assert!(
        view.dispatch(&click_gutter(), &mut documents, &mut focus, area)
            .unwrap()
    );
    assert_eq!(view.visuals.folds.collapsed(&snapshot).len(), 1);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    view.render(
        &documents,
        &focus,
        area,
        &mut buffer,
        &kode_tui::theme::Theme::compiled(),
        &[],
    )
    .unwrap();
    let summary: String = (geometry.text.x..geometry.text.right())
        .map(|x| buffer[(x, geometry.text.y)].symbol())
        .collect();
    assert!(summary.contains(" ... "), "{summary}");
    assert_eq!(
        buffer[(geometry.gutter.right() - 1, geometry.text.y)].symbol(),
        "+"
    );
    let collapsed = view.visuals.folds.collapsed(&snapshot);
    let layout = view
        .input
        .layout(
            documents.document(view.document_id()).unwrap(),
            kode_tui::editor::input::InputViewport {
                geometry,
                folds: &collapsed,
            },
        )
        .unwrap();
    let marker = layout.rows[0].summary.as_ref().unwrap().marker.start;
    let click_ellipsis = InputEvent::Pointer(PointerInput {
        x: geometry.text.x + marker as u16,
        y: geometry.text.y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&click_ellipsis, &mut documents, &mut focus, area)
            .unwrap()
    );
    assert!(view.visuals.folds.collapsed(&snapshot).is_empty());
    assert_eq!(source(&documents, &view), text);
}

#[test]
fn indentation_fold_reports_publish_and_render_through_file_hosts() {
    let cases = [
        (
            "python",
            "outer:\n    inner:\n\t\twork\n\n    next\nend",
            vec![(0, 4), (1, 3)],
        ),
        (
            "yaml",
            "outer:\n    inner:\n\t\twork\n\n    next\nend",
            vec![(0, 4), (1, 3)],
        ),
        (
            "bash",
            "outer:\n    inner:\n\t\twork\n\n    next\nend",
            vec![(0, 4), (1, 3)],
        ),
        (
            "powershell",
            "outer:\n    inner:\n\t\twork\n\n    next\nend",
            vec![(0, 4), (1, 3)],
        ),
        (
            "ruby",
            "class X\n  def f\n    work\n  end\nend",
            vec![(0, 3), (1, 2)],
        ),
    ];
    let area = Rect::new(0, 0, 60, 10);
    for (language, text, expected) in cases {
        let (documents, mut view, focus) =
            open_file_with_language(text, language, &format!("indent-fold-{language}"));
        let snapshot = documents.document(view.document_id()).unwrap().snapshot();
        let report = kode_tui::editor::folding::discover(&snapshot, || false).unwrap();
        assert_eq!(
            report
                .regions
                .iter()
                .map(|region| (region.start.0, region.end.0))
                .collect::<Vec<_>>(),
            expected,
            "{language}"
        );
        assert!(view.visuals.folds.publish(report, &snapshot), "{language}");
        assert!(
            view.visuals.folds.toggle(kode_core::LineIndex(0)),
            "{language}"
        );
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        view.render(
            &documents,
            &focus,
            area,
            &mut buffer,
            &kode_tui::theme::Theme::compiled(),
            &[],
        )
        .unwrap();
        let geometry = view.geometry(&documents, area).unwrap();
        let summary: String = (geometry.text.x..geometry.text.right())
            .map(|x| buffer[(x, geometry.text.y)].symbol())
            .collect();
        assert!(summary.contains(" ... "), "{language}: {summary}");
        assert_eq!(source(&documents, &view), text, "{language}");
    }
}

#[test]
fn navigation_into_hidden_host_line_expands_fold_and_keeps_selection() {
    let text = "fn f() {\n  first();\n  second();\n}";
    let (mut documents, mut view, _focus) = open_file(text, "navigate-folded.rs");
    let snapshot = documents.document(view.document_id()).unwrap().snapshot();
    let report = kode_tui::editor::folding::discover(&snapshot, || false).unwrap();
    assert!(view.visuals.folds.publish(report, &snapshot));
    assert!(view.visuals.folds.toggle(kode_core::LineIndex(0)));
    assert_eq!(view.visuals.folds.collapsed(&snapshot).len(), 1);

    let target = text.find("second").unwrap();
    view.command(
        EditorCommand::MoveTo {
            position: ByteOffset(target),
            extend: true,
        },
        &mut documents,
        Rect::new(0, 0, 60, 10),
    )
    .unwrap();
    assert!(view.visuals.folds.collapsed(&snapshot).is_empty());
    assert_eq!(
        view.input.state.caret.selection,
        Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(target),
        }
    );
    assert_eq!(source(&documents, &view), text);
}

#[test]
fn editing_host_invalidates_fold_anchors_and_rejects_late_discovery() {
    let text = "fn f() {\n  first();\n  second();\n}";
    let (mut documents, mut view, _focus) = open_file(text, "edit-folded.rs");
    let area = Rect::new(0, 0, 60, 10);
    let (old_tag, old_snapshot) = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            EditorWork::Folds { tag, snapshot } => Some((tag, snapshot)),
            _ => None,
        })
        .expect("editor host queues fold discovery");
    let old_report = kode_tui::editor::folding::discover(&old_snapshot, || false).unwrap();
    assert!(view.visuals.folds.publish(old_report, &old_snapshot));
    assert!(view.visuals.folds.toggle(kode_core::LineIndex(0)));

    let start = text.find("first").unwrap();
    view.programmatic_replace(
        TextRange::new(ByteOffset(start), ByteOffset(start + "first".len())).unwrap(),
        "changed",
        &mut documents,
        area,
    )
    .unwrap();
    let current = documents.document(view.document_id()).unwrap().snapshot();
    assert_eq!(source(&documents, &view), text.replace("first", "changed"));
    assert!(view.visuals.folds.regions().next().is_none());
    assert!(view.visuals.folds.collapsed(&current).is_empty());
    let late_report = kode_tui::editor::folding::discover(&old_snapshot, || false).unwrap();
    assert!(
        !view
            .complete_folds(old_tag, Ok(late_report), &documents, area)
            .unwrap()
    );
    assert_eq!(source(&documents, &view), text.replace("first", "changed"));
}

#[test]
fn host_minimap_click_moves_to_block_and_disappears_when_too_narrow() {
    let text = (0..100)
        .map(|line| format!("line_{line} = value"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut documents, mut view, mut focus) = open_file(&text, "minimap-host.txt");
    let area = Rect::new(0, 0, 80, 14);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    let theme = kode_tui::theme::Theme::compiled();
    view.render(&documents, &focus, area, &mut buffer, &theme, &[])
        .unwrap();
    let geometry = view.geometry(&documents, area).unwrap();
    assert!(geometry.minimap.width > 0);
    assert_eq!(
        buffer[(geometry.minimap.x, geometry.minimap.y)].bg,
        theme.style("code-selection").bg.unwrap()
    );
    let target = (geometry.minimap.x, geometry.minimap.y + 7);
    let click = InputEvent::Pointer(PointerInput {
        x: target.0,
        y: target.1,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    assert!(
        view.dispatch(&click, &mut documents, &mut focus, area)
            .unwrap()
    );
    let moved = documents.document(view.document_id()).unwrap().snapshot();
    assert_eq!(
        moved
            .byte_to_position(view.input.state.caret.cursor())
            .unwrap()
            .line,
        kode_core::LineIndex(28)
    );
    assert!(view.input.state.scroll_rows > 0);
    assert_eq!(source(&documents, &view), text);

    let narrow = Rect::new(0, 0, 39, 8);
    let narrow_geometry = view.geometry(&documents, narrow).unwrap();
    assert_eq!(narrow_geometry.minimap.width, 0);
    let mut narrow_buffer = ratatui::buffer::Buffer::empty(narrow);
    view.render(&documents, &focus, narrow, &mut narrow_buffer, &theme, &[])
        .unwrap();
    assert_eq!(source(&documents, &view), text);
}

#[test]
fn unfocused_editor_keeps_its_view_state_and_does_not_receive_keys() {
    let (mut documents, mut view, mut focus) = open_file("draft", "focus-owner.txt");
    let area = Rect::new(0, 0, 30, 8);
    view.input
        .state
        .caret
        .move_to(
            &documents.document(view.document_id()).unwrap().snapshot(),
            ByteOffset(2),
            false,
        )
        .unwrap();
    focus.focused = Some(FocusId(view.input.id.0 + 1));
    assert!(!view.input.render_state(&focus).focused);
    let caret = view.input.state.caret.clone();
    assert!(
        !view
            .dispatch(
                &event(Key::Character('x')),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
    );
    assert_eq!(view.input.state.caret, caret);
    assert_eq!(source(&documents, &view), "draft");

    focus.focused = Some(view.input.id);
    assert!(view.input.render_state(&focus).focused);
    assert!(
        view.dispatch(
            &event(Key::Character('y')),
            &mut documents,
            &mut focus,
            area,
        )
        .unwrap()
    );
    assert_eq!(source(&documents, &view), "dryaft");
}

fn replay_modal_focus(sequence: &Value) {
    let (documents, view, focus) = open_file("editor remains unchanged", "modal-focus.txt");
    let original_focus = focus.focused;
    let original_source = source(&documents, &view);
    let area = Rect::new(0, 0, 80, 24);
    let mut dialog: Option<TextDialog> = None;
    for fixture_event in sequence["events"].as_array().unwrap() {
        match fixture_event.as_str().unwrap() {
            "open-dialog" => {
                let mut opened = TextDialog::new("Name".into(), String::new());
                opened.render(
                    area,
                    &mut ratatui::buffer::Buffer::empty(area),
                    &kode_tui::theme::Theme::compiled(),
                );
                dialog = Some(opened);
            }
            "type:a" => dialog.as_mut().unwrap().handle(&event(Key::Character('a'))),
            "tab" => dialog.as_mut().unwrap().handle(&event(Key::Tab)),
            "escape" => dialog.as_mut().unwrap().handle(&event(Key::Escape)),
            other => panic!("unsupported modal fixture event: {other}"),
        }
    }
    let dialog = dialog.unwrap();
    assert_eq!(dialog.field.value, "a");
    assert_eq!(dialog.focus, 2);
    assert_eq!(dialog.action, Some(DialogAction::Dismissed));
    assert_eq!(focus.focused, original_focus);
    assert_eq!(source(&documents, &view), original_source);
}

#[test]
fn baseline_newline_and_reverse_selection_sequences_replay_through_editor_host() {
    let sequences: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../fixtures/editor/event-sequences.json"
    ))
    .unwrap();
    let mut replayed = 0;
    for sequence in &sequences {
        match sequence["name"].as_str().unwrap() {
            "newline-aliases" => replay_newline_aliases(sequence),
            "boundary-delete-undo" => replay_boundary_deletes(sequence),
            "logical-arrow-movement" => replay_logical_movement(sequence),
            "line-and-word-navigation" => replay_line_word_navigation(sequence),
            "source-scalar-movement" => replay_source_scalar_movement(sequence),
            "select-all-multiline" | "select-all-empty" => replay_select_all(sequence),
            "undo-redo-history-branch" | "empty-history-at-eof" => replay_history(sequence),
            "reverse-selection-paste" => replay_reverse_selection_paste(sequence),
            "modal-focus" => replay_modal_focus(sequence),
            "popup-outside-click" => replay_popup_outside_click(),
            "wrapped-pointer-selection" => replay_wrapped_pointer_selection(sequence),
            other => panic!("editor event fixture has no replay: {other}"),
        }
        replayed += 1;
    }
    assert_eq!(replayed, sequences.len());
}

#[test]
fn the_same_reverse_paste_undo_redo_sequence_runs_through_all_editable_hosts() {
    let original = "abc\ndef";
    let replacement = "X\nY";
    let expected = "aX\nYf";
    let selection = Selection {
        anchor: ByteOffset(6),
        head: ByteOffset(1),
    };
    let area = Rect::new(0, 0, 80, 24);
    for role in [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ] {
        let key = format!("editable-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, original, &key);
        view.input.state.caret.selection = selection;
        assert!(
            view.dispatch(
                &InputEvent::Paste(replacement.into()),
                &mut documents,
                &mut focus,
                area,
            )
            .unwrap()
        );
        assert_eq!(source(&documents, &view), expected, "{role:?}");
        view.command(EditorCommand::Undo, &mut documents, area)
            .unwrap();
        assert_eq!(source(&documents, &view), original, "{role:?}");
        assert_eq!(view.input.state.caret.selection, selection, "{role:?}");
        view.command(EditorCommand::Redo, &mut documents, area)
            .unwrap();
        assert_eq!(source(&documents, &view), expected, "{role:?}");
        assert_eq!(
            view.input.state.caret.cursor(),
            ByteOffset(selection.range().start.0 + replacement.len()),
            "{role:?}"
        );
    }

    for role in [
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ] {
        let key = format!("readonly-{role:?}");
        let (mut documents, mut view, mut focus) = open_host(role, original, &key);
        view.input.state.caret.selection = selection;
        assert!(matches!(
            view.dispatch(
                &InputEvent::Paste(replacement.into()),
                &mut documents,
                &mut focus,
                area,
            ),
            Err(DomainError::ReadOnly)
        ));
        assert_eq!(source(&documents, &view), original, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}

#[test]
fn clipboard_shortcuts_roundtrip_through_all_editable_hosts_with_internal_fallback() {
    let roles = [
        HostRole::File,
        HostRole::CommitMessage,
        HostRole::ProjectSearch,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
    ];
    let area = Rect::new(0, 0, 80, 24);
    let shortcut = |ch: char, alt: bool| {
        InputEvent::Key(KeyInput {
            key: Key::Character(ch),
            modifiers: Modifiers {
                ctrl: !alt,
                alt,
                ..Default::default()
            },
            repeat: false,
        })
    };

    for (index, source_role) in roles.iter().copied().enumerate() {
        let target_role = roles[(index + 1) % roles.len()];
        let clipboard = Arc::new(ClipboardService::internal());

        let key = format!("clipboard-copy-source-{source_role:?}");
        let (mut source_documents, mut source_view, mut source_focus) =
            open_host_with_clipboard(source_role, "copy me", &key, clipboard.clone());
        source_view
            .command(EditorCommand::SelectAll, &mut source_documents, area)
            .unwrap();
        assert!(
            source_view
                .dispatch(
                    &shortcut('c', index % 2 == 1),
                    &mut source_documents,
                    &mut source_focus,
                    area,
                )
                .unwrap()
        );
        assert_eq!(clipboard.local_text(), "copy me", "{source_role:?}");

        let key = format!("clipboard-copy-target-{target_role:?}");
        let (mut target_documents, mut target_view, mut target_focus) =
            open_host_with_clipboard(target_role, "@", &key, clipboard.clone());
        target_view
            .command(
                EditorCommand::MoveTo {
                    position: ByteOffset(1),
                    extend: false,
                },
                &mut target_documents,
                area,
            )
            .unwrap();
        assert!(
            target_view
                .dispatch(
                    &shortcut('v', index % 2 == 0),
                    &mut target_documents,
                    &mut target_focus,
                    area,
                )
                .unwrap()
        );
        let paste = target_view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::ClipboardRead(target) => Some(target),
                _ => None,
            })
            .expect("paste schedules a clipboard read");
        let local_result = kode_core::services::Clipboard::read(clipboard.as_ref());
        assert!(
            target_view
                .complete_clipboard(
                    &paste,
                    local_result,
                    &mut target_documents,
                    &target_focus,
                    area,
                )
                .unwrap()
        );
        assert_eq!(
            source(&target_documents, &target_view),
            "@copy me",
            "{target_role:?}"
        );

        let key = format!("clipboard-cut-source-{source_role:?}");
        let (mut cut_documents, mut cut_view, mut cut_focus) =
            open_host_with_clipboard(source_role, "cut me", &key, clipboard.clone());
        cut_view
            .command(EditorCommand::SelectAll, &mut cut_documents, area)
            .unwrap();
        assert!(
            cut_view
                .dispatch(
                    &shortcut('x', index % 2 == 0),
                    &mut cut_documents,
                    &mut cut_focus,
                    area,
                )
                .unwrap()
        );
        assert_eq!(source(&cut_documents, &cut_view), "", "{source_role:?}");
        assert_eq!(clipboard.local_text(), "cut me", "{source_role:?}");

        let key = format!("clipboard-cut-target-{target_role:?}");
        let (mut target_documents, mut target_view, mut target_focus) =
            open_host_with_clipboard(target_role, "", &key, clipboard.clone());
        assert!(
            target_view
                .dispatch(
                    &shortcut('v', index % 2 == 1),
                    &mut target_documents,
                    &mut target_focus,
                    area,
                )
                .unwrap()
        );
        let paste = target_view
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                EditorWork::ClipboardRead(target) => Some(target),
                _ => None,
            })
            .expect("paste schedules a clipboard read");
        let local_result = kode_core::services::Clipboard::read(clipboard.as_ref());
        assert!(
            target_view
                .complete_clipboard(
                    &paste,
                    local_result,
                    &mut target_documents,
                    &target_focus,
                    area,
                )
                .unwrap()
        );
        assert_eq!(
            source(&target_documents, &target_view),
            "cut me",
            "{target_role:?}"
        );
    }
}

#[test]
fn virtual_save_never_treats_host_label_as_file_path() {
    let directory = tempfile::tempdir().unwrap();
    let roles = [
        HostRole::CommitMessage,
        HostRole::Sql,
        HostRole::RestBody,
        HostRole::RestHeaders,
        HostRole::Environment,
        HostRole::Response,
        HostRole::RawRequest,
        HostRole::Historical,
    ];
    let area = Rect::new(0, 0, 80, 24);

    for role in roles {
        let label_path = directory.path().join(format!("{role:?}.sql"));
        let key = label_path.to_string_lossy().into_owned();
        let initial = "unchanged virtual source";
        let (mut documents, mut view, mut focus) = open_host(role, initial, &key);
        let save = InputEvent::Key(KeyInput {
            key: Key::Character('s'),
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        });
        let result = view.dispatch(&save, &mut documents, &mut focus, area);
        if matches!(
            role,
            HostRole::Response | HostRole::RawRequest | HostRole::Historical
        ) {
            assert!(matches!(result, Err(DomainError::ReadOnly)), "{role:?}");
        } else {
            assert!(matches!(result, Err(DomainError::NoSaveTarget)), "{role:?}");
        }
        assert!(!label_path.exists(), "{role:?} wrote its display label");
        assert_eq!(source(&documents, &view), initial, "{role:?}");
        assert!(!documents.document(view.document_id()).unwrap().is_dirty());
    }
}
