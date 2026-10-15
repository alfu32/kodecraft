use kode_core::{DomainError, WorkspaceGeneration};
use kode_git::{model::*, read::ReadOutput};
use kode_tui::{panels::git::read::GitReadPanel, theme::Theme};
use ratatui::{buffer::Buffer, layout::Rect};
use std::path::PathBuf;
fn empty<T>() -> Listing<T> {
    Listing {
        entries: vec![],
        truncated: false,
    }
}
fn snapshot(names: &[&str]) -> RepositorySnapshot {
    RepositorySnapshot {
        location: RepositoryLocation {
            git_dir: "fixture/.git".into(),
            work_tree: Some("fixture".into()),
            bare: false,
        },
        head: Some(Head::Unborn {
            reference: "refs/heads/main".into(),
            name: "main".into(),
        }),
        status: Listing {
            entries: names
                .iter()
                .map(|name| StatusEntry {
                    path: PathBuf::from(name),
                    original_path: None,
                    layer: StatusLayer::Worktree,
                    kind: ChangeKind::Modified,
                    code: "M".into(),
                })
                .collect(),
            truncated: false,
        },
        branches: empty(),
        tags: empty(),
        commits: empty(),
        errors: vec![],
    }
}
fn line(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}
#[test]
fn original_status_failure_does_not_crash_panel_startup_or_render_and_never_does_io() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("never-created");
    let mut panel = GitReadPanel::new(root.clone(), WorkspaceGeneration(1)).unwrap();
    let (tag, job) = panel.begin_refresh().unwrap();
    assert_eq!(job.root, root);
    assert!(panel.complete_refresh(
        tag,
        Err(DomainError::Git {
            operation: "status".into(),
            message: "large object exceeds size limit".into()
        })
    ));
    for area in [
        Rect::new(0, 0, 80, 12),
        Rect::new(7, 4, 20, 6),
        Rect::new(0, 0, 1, 1),
        Rect::new(0, 0, 0, 0),
    ] {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
        panel.render_status(area, &mut buffer, &Theme::compiled());
        panel.render_history(Rect::new(0, 20, 80, 10), &mut buffer, &Theme::compiled());
        if area.width == 80 {
            assert!(
                line(&buffer, 1).contains("Git status failed: large object exceeds size limit")
            );
        }
    }
    assert!(!root.exists());
    assert!(panel.snapshot.is_none());
    assert!(panel.error.is_some());
}
#[test]
fn refresh_keeps_previous_rows_on_error_preserves_selection_and_rejects_old_workspace_results() {
    let mut panel = GitReadPanel::new("fixture".into(), WorkspaceGeneration(1)).unwrap();
    let (tag, _) = panel.begin_refresh().unwrap();
    assert!(panel.begin_refresh().is_none());
    panel.complete_refresh(
        tag,
        Ok(ReadOutput::Snapshot(Box::new(snapshot(&["a", "b"])))),
    );
    panel.selected_status = 1;
    let (tag, _) = panel.begin_refresh().unwrap();
    panel.complete_refresh(
        tag,
        Ok(ReadOutput::Snapshot(Box::new(snapshot(&["0", "a", "b"])))),
    );
    assert_eq!(panel.selected_status, 2);
    let (tag, _) = panel.begin_refresh().unwrap();
    panel.complete_refresh(
        tag,
        Err(DomainError::Git {
            operation: "status".into(),
            message: "fixture failure".into(),
        }),
    );
    assert_eq!(panel.snapshot.as_ref().unwrap().status.entries.len(), 3);
    assert_eq!(panel.selected_status, 2);
    let (old, _) = panel.begin_refresh().unwrap();
    panel.set_root("next".into(), WorkspaceGeneration(2));
    let (current, _) = panel.begin_refresh().unwrap();
    assert!(!panel.complete_refresh(old, Ok(ReadOutput::Snapshot(Box::new(snapshot(&["old"]))))));
    assert_eq!(panel.busy, Some(current));
    let mut partial = snapshot(&["current"]);
    partial.errors.push("Git commit listing unavailable".into());
    panel.complete_refresh(current, Ok(ReadOutput::Snapshot(Box::new(partial))));
    assert_eq!(
        panel.snapshot.as_ref().unwrap().status.entries[0].path,
        PathBuf::from("current")
    );
    assert!(panel.error.as_ref().unwrap().contains("commit listing"));
    let (cancelled, _) = panel.begin_refresh().unwrap();
    panel.cancel_refresh();
    assert!(!panel.complete_refresh(
        cancelled,
        Ok(ReadOutput::Snapshot(Box::new(snapshot(&["late"]))))
    ));
}
