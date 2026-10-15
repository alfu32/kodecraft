use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag,
    services::{Clipboard, DocumentIo},
};
use kode_platform::{
    clipboard::ClipboardService,
    files::NativeDocumentIo,
    jobs::{CancellationToken, Clock},
};
use kode_tui::{
    editor::{
        host::{
            ClipboardJob, ClipboardResult, EditorDocuments, EditorView, EditorWork, HostDescriptor,
            HostRole, QueuedEditorHost,
        },
        loading::prepare,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    panels::project_search::{ProjectSearchDialog, ProjectSearchWork, SearchFocus},
    theme::Theme,
};
use kode_workspace::{mime::MimeDetector, project::Project, search::search};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
const AREA: Rect = Rect::new(4, 3, 100, 35);
#[derive(Default)]
struct FakeClock(AtomicU64);
impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn key(key: Key, ctrl: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl,
            ..Default::default()
        },
        repeat: false,
    })
}
fn mouse(point: Position, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x: point.x,
        y: point.y,
        action,
        modifiers: Default::default(),
    })
}
fn put(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn dialog(
    root: &Path,
    documents: &EditorDocuments,
    focus: &mut FocusState,
    clipboard: Arc<ClipboardService>,
    clock: Arc<FakeClock>,
) -> ProjectSearchDialog {
    ProjectSearchDialog::new(
        Project::new(root, kode_core::WorkspaceGeneration(1)).unwrap(),
        "needle".into(),
        String::new(),
        MimeDetector::compiled(),
        clipboard,
        clock,
        documents,
        focus,
    )
    .unwrap()
}
fn run_search(dialog: &mut ProjectSearchDialog) {
    let jobs = dialog.take_work();
    for work in jobs {
        if let ProjectSearchWork::Search(job) = work {
            let result = search(&job, &CancellationToken::new());
            assert!(dialog.complete_search(job.tag, result).unwrap());
        }
    }
}
fn attach_selected(
    dialog: &mut ProjectSearchDialog,
    documents: &mut EditorDocuments,
    focus: &mut FocusState,
    clipboard: Arc<ClipboardService>,
    clock: Arc<FakeClock>,
) -> JobTag {
    let (tag, hit) = dialog
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            ProjectSearchWork::OpenHit { tag, hit } => Some((tag, hit)),
            _ => None,
        })
        .expect("real selected hit");
    let origin = DocumentOrigin::File(hit.path.clone());
    let descriptor = HostDescriptor::new(
        HostRole::ProjectSearch,
        "project-search".into(),
        "preview".into(),
        "preview".into(),
        dialog.project.root.clone(),
    );
    let id = match documents.find_origin(&origin) {
        Some(id) => id,
        None => {
            let loaded = NativeDocumentIo.load(&origin, tag).unwrap();
            let prepared =
                prepare(loaded, DocumentId::fresh().unwrap(), hit.language.clone()).unwrap();
            documents
                .insert_file(prepared, &descriptor, dialog.project.generation)
                .unwrap()
        }
    };
    let view = EditorView::new(
        id,
        documents,
        QueuedEditorHost::new(descriptor),
        dialog.project.generation,
        clipboard,
        clock,
    )
    .unwrap();
    assert!(dialog.attach_editor(tag, view, documents, focus).unwrap());
    tag
}
fn paint(
    dialog: &mut ProjectSearchDialog,
    documents: &EditorDocuments,
    focus: &FocusState,
) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 50));
    dialog
        .render(AREA, &mut buffer, &Theme::compiled(), documents, focus)
        .unwrap();
    buffer
}
#[test]
fn modal_prefill_filter_rows_navigation_focus_and_preview_center_use_real_document_identity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let text = (0..80)
        .map(|index| format!("line {index}{}\n", if index == 60 { " needle" } else { "" }))
        .collect::<String>();
    put(root, "src/one.rs", &text);
    put(root, "src/two.rs", "needle second");
    let mut documents = EditorDocuments::default();
    let previous = FocusId(999);
    let mut focus = FocusState {
        focused: Some(previous),
        ..Default::default()
    };
    let clipboard = Arc::new(ClipboardService::internal());
    let clock = Arc::new(FakeClock::default());
    let mut dialog = dialog(
        root,
        &documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    run_search(&mut dialog);
    assert_eq!(dialog.hits.len(), 2);
    assert_eq!(dialog.focused, SearchFocus::Query);
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    paint(&mut dialog, &documents, &focus);
    assert!(dialog.editor.as_ref().unwrap().input.state.scroll_rows > 0);
    let id = dialog.editor.as_ref().unwrap().document_id();
    assert_eq!(documents.ids().count(), 1);
    for _ in 0..3 {
        dialog
            .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
            .unwrap();
    }
    assert_eq!(dialog.focused, SearchFocus::List);
    dialog
        .dispatch(&key(Key::Down, false), &mut documents, &mut focus)
        .unwrap();
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    assert_ne!(dialog.editor.as_ref().unwrap().document_id(), id);
    assert_eq!(documents.ids().count(), 2);
    dialog
        .dispatch(&key(Key::Home, false), &mut documents, &mut focus)
        .unwrap();
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    assert_eq!(dialog.editor.as_ref().unwrap().document_id(), id);
    let prefill = ProjectSearchDialog::prefill("a+b😀", Some(Path::new("sample.rs")));
    assert_eq!(prefill.0, "a\\+b😀");
    assert_eq!(prefill.1, "\\.rs");
    dialog
        .dispatch(&key(Key::Escape, false), &mut documents, &mut focus)
        .unwrap();
    assert!(dialog.finished);
    assert_eq!(focus.focused, Some(previous));
}
#[test]
fn preview_edits_refresh_matches_after_debounce_and_ctrl_s_saves_while_fields_have_focus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "one.rs", "needle first\nneedle second\n");
    let mut documents = EditorDocuments::default();
    let mut focus = FocusState::default();
    let clipboard = Arc::new(ClipboardService::internal());
    let clock = Arc::new(FakeClock::default());
    let mut dialog = dialog(
        root,
        &documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    run_search(&mut dialog);
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    paint(&mut dialog, &documents, &focus);
    let id = dialog.editor.as_ref().unwrap().document_id();
    for _ in 0..4 {
        dialog
            .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
            .unwrap();
    }
    assert_eq!(dialog.focused, SearchFocus::Editor);
    dialog
        .dispatch(
            &InputEvent::Paste("prefix needle".into()),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(documents.document(id).unwrap().is_dirty());
    assert!(
        !dialog
            .take_work()
            .iter()
            .any(|work| matches!(work, ProjectSearchWork::Search(_)))
    );
    clock.0.store(149, Ordering::SeqCst);
    dialog.tick(&documents, &focus).unwrap();
    assert!(
        !dialog
            .take_work()
            .iter()
            .any(|work| matches!(work, ProjectSearchWork::Search(_)))
    );
    clock.0.store(150, Ordering::SeqCst);
    dialog.tick(&documents, &focus).unwrap();
    run_search(&mut dialog);
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    assert_eq!(dialog.editor.as_ref().unwrap().document_id(), id);
    assert_eq!(dialog.hits[0].range.start.0, 7);
    assert!(documents.document(id).unwrap().is_dirty());
    dialog
        .dispatch(&key(Key::Tab, true), &mut documents, &mut focus)
        .unwrap();
    assert_eq!(dialog.focused, SearchFocus::Query);
    dialog
        .dispatch(&key(Key::Character('s'), true), &mut documents, &mut focus)
        .unwrap();
    let job = dialog
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            ProjectSearchWork::Editor {
                work: EditorWork::SaveFile(job),
                ..
            } => Some(job),
            _ => None,
        })
        .unwrap();
    let DocumentOrigin::File(path) = &job.snapshot.origin else {
        panic!("physical save");
    };
    let identity = NativeDocumentIo
        .save(
            path,
            &job.snapshot.to_service_snapshot(),
            job.expected_disk.as_ref(),
        )
        .unwrap();
    documents
        .complete_file_save(id, job.tag, Ok(identity))
        .unwrap();
    assert!(!documents.document(id).unwrap().is_dirty());
    assert!(
        fs::read_to_string(path)
            .unwrap()
            .starts_with("prefix needle")
    );
    // No global bulk replacement is invented by the replacement field.
    dialog
        .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
        .unwrap();
    dialog
        .dispatch(
            &InputEvent::Paste("replacement".into()),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(
        documents.document(id).unwrap().snapshot().text().as_ref(),
        "prefix needle first\nneedle second\n"
    );
}
#[test]
fn field_clipboard_tickets_paste_focus_changes_stale_search_and_failed_load_are_guarded() {
    let directory = tempfile::tempdir().unwrap();
    put(directory.path(), "one.txt", "needle");
    let documents = &mut EditorDocuments::default();
    let mut focus = FocusState::default();
    let clipboard = Arc::new(ClipboardService::internal());
    clipboard.remember("pasted").unwrap();
    let clock = Arc::new(FakeClock::default());
    let mut dialog = dialog(
        directory.path(),
        documents,
        &mut focus,
        clipboard.clone(),
        clock,
    );
    let old = dialog
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let ProjectSearchWork::Search(job) = work {
                Some(job)
            } else {
                None
            }
        })
        .unwrap();
    dialog
        .dispatch(&key(Key::Character('v'), true), documents, &mut focus)
        .unwrap();
    let tag = dialog
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let ProjectSearchWork::Clipboard {
                tag,
                job: ClipboardJob::Read,
            } = work
            {
                Some(tag)
            } else {
                None
            }
        })
        .unwrap();
    dialog
        .dispatch(&key(Key::Tab, false), documents, &mut focus)
        .unwrap();
    assert!(
        !dialog
            .complete_clipboard(tag, Ok(ClipboardResult::Read("late".into())), documents)
            .unwrap()
    );
    assert_eq!(dialog.query.value, "needle");
    dialog
        .dispatch(
            &InputEvent::Paste("replacement".into()),
            documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(dialog.replacement.value, "replacement");
    dialog
        .dispatch(&key(Key::Tab, false), documents, &mut focus)
        .unwrap();
    dialog
        .dispatch(&InputEvent::Paste("[".into()), documents, &mut focus)
        .unwrap();
    assert!(
        !dialog
            .complete_search(old.tag, search(&old, &CancellationToken::new()))
            .unwrap()
    );
    run_search(&mut dialog);
    assert!(dialog.error.is_some());
    dialog.filter.value.clear();
    dialog.begin_search(documents).unwrap();
    run_search(&mut dialog);
    let tag = dialog
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let ProjectSearchWork::OpenHit { tag, .. } = work {
                Some(tag)
            } else {
                None
            }
        })
        .unwrap();
    assert!(dialog.load_failed(tag, DomainError::MissingResource("file removed".into())));
    assert!(dialog.editor.is_none());
    assert!(dialog.error.unwrap().contains("file removed"));
}
#[test]
fn mouse_selection_divider_drag_release_scope_help_and_tiny_paint_stay_inside_modal() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..20 {
        put(directory.path(), &format!("{index:02}.txt"), "needle");
    }
    let mut documents = EditorDocuments::default();
    let mut focus = FocusState::default();
    let clipboard = Arc::new(ClipboardService::internal());
    let clock = Arc::new(FakeClock::default());
    let mut dialog = dialog(directory.path(), &documents, &mut focus, clipboard, clock);
    run_search(&mut dialog);
    paint(&mut dialog, &documents, &focus);
    let geometry = dialog.geometry;
    dialog
        .dispatch(
            &mouse(
                Position::new(geometry.list.x + 1, geometry.list.y + 1),
                PointerAction::Down(MouseButton::Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(dialog.selected, Some(1));
    dialog
        .dispatch(
            &mouse(
                Position::new(geometry.list.x, geometry.list.y),
                PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 1,
                },
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(dialog.scroll, 1);
    dialog
        .dispatch(
            &mouse(
                Position::new(geometry.divider.x, geometry.divider.y),
                PointerAction::Down(MouseButton::Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(dialog.splitter.captured());
    dialog
        .dispatch(
            &mouse(
                Position::new(geometry.divider.x, geometry.divider.y + 2),
                PointerAction::Drag(MouseButton::Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    dialog
        .dispatch(
            &mouse(Position::new(0, 0), PointerAction::Up(MouseButton::Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(!dialog.splitter.captured());
    dialog
        .dispatch(&key(Key::Function(1), false), &mut documents, &mut focus)
        .unwrap();
    paint(&mut dialog, &documents, &focus);
    dialog
        .dispatch(&key(Key::Escape, false), &mut documents, &mut focus)
        .unwrap();
    assert!(!dialog.finished);
    for width in 0..8 {
        for height in 0..8 {
            let area = Rect::new(3, 2, width, height);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 12));
            dialog
                .render(area, &mut buffer, &Theme::compiled(), &documents, &focus)
                .unwrap();
            for y in 0..12 {
                for x in 0..12 {
                    if !area.contains(Position::new(x, y)) {
                        assert_eq!(buffer[(x, y)].symbol(), " ");
                    }
                }
            }
        }
    }
}
#[test]
fn prepared_syntax_unicode_markers_mouse_copy_and_preview_clipboard_use_the_shared_host() {
    struct Styler;
    impl kode_workspace::search::SearchStyler for Styler {
        fn line_spans(
            &self,
            _: &Path,
            _: Option<&str>,
            _: &str,
        ) -> kode_core::Result<Vec<kode_core::intelligence::HighlightSpan>> {
            Ok(vec![kode_core::intelligence::HighlightSpan {
                range: kode_core::TextRange {
                    start: kode_core::ByteOffset(0),
                    end: kode_core::ByteOffset(3),
                },
                class: "keyword".into(),
            }])
        }
    }
    let directory = tempfile::tempdir().unwrap();
    put(directory.path(), "one.rs", "let 😀 needle");
    let mut documents = EditorDocuments::default();
    let mut focus = FocusState::default();
    let clipboard = Arc::new(ClipboardService::internal());
    let clock = Arc::new(FakeClock::default());
    let mut dialog = dialog(
        directory.path(),
        &documents,
        &mut focus,
        clipboard.clone(),
        clock.clone(),
    );
    dialog.styler = Some(Arc::new(Styler));
    dialog.begin_search(&documents).unwrap();
    run_search(&mut dialog);
    attach_selected(
        &mut dialog,
        &mut documents,
        &mut focus,
        clipboard.clone(),
        clock,
    );
    let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 50));
    let theme = Theme::parse(
        "default { fg:#ffffff; bg:#000000; } keyword { fg:#ff2030; } project-search-list-highlight { bg:#123456; }",
    );
    dialog
        .render(AREA, &mut buffer, &theme, &documents, &focus)
        .unwrap();
    assert_eq!(
        buffer[(dialog.geometry.list.x + 4, dialog.geometry.list.y)].fg,
        ratatui::style::Color::Rgb(255, 32, 48)
    );
    assert_eq!(
        buffer[(dialog.geometry.list.x + 8, dialog.geometry.list.y)].symbol(),
        "\\"
    );
    let view = dialog.editor.as_mut().unwrap();
    let geometry = view.geometry(&documents, dialog.geometry.editor).unwrap();
    let start = Position::new(geometry.text.x, geometry.text.y);
    dialog
        .dispatch(
            &mouse(start, PointerAction::Down(MouseButton::Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    dialog
        .dispatch(
            &mouse(
                Position::new(start.x + 3, start.y),
                PointerAction::Drag(MouseButton::Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    dialog
        .dispatch(
            &mouse(Position::new(0, 0), PointerAction::Up(MouseButton::Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(!dialog.finished);
    dialog
        .dispatch(&key(Key::Character('c'), true), &mut documents, &mut focus)
        .unwrap();
    assert_eq!(clipboard.read().unwrap(), "let");
    dialog
        .dispatch(&key(Key::Character('v'), true), &mut documents, &mut focus)
        .unwrap();
    let target = dialog
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            ProjectSearchWork::Editor {
                work: EditorWork::ClipboardRead(target),
                ..
            } => Some(target),
            _ => None,
        })
        .unwrap();
    let view = dialog.editor.as_mut().unwrap();
    assert!(
        view.complete_clipboard(
            &target,
            Ok("changed".into()),
            &mut documents,
            &focus,
            dialog.geometry.editor
        )
        .unwrap()
    );
    assert!(documents.document(view.document_id()).unwrap().is_dirty());
}
