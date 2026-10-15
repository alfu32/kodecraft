use kode_core::{DomainError, JobTag, RequestId, WorkspaceGeneration};
use kode_platform::jobs::{CancellationToken, MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_workspace::{
    exclusions::FolderStatus,
    project::Project,
    tree::*,
    watch::{RefreshTimer, TreeWatcher},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
fn project(root: &Path) -> Project {
    Project::new(root, WorkspaceGeneration(1)).unwrap()
}
fn put(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "text").unwrap();
    path
}
fn refresh(tree: &mut FileTree, project: &Project) {
    let job = tree.begin_refresh(project).unwrap();
    let result = read_tree(&job, &CancellationToken::new());
    assert!(tree.publish(job.tag, result));
}

#[test]
fn original_metadata_fixture_browser_policy_sorting_counts_sizes_and_folder_status() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "Visible.kt");
    put(root, ".kode.json");
    put(root, ".kode/hidden.txt");
    put(root, ".kode-rs.json.bak");
    put(root, ".kode-rs/recovery.txt");
    put(root, ".gitignore");
    put(root, ".git/config");
    put(root, ".hidden.txt");
    put(root, "build/output.kt");
    put(root, "ignored/data");
    put(root, "src/nested/code.rs");
    fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    let project = project(root);
    let mut tree = FileTree::new(&project);
    refresh(&mut tree, &project);
    let entries = tree.flattened();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.entry.name.as_str())
            .collect::<Vec<_>>(),
        [
            ".git",
            "build",
            "ignored",
            "src",
            ".gitignore",
            ".hidden.txt",
            "Visible.kt"
        ]
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.entry.name == "src")
            .unwrap()
            .entry
            .child_count,
        Some(1)
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.entry.name == "src")
            .unwrap()
            .entry
            .status,
        Some(FolderStatus::InSource)
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.entry.name == "ignored")
            .unwrap()
            .entry
            .status,
        Some(FolderStatus::GitIgnored)
    );
    assert_eq!(human_size(1024), "1.0 KiB");
    assert_eq!(human_size(10), "10 B");
    assert_eq!(human_size(1_048_576), "1.0 MiB");
    assert!(!entries.iter().any(|entry| entry.entry.name == "nested"));
}
#[test]
fn lazy_expansion_refresh_preserves_state_rejects_old_jobs_and_prunes_replaced_subtrees() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let file = put(root, "src/a/one file.rs");
    let project = project(root);
    let mut tree = FileTree::new(&project);
    refresh(&mut tree, &project);
    tree.open_path(&file).unwrap();
    refresh(&mut tree, &project);
    assert_eq!(tree.flattened().len(), 3);
    assert_eq!(tree.flattened()[2].depth, 2);
    let old = tree.begin_refresh(&project).unwrap();
    put(root, "src/a/two.rs");
    let current = tree.begin_refresh(&project).unwrap();
    assert!(!tree.publish(old.tag, read_tree(&old, &CancellationToken::new())));
    assert!(tree.publish(current.tag, read_tree(&current, &CancellationToken::new())));
    assert!(tree.is_open(&root.join("src/a")));
    assert_eq!(tree.flattened().len(), 4);
    assert!(tree.toggle(&root.join("src")));
    assert_eq!(tree.flattened().len(), 1);
    assert!(tree.toggle(&root.join("src")));
    assert_eq!(tree.flattened().len(), 4);
    fs::remove_dir_all(root.join("src/a")).unwrap();
    refresh(&mut tree, &project);
    assert_eq!(tree.flattened().len(), 1);
    assert!(!tree.is_open(&root.join("src/a")));
    let old = tree.begin_refresh(&project).unwrap();
    assert!(tree.publish(old.tag, Err(DomainError::Cancelled)));
    assert_eq!(tree.flattened().len(), 1);
    assert!(!tree.errors.is_empty());
    assert!(
        tree.open_path(&root.with_extension("sibling").join("x"))
            .is_err()
    );
}
#[test]
fn actions_preserve_spaces_refuse_overwrite_escape_metadata_and_failures_are_visible() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let project = project(root);
    let token = CancellationToken::new();
    let path = root.join("my file.rs");
    assert_eq!(
        perform_action(
            &project,
            &FileAction::CreateFile {
                parent: root.into(),
                name: "my file.rs".into()
            },
            &token
        )
        .unwrap(),
        FileChange::Created(path.clone())
    );
    fs::write(&path, "original").unwrap();
    put(root, "existing.rs");
    assert!(
        perform_action(
            &project,
            &FileAction::Rename {
                path: path.clone(),
                name: "existing.rs".into()
            },
            &token
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(
        fs::read_to_string(root.join("existing.rs")).unwrap(),
        "text"
    );
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a/b",
        "a\\b",
        ".kode.json",
        ".kode-rs.json.bak",
    ] {
        assert!(
            perform_action(
                &project,
                &FileAction::Rename {
                    path: path.clone(),
                    name: name.into()
                },
                &token
            )
            .is_err(),
            "{name}"
        );
    }
    let to = root.join("renamed file.rs");
    assert_eq!(
        perform_action(
            &project,
            &FileAction::Rename {
                path: path.clone(),
                name: "renamed file.rs".into()
            },
            &token
        )
        .unwrap(),
        FileChange::Renamed {
            from: path,
            to: to.clone()
        }
    );
    assert_eq!(fs::read_to_string(&to).unwrap(), "original");
    assert!(perform_action(&project, &FileAction::Delete { path: root.into() }, &token).is_err());
    assert!(
        perform_action(
            &project,
            &FileAction::Delete {
                path: root.join("missing")
            },
            &token
        )
        .is_err()
    );
    perform_action(&project, &FileAction::Delete { path: to.clone() }, &token).unwrap();
    assert!(!to.exists());
    token.cancel();
    assert!(matches!(
        perform_action(
            &project,
            &FileAction::Delete {
                path: root.join("existing.rs")
            },
            &token
        ),
        Err(DomainError::Cancelled)
    ));
}
#[cfg(unix)]
#[test]
fn directory_links_never_expand_and_deleting_links_does_not_touch_external_targets() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/one");
    let external = put(outside.path(), "external");
    symlink(root.join("src"), root.join("loop")).unwrap();
    symlink(outside.path(), root.join("outside")).unwrap();
    let project = project(root);
    let mut tree = FileTree::new(&project);
    refresh(&mut tree, &project);
    assert!(!tree.toggle(&root.join("loop")));
    tree.open_path(&root.join("outside/external")).unwrap();
    refresh(&mut tree, &project);
    assert!(!tree.errors.is_empty());
    assert!(
        !tree
            .flattened()
            .iter()
            .any(|entry| entry.entry.name == "external")
    );
    assert!(
        perform_action(
            &project,
            &FileAction::CreateFile {
                parent: root.join("outside"),
                name: "new".into()
            },
            &CancellationToken::new()
        )
        .is_err()
    );
    perform_action(
        &project,
        &FileAction::Delete {
            path: root.join("outside"),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(external.exists());
    assert!(!root.join("outside").exists());
}
#[test]
fn coalesced_native_refresh_and_timer_fallback_do_not_lose_busy_invalidations() {
    let mut timer = RefreshTimer::default();
    timer.changed(100);
    timer.changed(200);
    assert!(!timer.due(249, false));
    assert!(timer.due(250, false));
    timer.changed(300);
    assert!(!timer.due(6000, true));
    assert!(timer.due(6001, false));
    assert!(!timer.due(11000, false));
    assert!(timer.due(11001, false));
    let directory = tempfile::tempdir().unwrap();
    let watcher = TreeWatcher::new(directory.path());
    assert!(watcher.take_error().is_none());
    put(directory.path(), "external refresh.txt");
    for _ in 0..100 {
        if watcher.take_dirty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("native watcher did not report the external mutation");
}
#[tokio::test]
async fn actual_worker_reads_tree_and_delivers_critical_file_operation() {
    let directory = tempfile::tempdir().unwrap();
    let project = project(directory.path());
    let mut tree = FileTree::new(&project);
    let job = tree.begin_refresh(&project).unwrap();
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_tree(&mut scheduler, OwnerId(7), job).unwrap();
    let completion = scheduler.next().await.unwrap();
    assert!(tree.publish(completion.tag, completion.result));
    assert!(tree.flattened().is_empty());
    assert!(scheduler.shutdown().await.is_empty());
    let tag = JobTag {
        workspace: project.generation,
        request: RequestId::fresh().unwrap(),
        document: None,
    };
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_action(
        &mut scheduler,
        OwnerId(8),
        tag,
        project.clone(),
        FileAction::CreateDirectory {
            parent: project.root.clone(),
            name: "new folder".into(),
        },
    )
    .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.tag, tag);
    assert_eq!(
        result.result.unwrap(),
        FileChange::Created(project.root.join("new folder"))
    );
    assert!(scheduler.shutdown().await.is_empty());
}
