use crossterm::event::MouseButton;

#[test]
fn name_dialog_native_clipboard_requests_are_scoped_to_current_value_caret_and_focus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut panel = FilesPanel::new(project(root)).unwrap();
    let mut focus = FocusState::default();
    refresh(&mut panel);
    click_label(&mut panel, "[+f]", AREA.y, &mut focus);
    let ctrl = |key| {
        InputEvent::Key(KeyInput {
            key,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            repeat: false,
        })
    };
    panel
        .dispatch(&ctrl(Key::Character('a')), &mut focus)
        .unwrap();
    panel
        .dispatch(&ctrl(Key::Character('v')), &mut focus)
        .unwrap();
    let tag = panel
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let FilesWork::ClipboardRead { tag } = work {
                Some(tag)
            } else {
                None
            }
        })
        .unwrap();
    assert!(panel.complete_name_clipboard(tag, Ok("clipboard name.rs".into()), &focus));
    panel
        .dispatch(&ctrl(Key::Character('a')), &mut focus)
        .unwrap();
    panel
        .dispatch(&ctrl(Key::Character('c')), &mut focus)
        .unwrap();
    assert!(
        matches!(&panel.take_work()[0], FilesWork::ClipboardCopy(text) if text == "clipboard name.rs")
    );
    panel
        .dispatch(&ctrl(Key::Character('v')), &mut focus)
        .unwrap();
    let tag = panel
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let FilesWork::ClipboardRead { tag } = work {
                Some(tag)
            } else {
                None
            }
        })
        .unwrap();
    panel
        .dispatch(&InputEvent::Focus(false), &mut focus)
        .unwrap();
    assert!(!panel.complete_name_clipboard(tag, Ok("late".into()), &focus));
    panel.dispatch(&key(Key::Enter), &mut focus).unwrap();
    assert!(
        matches!(&panel.take_work()[0], FilesWork::Action { action: FileAction::CreateFile { name, .. }, .. } if name == "clipboard name.rs")
    );
}
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, TextRange,
    WorkspaceGeneration, commands::EditorCommand, services::DocumentIo,
};
use kode_platform::{
    clipboard::ClipboardService,
    files::NativeDocumentIo,
    jobs::{CancellationToken, MonotonicClock, Progress},
};
use kode_tui::{
    editor::{
        host::{
            EditorDocuments, EditorView, EditorWork, HostDescriptor, HostRole, QueuedEditorHost,
        },
        loading::prepare,
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    panels::{
        files::{FilesPanel, FilesWork, RecentEntry},
        project::{ProjectPanel, ProjectWork, RescanDialog},
    },
    theme::Theme,
    widgets::folder_picker::{FolderPicker, PickerWork},
};
use kode_workspace::{
    project::Project,
    tree::{FileAction, FileChange, perform_action, read_tree},
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{fs, path::Path, sync::Arc};
const AREA: Rect = Rect::new(7, 4, 60, 16);
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn mouse(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Modifiers::default(),
    })
}
fn project(root: &Path) -> Project {
    Project::new(root, WorkspaceGeneration(1)).unwrap()
}
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn put(root: &Path, name: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "disk").unwrap();
}
fn refresh(panel: &mut FilesPanel) {
    panel.refresh().unwrap();
    for work in panel.take_work() {
        if let FilesWork::Refresh(job) = work {
            assert!(panel.complete_refresh(job.tag, read_tree(&job, &CancellationToken::new())));
        }
    }
}
fn paint(panel: &mut FilesPanel) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 30));
    panel.render(AREA, &mut buffer, &Theme::compiled());
    buffer
}
fn text(buffer: &Buffer, y: u16) -> String {
    (AREA.x..AREA.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}
fn click_label(panel: &mut FilesPanel, label: &str, y: u16, focus: &mut FocusState) {
    let buffer = paint(panel);
    let line = text(&buffer, y);
    let x = AREA.x + line.find(label).expect("button is rendered") as u16;
    assert!(
        panel
            .dispatch(&mouse(x, y, PointerAction::Down(MouseButton::Left)), focus)
            .unwrap()
    );
}

#[test]
fn file_tree_keys_mouse_hover_actions_and_recents_keep_absolute_coordinates_and_focus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/a file.rs");
    put(root, "z.txt");
    let mut panel = FilesPanel::new(project(root)).unwrap();
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    refresh(&mut panel);
    paint(&mut panel);
    assert!(panel.dispatch(&key(Key::Right), &mut focus).unwrap());
    let work = panel.take_work();
    assert_eq!(work.len(), 1);
    if let FilesWork::Refresh(job) = work.into_iter().next().unwrap() {
        panel.complete_refresh(job.tag, read_tree(&job, &CancellationToken::new()));
    } else {
        panic!("expansion must schedule a worker");
    }
    panel.dispatch(&key(Key::Down), &mut focus).unwrap();
    panel.dispatch(&key(Key::Enter), &mut focus).unwrap();
    assert!(
        matches!(&panel.take_work()[0], FilesWork::Open(path) if path == &root.join("src/a file.rs"))
    );
    panel.recents = vec![RecentEntry {
        path: root.join("z.txt"),
        dirty: true,
    }];
    panel.active_path = Some(root.join("z.txt"));
    let buffer = paint(&mut panel);
    assert!(text(&buffer, AREA.y + 2).starts_with("* "));
    panel
        .dispatch(
            &mouse(AREA.x, AREA.y + 2, PointerAction::Down(MouseButton::Left)),
            &mut focus,
        )
        .unwrap();
    assert!(
        matches!(&panel.take_work()[0], FilesWork::RemoveRecent(path) if path == &root.join("z.txt"))
    );
    panel
        .dispatch(
            &mouse(
                AREA.x + 2,
                AREA.y + 2,
                PointerAction::Down(MouseButton::Left),
            ),
            &mut focus,
        )
        .unwrap();
    assert!(matches!(&panel.take_work()[0], FilesWork::Open(path) if path == &root.join("z.txt")));
    click_label(&mut panel, "[change]", AREA.y, &mut focus);
    assert!(matches!(&panel.take_work()[0], FilesWork::ChangeWorkspace));
    // Recents occupy four rows here; hover controls belong to the tree only.
    let buffer = paint(&mut panel);
    let row = (AREA.y + 1..AREA.bottom())
        .find(|y| text(&buffer, *y).contains("[=] a file.rs"))
        .unwrap();
    panel
        .dispatch(&mouse(AREA.x + 4, row, PointerAction::Move), &mut focus)
        .unwrap();
    click_label(&mut panel, "[r]", row, &mut focus);
    assert!(panel.has_modal());
    panel.dispatch(&key(Key::Escape), &mut focus).unwrap();
    assert!(!panel.has_modal());
    assert_eq!(focus.focused, Some(panel.id));
}
#[test]
fn name_dialog_worker_action_failure_and_stale_completion_leave_selection_and_dirty_recents_intact()
{
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/file.rs");
    let mut panel = FilesPanel::new(project(root)).unwrap();
    let mut focus = FocusState::default();
    refresh(&mut panel);
    click_label(&mut panel, "[+f]", AREA.y, &mut focus);
    panel
        .dispatch(
            &InputEvent::Key(KeyInput {
                key: Key::Character('a'),
                modifiers: Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                repeat: false,
            }),
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(&InputEvent::Paste("name with spaces.rs".into()), &mut focus)
        .unwrap();
    panel.dispatch(&key(Key::Enter), &mut focus).unwrap();
    assert!(!panel.has_modal());
    let FilesWork::Action { tag, action } = panel.take_work().pop().unwrap() else {
        panic!("file action");
    };
    assert!(
        matches!(&action, FileAction::CreateFile { name, .. } if name == "name with spaces.rs")
    );
    assert!(
        panel
            .complete_action(crate::tag(), Ok(FileChange::Created(root.join("wrong"))))
            .unwrap()
            .is_none()
    );
    let result = perform_action(&panel.project, &action, &CancellationToken::new());
    let change = panel.complete_action(tag, result).unwrap().unwrap();
    assert_eq!(
        change,
        FileChange::Created(root.join("name with spaces.rs"))
    );
    assert_eq!(
        panel.selected.as_ref(),
        Some(&root.join("name with spaces.rs"))
    );
    for work in panel.take_work() {
        if let FilesWork::Refresh(job) = work {
            panel.complete_refresh(job.tag, read_tree(&job, &CancellationToken::new()));
        }
    }
    panel.recents.push(RecentEntry {
        path: root.join("name with spaces.rs"),
        dirty: true,
    });
    paint(&mut panel);
    panel.dispatch(&key(Key::Function(2)), &mut focus).unwrap();
    paint(&mut panel);
    panel.dispatch(&key(Key::Enter), &mut focus).unwrap();
    let FilesWork::Action { tag, .. } = panel.take_work().pop().unwrap() else {
        panic!("rename action");
    };
    let selected = panel.selected.clone();
    panel
        .complete_action(tag, Err(DomainError::Unsupported("rename denied".into())))
        .unwrap();
    assert_eq!(panel.selected, selected);
    assert!(panel.recents[0].dirty);
    assert!(panel.error.as_ref().unwrap().contains("rename denied"));
}
#[test]
fn refresh_preserves_selection_scroll_and_name_dialog_while_external_changes_are_published() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for index in 0..40 {
        put(root, &format!("file-{index:02}.rs"));
    }
    let mut panel = FilesPanel::new(project(root)).unwrap();
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    refresh(&mut panel);
    paint(&mut panel);
    panel.dispatch(&key(Key::End), &mut focus).unwrap();
    assert!(panel.scroll > 0);
    let scroll = panel.scroll;
    panel.dispatch(&key(Key::Function(2)), &mut focus).unwrap();
    put(root, "external.rs");
    refresh(&mut panel);
    assert!(panel.has_modal());
    assert_eq!(panel.selected.as_ref(), Some(&root.join("file-39.rs")));
    assert_eq!(panel.scroll, scroll);
    assert!(
        panel
            .tree
            .flattened()
            .iter()
            .any(|entry| entry.entry.name == "external.rs")
    );
    paint(&mut panel);
    panel.dispatch(&key(Key::Escape), &mut focus).unwrap();
    panel
        .dispatch(
            &mouse(
                AREA.x,
                AREA.y + 2,
                PointerAction::Scroll {
                    horizontal: 0,
                    vertical: -1,
                },
            ),
            &mut focus,
        )
        .unwrap();
    assert_eq!(panel.scroll, scroll - 1);
}
fn complete_picker(picker: &mut FolderPicker) {
    for work in picker.take_work() {
        if let PickerWork::Refresh(job) = work {
            assert!(picker.complete(job.tag, read_tree(&job, &CancellationToken::new())));
        }
    }
}
#[test]
fn picker_filters_files_focuses_nested_selection_preserves_wheel_scroll_and_restores_modal_focus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/deep/code.rs");
    for index in 0..30 {
        fs::create_dir(root.join(format!("folder{index:02}"))).unwrap();
    }
    let previous = FocusId(900);
    let mut focus = FocusState {
        focused: Some(previous),
        captured: Some(FocusId(901)),
        ..Default::default()
    };
    let mut picker =
        FolderPicker::new(project(root), "Source folder".into(), true, &mut focus).unwrap();
    assert_eq!(focus.captured, None);
    assert_eq!(focus.focused, Some(picker.id));
    complete_picker(&mut picker);
    picker.focus_path(root.join("src/deep")).unwrap();
    complete_picker(&mut picker);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 30));
    picker.render(AREA, &mut buffer, &Theme::compiled());
    assert!(picker.scroll > 0);
    let scroll = picker.scroll;
    assert!(!buffer.content.iter().any(|cell| cell.symbol() == "code.rs"));
    // Mouse wheel remains effective through paint instead of snapping to selection.
    picker
        .dispatch(
            &mouse(
                AREA.x + 10,
                AREA.y + 4,
                PointerAction::Scroll {
                    horizontal: 0,
                    vertical: -1,
                },
            ),
            &mut focus,
        )
        .unwrap();
    picker.render(AREA, &mut buffer, &Theme::compiled());
    assert_eq!(picker.scroll, scroll - 1);
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    assert!(picker.finished);
    assert_eq!(focus.focused, Some(previous));
    assert!(
        matches!(&picker.take_work()[0], PickerWork::Confirmed(path) if path == &root.join("src/deep"))
    );
    let mut picker =
        FolderPicker::new(project(root), "Exclude entry".into(), false, &mut focus).unwrap();
    complete_picker(&mut picker);
    picker.focus_path(root.join("src/deep/code.rs")).unwrap();
    complete_picker(&mut picker);
    picker.dispatch(&key(Key::Escape), &mut focus).unwrap();
    assert!(matches!(picker.take_work()[0], PickerWork::Cancelled));
    assert_eq!(focus.focused, Some(previous));
}
#[test]
fn project_panel_edits_config_via_commands_and_rescan_cancellation_waits_for_worker_acknowledgement()
 {
    let directory = tempfile::tempdir().unwrap();
    put(directory.path(), "src/code.rs");
    let mut project = project(directory.path());
    project.exclude(Path::new("src/code.rs")).unwrap();
    let mut panel = ProjectPanel::new().unwrap();
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 30));
    panel.render(AREA, &mut buffer, &Theme::compiled(), &project);
    for input in [
        Key::Character('a'),
        Key::Character('x'),
        Key::Character('r'),
        Key::End,
        Key::Delete,
    ] {
        assert!(panel.dispatch(&key(input), &project, &mut focus));
    }
    assert_eq!(
        panel.take_work(),
        [
            ProjectWork::AddSource,
            ProjectWork::AddExclusion,
            ProjectWork::Rescan,
            ProjectWork::RemoveExclusion("src/code.rs".into())
        ]
    );
    let token = CancellationToken::new();
    let tag = tag();
    let mut dialog = RescanDialog::new(tag, token.clone());
    assert!(!dialog.progress(&Progress {
        tag: crate::tag(),
        message: "old".into(),
        completed: 99,
        total: None,
        at_millis: 0
    }));
    assert!(dialog.progress(&Progress {
        tag,
        message: "src/code.rs".into(),
        completed: 2,
        total: Some(3),
        at_millis: 0
    }));
    dialog.dispatch(&key(Key::Escape));
    assert!(token.is_cancelled());
    assert!(!dialog.done);
    assert!(!dialog.finished);
    assert!(!dialog.complete(crate::tag(), Ok(())));
    assert!(dialog.complete(tag, Err(DomainError::Cancelled)));
    assert!(dialog.aborted);
    dialog.dispatch(&key(Key::Enter));
    assert!(dialog.finished);
}
#[test]
fn successful_open_folder_move_preserves_unsaved_history_and_updates_save_target_with_reservation()
{
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/file.rs");
    let project = project(root);
    let origin = DocumentOrigin::File(root.join("src/file.rs"));
    let descriptor = HostDescriptor::new(
        HostRole::File,
        "file".into(),
        "test".into(),
        "file.rs".into(),
        root.into(),
    );
    let prepared = prepare(
        NativeDocumentIo.load(&origin, tag()).unwrap(),
        DocumentId::fresh().unwrap(),
        Some("rust".into()),
    )
    .unwrap();
    let mut documents = EditorDocuments::default();
    let id = documents
        .insert_file(prepared, &descriptor, project.generation)
        .unwrap();
    let mut view = EditorView::new(
        id,
        &documents,
        QueuedEditorHost::new(descriptor),
        project.generation,
        Arc::new(ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    view.programmatic_replace(
        TextRange {
            start: ByteOffset(0),
            end: ByteOffset(4),
        },
        "unsaved",
        &mut documents,
        AREA,
    )
    .unwrap();
    let revision = documents.document(id).unwrap().revision();
    let tag = tag();
    documents
        .reserve_file_action(tag, vec![root.join("src")])
        .unwrap();
    assert!(
        view.command(EditorCommand::Save, &mut documents, AREA)
            .is_err()
    );
    let change = perform_action(
        &project,
        &FileAction::Rename {
            path: root.join("src"),
            name: "new source".into(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        documents.finish_file_action(tag, Some(&change)).unwrap(),
        [id]
    );
    view.synchronize(&documents).unwrap();
    assert_eq!(
        documents.document(id).unwrap().origin(),
        &DocumentOrigin::File(root.join("new source/file.rs"))
    );
    assert!(documents.document(id).unwrap().is_dirty());
    assert!(documents.document(id).unwrap().revision() > revision);
    view.command(EditorCommand::Undo, &mut documents, AREA)
        .unwrap();
    assert_eq!(&*documents.document(id).unwrap().snapshot().text(), "disk");
    view.command(EditorCommand::Redo, &mut documents, AREA)
        .unwrap();
    assert_eq!(
        &*documents.document(id).unwrap().snapshot().text(),
        "unsaved"
    );
    view.take_work();
    view.command(EditorCommand::Save, &mut documents, AREA)
        .unwrap();
    let job = view
        .take_work()
        .into_iter()
        .find_map(|work| {
            if let EditorWork::SaveFile(job) = work {
                Some(job)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        documents
            .reserve_file_action(crate::tag(), vec![root.join("new source")])
            .is_err()
    );
    let DocumentOrigin::File(path) = &job.snapshot.origin else {
        panic!("file origin");
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
    assert_eq!(fs::read_to_string(path).unwrap(), "unsaved");
    assert!(!root.join("src").exists());
}
#[test]
fn panels_and_modals_clip_empty_and_tiny_viewports_without_painting_outside_bounds() {
    let directory = tempfile::tempdir().unwrap();
    put(directory.path(), "a file");
    let project = project(directory.path());
    let mut files = FilesPanel::new(project.clone()).unwrap();
    refresh(&mut files);
    let mut panel = ProjectPanel::new().unwrap();
    let mut focus = FocusState::default();
    let mut picker = FolderPicker::new(project.clone(), "Source".into(), true, &mut focus).unwrap();
    complete_picker(&mut picker);
    let mut dialog = RescanDialog::new(tag(), CancellationToken::new());
    for width in 0..5 {
        for height in 0..6 {
            let area = Rect::new(3, 2, width, height);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 12));
            files.render(area, &mut buffer, &Theme::compiled());
            panel.render(area, &mut buffer, &Theme::compiled(), &project);
            picker.render(area, &mut buffer, &Theme::compiled());
            dialog.render(area, &mut buffer, &Theme::compiled());
            for y in 0..12 {
                for x in 0..12 {
                    if !area.contains((x, y).into()) {
                        assert_eq!(buffer[(x, y)].symbol(), " ");
                    }
                }
            }
        }
    }
}
