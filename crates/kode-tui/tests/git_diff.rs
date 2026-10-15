use kode_core::{WorkspaceGeneration, services::Clipboard};
use kode_git::{diff::DiffPlan, model::*};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock},
};
use kode_tui::{
    editor::host::EditorDocuments,
    focus::FocusState,
    input::terminal::*,
    panels::git::diff::{DiffViewer, DiffWork, projection::Projection},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::sync::Arc;
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_millis(&self) -> u64 {
        0
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
fn content(text: &str) -> Content {
    Content {
        kind: ContentKind::Text,
        bytes: text.as_bytes().into(),
        length: text.len() as u64,
        object: None,
        mode: 0o100644,
    }
}
fn plan(staged: bool) -> Arc<DiffPlan> {
    Arc::new(
        DiffPlan::build(
            DiffContents {
                path: "file.txt".into(),
                staged,
                revision: None,
                old: content("first\ncontext\nlast\n"),
                new: content("changed and wrapped 😀\ncontext\nLAST"),
            },
            &CancellationToken::new(),
        )
        .unwrap(),
    )
}
fn layout(view: &mut DiffViewer, documents: &mut EditorDocuments, focus: &mut FocusState) {
    let work = view.take_work();
    for work in work {
        if let DiffWork::Layout { tag, plan, key, .. } = work {
            let projection = Projection::build(
                &plan,
                key.width,
                key.context,
                key.unified,
                &CancellationToken::new(),
            );
            view.complete_layout(tag, projection, documents, focus)
                .unwrap();
        }
    }
}
#[test]
fn preview_uses_common_readonly_selection_copy_search_and_worker_context_reflow_with_no_paint_io() {
    let clipboard = Arc::new(ClipboardService::internal());
    let mut documents = EditorDocuments::default();
    let mut focus = FocusState::default();
    let mut view = DiffViewer::new(
        "/nonexistent/fixture".into(),
        plan(false),
        WorkspaceGeneration(1),
        clipboard.clone(),
        Arc::new(ZeroClock),
    )
    .unwrap();
    focus.focused = Some(view.id());
    view.resize(Rect::new(8, 4, 40, 12));
    layout(&mut view, &mut documents, &mut focus);
    let id = view.editor.as_ref().unwrap().document_id();
    let original = documents
        .document(id)
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    assert!(original.contains("context"));
    assert!(original.contains("\\x1F600"));
    assert!(original.contains("[no new"));
    assert!(documents.document(id).unwrap().is_read_only());
    view.dispatch(&key(Key::Character('a'), true), &mut documents, &mut focus)
        .unwrap();
    view.dispatch(&key(Key::Character('c'), true), &mut documents, &mut focus)
        .unwrap();
    assert_eq!(clipboard.read().unwrap(), original);
    for input in [
        InputEvent::Paste("forbidden".into()),
        key(Key::Backspace, false),
        key(Key::Delete, false),
        key(Key::Character('x'), true),
        key(Key::Character('v'), true),
        key(Key::Character('z'), true),
        key(Key::Character('s'), true),
    ] {
        view.dispatch(&input, &mut documents, &mut focus).unwrap();
    }
    assert_eq!(
        documents.document(id).unwrap().snapshot().text().as_ref(),
        original
    );
    let start = original.find("changed").unwrap();
    let snapshot = documents.document(id).unwrap().snapshot();
    view.editor
        .as_mut()
        .unwrap()
        .input
        .state
        .caret
        .set_selection(
            &snapshot,
            kode_core::Selection {
                anchor: kode_core::ByteOffset(start + 7),
                head: kode_core::ByteOffset(start),
            },
        )
        .unwrap();
    view.resize(Rect::new(8, 4, 60, 12));
    layout(&mut view, &mut documents, &mut focus);
    let editor = view.editor.as_ref().unwrap();
    let snapshot = documents.document(editor.document_id()).unwrap().snapshot();
    assert_eq!(
        editor.input.state.caret.selected_text(&snapshot).unwrap(),
        "changed"
    );
    assert!(editor.input.state.caret.selection.is_reverse());
    assert!(documents.document(id).is_err());
    view.dispatch(&key(Key::Character('f'), true), &mut documents, &mut focus)
        .unwrap();
    assert!(view.editor.as_ref().unwrap().input.state.search_has_focus);
    view.dispatch(&key(Key::Character('c'), false), &mut documents, &mut focus)
        .unwrap();
    assert!(view.context, "find field owns c");
    view.dispatch(&key(Key::Escape, false), &mut documents, &mut focus)
        .unwrap();
    view.dispatch(&key(Key::Character('c'), false), &mut documents, &mut focus)
        .unwrap();
    layout(&mut view, &mut documents, &mut focus);
    assert!(!view.context);
    let id = view.editor.as_ref().unwrap().document_id();
    assert!(
        !documents
            .document(id)
            .unwrap()
            .snapshot()
            .text()
            .contains("context")
    );
    view.dispatch(&key(Key::Character('v'), false), &mut documents, &mut focus)
        .unwrap();
    layout(&mut view, &mut documents, &mut focus);
    assert!(view.unified);
    let text = documents
        .document(view.editor.as_ref().unwrap().document_id())
        .unwrap()
        .snapshot()
        .text()
        .to_string();
    assert!(text.starts_with("-1 first"));
    assert!(text.contains("+1 changed"));
    for area in [
        Rect::new(8, 4, 40, 12),
        Rect::new(3, 2, 17, 6),
        Rect::new(0, 0, 1, 1),
        Rect::default(),
    ] {
        view.resize(area);
        layout(&mut view, &mut documents, &mut focus);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 30));
        view.render(area, &documents, &focus, &mut buffer, &Theme::compiled())
            .unwrap();
        assert_eq!(focus.focused, Some(view.id()));
    }
}
#[test]
fn exact_visible_chunk_restore_is_disabled_for_staged_and_late_layout_cannot_win_after_resize() {
    let mut documents = EditorDocuments::default();
    let mut focus = FocusState::default();
    let mut view = DiffViewer::new(
        "/fixture".into(),
        plan(false),
        WorkspaceGeneration(1),
        Arc::new(ClipboardService::internal()),
        Arc::new(ZeroClock),
    )
    .unwrap();
    focus.focused = Some(view.id());
    view.resize(Rect::new(4, 3, 60, 15));
    let old = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            DiffWork::Layout { tag, plan, key, .. } => Some((tag, plan, key)),
            _ => None,
        })
        .unwrap();
    view.resize(Rect::new(4, 3, 30, 15));
    layout(&mut view, &mut documents, &mut focus);
    let current = view.editor.as_ref().unwrap().document_id();
    view.complete_layout(
        old.0,
        Projection::build(
            &old.1,
            old.2.width,
            old.2.context,
            old.2.unified,
            &CancellationToken::new(),
        ),
        &mut documents,
        &mut focus,
    )
    .unwrap();
    assert_eq!(view.editor.as_ref().unwrap().document_id(), current);
    view.dispatch(&key(Key::Enter, false), &mut documents, &mut focus)
        .unwrap();
    let restores = view
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            DiffWork::Restore { job, .. } => Some(job),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(restores.len(), 1);
    assert_eq!(restores[0].hunk, 0);
    view.replace_plan(plan(true));
    layout(&mut view, &mut documents, &mut focus);
    view.restore_busy = None;
    view.dispatch(&key(Key::Enter, false), &mut documents, &mut focus)
        .unwrap();
    assert!(
        !view
            .take_work()
            .iter()
            .any(|work| matches!(work, DiffWork::Restore { .. }))
    );
    assert!(view.notice.contains("only for unstaged"));
    // A new capture at the same width must supersede an in-flight projection.
    view.resize(Rect::new(4, 3, 45, 15));
    let old = view
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            DiffWork::Layout { tag, plan, key, .. } => Some((tag, plan, key)),
            _ => None,
        })
        .unwrap();
    let mut contents = plan(false).contents().clone();
    contents.new = content("LATEST\n");
    view.replace_plan(Arc::new(
        DiffPlan::build(contents, &CancellationToken::new()).unwrap(),
    ));
    view.complete_layout(
        old.0,
        Projection::build(
            &old.1,
            old.2.width,
            old.2.context,
            old.2.unified,
            &CancellationToken::new(),
        ),
        &mut documents,
        &mut focus,
    )
    .unwrap();
    layout(&mut view, &mut documents, &mut focus);
    assert!(
        documents
            .document(view.editor.as_ref().unwrap().document_id())
            .unwrap()
            .snapshot()
            .text()
            .contains("LATEST")
    );
}
#[test]
fn projection_bounds_and_nontext_notice_are_explicit_and_each_wrapped_row_fits_the_cell_width() {
    let plan = plan(false);
    for width in [0, 1, 16, 17, 18, 40, 100] {
        let projection =
            Projection::build(&plan, width, true, false, &CancellationToken::new()).unwrap();
        assert!(
            projection
                .text
                .lines()
                .all(|line| line.chars().count() <= usize::from(width.max(1)))
        );
        assert_eq!(projection.rows.len(), projection.text.lines().count());
    }
    let token = CancellationToken::new();
    token.cancel();
    assert!(Projection::build(&plan, 30, true, false, &token).is_err());
    let mut contents = plan.contents().clone();
    contents.new = content(&"x".repeat(100_001));
    let long = DiffPlan::build(contents, &CancellationToken::new()).unwrap();
    assert!(Projection::build(&long, 1, true, false, &CancellationToken::new()).is_err());
}
