use kode_core::{ByteOffset, TextRange, WorkspaceGeneration};
use kode_git::{model::*, read::ReadOutput, write::WriteReceipt};
use kode_platform::{clipboard::ClipboardService, jobs::Clock};
use kode_tui::{
    editor::host::{ClipboardResult, EditorDocuments},
    focus::FocusState,
    input::terminal::*,
    panels::git::{GitCommand, GitPanel, GitWork, ReadKind},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{path::PathBuf, sync::Arc};
struct ClockZero;
impl Clock for ClockZero {
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
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Default::default(),
    })
}
fn snapshot() -> RepositorySnapshot {
    let hash = "a".repeat(40);
    RepositorySnapshot {
        location: RepositoryLocation {
            git_dir: "fixture/.git".into(),
            work_tree: Some("fixture".into()),
            bare: false,
        },
        head: Some(Head::Branch {
            reference: "refs/heads/main".into(),
            name: "main".into(),
            commit: hash.clone(),
        }),
        status: Listing {
            entries: vec![StatusEntry {
                path: "file.txt".into(),
                original_path: None,
                layer: StatusLayer::Worktree,
                kind: ChangeKind::Modified,
                code: "M".into(),
            }],
            truncated: false,
        },
        branches: Listing {
            entries: vec![
                Branch {
                    name: "feature/topic".into(),
                    reference: "refs/heads/feature/topic".into(),
                    commit: Some("b".repeat(40)),
                    current: false,
                },
                Branch {
                    name: "main".into(),
                    reference: "refs/heads/main".into(),
                    commit: Some(hash.clone()),
                    current: true,
                },
            ],
            truncated: false,
        },
        tags: Listing {
            entries: vec![Tag {
                name: "release/v1.0".into(),
                object: "c".repeat(40),
                commit: Some(hash.clone()),
                annotated: true,
            }],
            truncated: false,
        },
        commits: Listing {
            entries: vec![Commit {
                hash,
                author: "Fixture".into(),
                timestamp: 0,
                offset_minutes: 0,
                message: "first message\n\nbody".into(),
                parents: vec![],
                tag: Some("v1.0".into()),
            }],
            truncated: false,
        },
        errors: vec![],
    }
}
fn ready() -> (GitPanel, EditorDocuments, FocusState) {
    let mut documents = EditorDocuments::default();
    let mut panel = GitPanel::new(
        "fixture".into(),
        WorkspaceGeneration(1),
        &mut documents,
        Arc::new(ClipboardService::internal()),
        Arc::new(ClockZero),
    )
    .unwrap();
    let discover = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Read {
                tag,
                kind: ReadKind::Discover,
                ..
            } => Some(tag),
            _ => None,
        })
        .unwrap();
    panel.complete_read(
        discover,
        ReadKind::Discover,
        Ok(ReadOutput::Location(Some(snapshot().location))),
    );
    refresh(&mut panel, snapshot());
    let focus = FocusState {
        focused: Some(panel.id()),
        ..Default::default()
    };
    (panel, documents, focus)
}
fn refresh(panel: &mut GitPanel, snapshot: RepositorySnapshot) {
    let tag = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Read {
                tag,
                kind: ReadKind::Snapshot,
                ..
            } => Some(tag),
            _ => None,
        })
        .unwrap();
    panel.complete_read(
        tag,
        ReadKind::Snapshot,
        Ok(ReadOutput::Snapshot(Box::new(snapshot))),
    );
}
fn paint(panel: &mut GitPanel, documents: &EditorDocuments, focus: &FocusState) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
    panel
        .render(
            Rect::new(10, 3, 40, 32),
            &mut buffer,
            &Theme::compiled(),
            documents,
            focus,
        )
        .unwrap();
    panel.render_modal(buffer.area, &mut buffer, &Theme::compiled());
    buffer
}
fn text(panel: &GitPanel, documents: &EditorDocuments) -> String {
    documents
        .document(panel.commit_editor.as_ref().unwrap().document_id())
        .unwrap()
        .snapshot()
        .text()
        .to_string()
}
fn replace(panel: &mut GitPanel, documents: &mut EditorDocuments, value: &str) {
    let view = panel.commit_editor.as_mut().unwrap();
    let length = documents
        .document(view.document_id())
        .unwrap()
        .snapshot()
        .len_bytes();
    view.programmatic_replace(
        TextRange::new(ByteOffset(0), ByteOffset(length)).unwrap(),
        value,
        documents,
        panel.geometry.editor,
    )
    .unwrap();
}
#[test]
fn status_acknowledgement_does_not_toggle_optimistically_or_clear_commit_text_and_commit_mouse_up_runs_once()
 {
    use crossterm::event::MouseButton::Left;
    let (mut panel, mut documents, mut focus) = ready();
    paint(&mut panel, &documents, &focus);
    replace(&mut panel, &mut documents, "draft");
    let row = panel.geometry.status.y + 1;
    panel
        .dispatch(
            &pointer(panel.geometry.status.x, row, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(panel.geometry.status.x, row, PointerAction::Up(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    let writes = panel
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            GitWork::Write { tag, command, .. } => Some((tag, command)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 1);
    assert!(matches!(&writes[0].1,GitCommand::Stage(paths) if paths==&[PathBuf::from("file.txt")]));
    assert!(!panel.read.snapshot.as_ref().unwrap().status.entries[0].staged());
    assert_eq!(text(&panel, &documents), "draft");
    panel
        .complete_write(
            writes[0].0,
            Ok(WriteReceipt {
                message: "Staged fixture".into(),
                commit: None,
                worktree_changed: false,
                warnings: vec![],
            }),
            &mut documents,
        )
        .unwrap();
    let mut staged = snapshot();
    staged.status.entries[0].layer = StatusLayer::Index;
    refresh(&mut panel, staged);
    assert_eq!(text(&panel, &documents), "draft");
    paint(&mut panel, &documents, &focus);
    let button = panel.geometry.commit;
    panel
        .dispatch(
            &pointer(button.x, button.y, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(panel.write_busy.is_none());
    panel
        .dispatch(
            &pointer(button.x, button.y, PointerAction::Up(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(button.x, button.y, PointerAction::Up(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    let writes = panel
        .take_work()
        .into_iter()
        .filter_map(|work| match work {
            GitWork::Write { tag, command, .. } => Some((tag, command)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(writes.len(), 1);
    assert!(
        matches!(&writes[0].1,GitCommand::Commit(snapshot) if snapshot.text().as_ref()=="draft")
    );
    replace(&mut panel, &mut documents, "draft typed while pending");
    panel
        .complete_write(
            writes[0].0,
            Ok(WriteReceipt {
                message: "Committed fixture".into(),
                commit: Some("a".repeat(40)),
                worktree_changed: false,
                warnings: vec![],
            }),
            &mut documents,
        )
        .unwrap();
    assert_eq!(text(&panel, &documents), "draft typed while pending");
    assert!(panel.message.contains("Current message kept"));
}
#[test]
fn branch_and_tag_menus_keep_full_names_explicit_move_and_field_paste_context() {
    let (mut panel, mut documents, mut focus) = ready();
    paint(&mut panel, &documents, &focus);
    panel
        .dispatch(&key(Key::Character('b'), false), &mut documents, &mut focus)
        .unwrap();
    paint(&mut panel, &documents, &focus);
    panel
        .dispatch(&key(Key::Up, false), &mut documents, &mut focus)
        .unwrap();
    panel
        .dispatch(&key(Key::Enter, false), &mut documents, &mut focus)
        .unwrap();
    let (tag, command) = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Write { tag, command, .. } => Some((tag, command)),
            _ => None,
        })
        .unwrap();
    assert!(matches!(command,GitCommand::Checkout(name) if name=="feature/topic"));
    assert!(!panel.has_modal());
    panel
        .complete_write(
            tag,
            Ok(WriteReceipt {
                message: "Checked out fixture".into(),
                commit: None,
                worktree_changed: false,
                warnings: vec![],
            }),
            &mut documents,
        )
        .unwrap();
    refresh(&mut panel, snapshot());
    panel
        .dispatch(&key(Key::Character('T'), false), &mut documents, &mut focus)
        .unwrap();
    paint(&mut panel, &documents, &focus);
    panel
        .dispatch(&key(Key::Character('v'), true), &mut documents, &mut focus)
        .unwrap();
    let paste = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Clipboard {
                tag, read: true, ..
            } => Some(tag),
            _ => None,
        })
        .unwrap();
    panel
        .dispatch(&key(Key::Character('a'), true), &mut documents, &mut focus)
        .unwrap();
    panel
        .dispatch(
            &InputEvent::Paste("release/v2.0".into()),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel.complete_clipboard(
        paste,
        Ok(ClipboardResult::Read("late paste".into())),
        &focus,
    );
    panel
        .dispatch(&key(Key::Enter, false), &mut documents, &mut focus)
        .unwrap();
    let command = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Write { command, .. } => Some(command),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(command,GitCommand::Tag {name,revision,move_existing:true} if name=="release/v2.0"&&revision=="a".repeat(40))
    );
}
#[test]
fn shared_editor_keeps_tab_and_clipboard_behavior_and_capture_prevents_accidental_commit_on_selection_release()
 {
    use crossterm::event::MouseButton::Left;
    let (mut panel, mut documents, mut focus) = ready();
    paint(&mut panel, &documents, &focus);
    panel
        .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
        .unwrap();
    let editor = panel.commit_editor.as_ref().unwrap().input.id;
    assert_eq!(focus.focused, Some(editor));
    panel
        .dispatch(
            &InputEvent::Paste("first\nsecond".into()),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    let before = text(&panel, &documents);
    panel
        .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
        .unwrap();
    assert_eq!(focus.focused, Some(editor));
    assert!(text(&panel, &documents).len() > before.len());
    paint(&mut panel, &documents, &focus);
    let geometry = panel
        .commit_editor
        .as_ref()
        .unwrap()
        .geometry(&documents, panel.geometry.editor)
        .unwrap();
    panel
        .dispatch(
            &pointer(geometry.text.x, geometry.text.y, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(
                panel.geometry.commit.x,
                panel.geometry.commit.y,
                PointerAction::Drag(Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(
                panel.geometry.commit.x,
                panel.geometry.commit.y,
                PointerAction::Up(Left),
            ),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(panel.write_busy.is_none());
    assert!(
        panel
            .take_work()
            .iter()
            .all(|work| !matches!(work, GitWork::Write { .. }))
    );
    assert!(focus.captured.is_none());
}
#[test]
fn splitters_offset_tiny_rectangles_and_missing_repo_init_button_have_safe_owned_hitboxes() {
    use crossterm::event::MouseButton::Left;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("never-created");
    let mut documents = EditorDocuments::default();
    let mut panel = GitPanel::new(
        root.clone(),
        WorkspaceGeneration(1),
        &mut documents,
        Arc::new(ClipboardService::internal()),
        Arc::new(ClockZero),
    )
    .unwrap();
    let tag = panel
        .take_work()
        .into_iter()
        .find_map(|work| match work {
            GitWork::Read {
                tag,
                kind: ReadKind::Discover,
                ..
            } => Some(tag),
            _ => None,
        })
        .unwrap();
    panel.complete_read(tag, ReadKind::Discover, Ok(ReadOutput::Location(None)));
    let mut focus = FocusState {
        focused: Some(panel.id()),
        ..Default::default()
    };
    paint(&mut panel, &documents, &focus);
    let status = panel.geometry.status;
    panel
        .dispatch(
            &pointer(status.x, status.y, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(
        panel
            .take_work()
            .iter()
            .all(|work| !matches!(work, GitWork::Write { .. }))
    );
    panel
        .dispatch(
            &pointer(status.x, status.y + 2, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(
        panel
            .take_work()
            .iter()
            .filter(|work| matches!(
                work,
                GitWork::Write {
                    command: GitCommand::Init,
                    ..
                }
            ))
            .count(),
        1
    );
    let divider = panel.geometry.first_divider;
    panel
        .dispatch(
            &pointer(divider.x, divider.y, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert!(panel.has_capture(&focus));
    panel
        .dispatch(
            &pointer(divider.x, divider.y + 3, PointerAction::Drag(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(divider.x, divider.y + 3, PointerAction::Up(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel
        .dispatch(
            &pointer(divider.x, divider.y, PointerAction::Down(Left)),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    panel.deactivate(&mut documents, &mut focus).unwrap();
    assert!(!panel.has_capture(&focus));
    assert!(focus.captured.is_none());
    panel
        .dispatch(&key(Key::Tab, false), &mut documents, &mut focus)
        .unwrap();
    panel
        .dispatch(
            &InputEvent::Paste("after tab switch".into()),
            &mut documents,
            &mut focus,
        )
        .unwrap();
    assert_eq!(text(&panel, &documents), "after tab switch");
    for area in [
        Rect::new(7, 5, 80, 24),
        Rect::new(2, 2, 20, 6),
        Rect::new(0, 0, 1, 1),
        Rect::new(0, 0, 0, 0),
    ] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
        panel
            .render(area, &mut buffer, &Theme::compiled(), &documents, &focus)
            .unwrap();
    }
    assert!(!root.exists());
}
