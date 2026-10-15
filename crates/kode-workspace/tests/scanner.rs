use kode_core::{DomainError, JobTag, RequestId, WorkspaceGeneration};
use kode_platform::jobs::{CancellationToken, Clock, Delivery, Notifications, OwnerId, Scheduler};
use kode_workspace::{
    exclusions::{FolderStatus, IgnoreRules, folder_status},
    project::{Project, ProjectConfig},
    scanner::{BATCH_SIZE, ScanBatch, ScanCollector, ScanSummary, ScannedFile, scan},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

fn put(root: &Path, path: &str) -> PathBuf {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "synthetic").unwrap();
    path
}
fn tag() -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn configured(root: &Path, roots: &[&str]) -> Project {
    Project::configured(
        root,
        WorkspaceGeneration(1),
        ProjectConfig {
            source_roots: roots.iter().map(PathBuf::from).collect(),
            ..Default::default()
        },
    )
    .unwrap()
}
fn collect(project: &Project) -> (Vec<ScannedFile>, ScanSummary) {
    let mut files = vec![];
    let summary = scan(project, tag(), CancellationToken::new(), |batch| {
        assert!(batch.files.len() <= BATCH_SIZE);
        files.extend(batch.files);
        Ok(())
    })
    .unwrap();
    (files, summary)
}
fn relative(files: &[ScannedFile]) -> Vec<String> {
    files
        .iter()
        .map(|file| file.relative.to_str().unwrap().to_string())
        .collect()
}

#[test]
fn original_scanner_fixtures_skip_git_build_globs_and_forced_paths() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, ".git/metadata");
    put(root, "build/output.kt");
    put(root, "src/Main.kt");
    put(root, "tmp/generated/File.kt");
    let mut project = configured(root, &["."]);
    project.config.ignore_patterns = vec!["tmp/**".into()];
    assert_eq!(relative(&collect(&project).0), ["src/Main.kt"]);
    fs::write(root.join(".gitignore"), "ignored-dir/\n*.generated\n").unwrap();
    put(root, "ignored-dir/File.kt");
    put(root, "src/Build.generated");
    put(root, "src/Exclude.kt");
    put(root, "vendor/ThirdParty.kt");
    fs::write(root.join("vendor/.gitignore"), "!ThirdParty.kt\n").unwrap();
    project.config.exclusions = vec!["vendor".into(), "src/Exclude.kt".into()];
    assert_eq!(relative(&collect(&project).0), ["src/Main.kt"]);
}

#[test]
fn defaults_existing_folders_saved_empty_roots_overlap_absolute_paths_and_edits_are_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "README.md");
    assert!(
        Project::new(root, WorkspaceGeneration(1))
            .unwrap()
            .config
            .source_roots
            .is_empty()
    );
    put(root, "src/main/one.kt");
    put(root, "lib/two.c");
    put(root, "include/three.h");
    put(root, "docs/unindexed.kt");
    let mut project = Project::new(root, WorkspaceGeneration(1)).unwrap();
    assert_eq!(
        project.config.source_roots,
        [
            PathBuf::from("src"),
            PathBuf::from("lib"),
            PathBuf::from("include")
        ]
    );
    assert_eq!(collect(&project).0.len(), 3);
    project.config.source_roots.push(root.join("src/main"));
    project.config.source_roots.push("src".into());
    assert_eq!(project.resolved_roots().0.len(), 3);
    assert_eq!(collect(&project).0.len(), 3);
    assert!(collect(&configured(root, &[])).0.is_empty());
    assert!(project.add_source_root(&root.join("docs")).unwrap());
    assert!(!project.add_source_root(&root.join("docs")).unwrap());
    assert!(project.exclude(&root.join("docs/unindexed.kt")).unwrap());
    assert_eq!(collect(&project).0.len(), 3);
    assert!(project.include(Path::new("docs/unindexed.kt")).unwrap());
    assert!(project.remove_source_root(Path::new("docs")).unwrap());
    assert!(project.exclude(root).is_err());
    assert!(project.add_source_root(&root.join("README.md")).is_err());
}

#[test]
fn nested_ignore_negation_anchoring_hidden_whitelists_and_metadata_have_explicit_policy() {
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join(".gitignore"), "*.kt\n").unwrap();
    let root = outer.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join(".gitignore"),
        "*.generated\n!.github/\n!.github/**\n!.kode/\n!.kode/**\n",
    )
    .unwrap();
    put(&root, "src/normal.kt");
    put(&root, "src/nested/keep.generated");
    put(&root, "src/nested/drop.generated");
    put(&root, "src/nested/local/drop.kt");
    put(&root, "src/local/kept.kt");
    put(&root, "src/.hidden/drop.kt");
    put(&root, ".github/workflows/build.yaml");
    put(&root, ".kode/private.kt");
    put(&root, ".kode-rs/index.sqlite");
    put(&root, "target/output.rs");
    put(&root, "buildSrc/real.kt");
    fs::write(
        root.join("src/nested/.gitignore"),
        "!keep.generated\n/local/\n",
    )
    .unwrap();
    let project = configured(&root, &["."]);
    let (files, summary) = collect(&project);
    assert!(!summary.incomplete);
    assert_eq!(
        relative(&files),
        [
            ".github/workflows/build.yaml",
            "buildSrc/real.kt",
            "src/local/kept.kt",
            "src/nested/keep.generated",
            "src/normal.kt"
        ]
    );
    let mut project = configured(&root, &["src"]);
    project.config.ignore_patterns = vec!["*.kt".into()];
    fs::write(
        root.join("src/nested/.gitignore"),
        "!keep.generated\n!override.kt\n/local/\n",
    )
    .unwrap();
    put(&root, "src/nested/override.kt");
    assert_eq!(
        relative(&collect(&project).0),
        ["src/nested/keep.generated", "src/nested/override.kt"]
    );
}

#[test]
fn original_folder_status_fixtures_and_outside_file_bounds_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for path in [
        "src/main/file.kt",
        "src/main/generated/file.kt",
        "docs/file.md",
        "build/cache/file",
    ] {
        put(root, path);
    }
    fs::write(root.join(".gitignore"), "build/\n").unwrap();
    let mut project = configured(root, &["src"]);
    project.config.exclusions = vec!["src/main/generated".into()];
    let rules = IgnoreRules::new(&project);
    for (path, status) in [
        ("src/main", FolderStatus::InSource),
        ("src/main/generated", FolderStatus::ExcludedSource),
        ("docs", FolderStatus::OutsideSource),
        ("build/cache", FolderStatus::GitIgnored),
    ] {
        assert_eq!(
            folder_status(&project, &root.join(path), &rules).unwrap(),
            Some(status)
        );
    }
    assert_eq!(
        folder_status(&project, &root.join("src/main/file.kt"), &rules).unwrap(),
        None
    );
    assert_eq!(
        folder_status(&project, root.parent().unwrap(), &rules).unwrap(),
        None
    );
}

#[test]
fn cancellation_staging_and_workspace_switch_never_publish_partial_or_old_generation_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for index in 0..400 {
        put(root, &format!("src/f{index:03}.kt"));
    }
    let project = configured(root, &["src"]);
    let token = CancellationToken::new();
    let cancel = token.clone();
    let mut batches = 0;
    assert!(matches!(
        scan(&project, tag(), token, |batch| {
            batches += 1;
            assert_eq!(batch.files.len(), BATCH_SIZE);
            cancel.cancel();
            Ok(())
        }),
        Err(DomainError::Cancelled)
    ));
    assert_eq!(batches, 1);
    let mut collector = ScanCollector::new(WorkspaceGeneration(1));
    let first = collector.begin().unwrap();
    let summary = scan(&project, first, CancellationToken::new(), |batch| {
        assert!(collector.batch(batch));
        Ok(())
    })
    .unwrap();
    assert!(collector.complete(first, Ok(summary)));
    assert_eq!(collector.files.len(), 400);
    let old = collector.begin().unwrap();
    collector.cancel();
    assert!(!collector.complete(old, Err(DomainError::Cancelled)));
    assert_eq!(collector.files.len(), 400);
    let old = collector.begin().unwrap();
    collector.switch_workspace(WorkspaceGeneration(2));
    assert!(!collector.batch(ScanBatch {
        tag: old,
        files: vec![],
        progress: Default::default()
    }));
    assert!(collector.files.is_empty());
    assert!(matches!(
        scan(
            &project,
            JobTag {
                workspace: WorkspaceGeneration(2),
                ..tag()
            },
            CancellationToken::new(),
            |_| Ok(())
        ),
        Err(DomainError::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn unreadable_folders_and_canonical_symlinks_loops_and_outside_targets_are_handled() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "src/main/keep.kt");
    put(root, "blocked/private.kt");
    let external = tempfile::tempdir().unwrap();
    let outside = put(external.path(), "outside.kt");
    symlink(root.join("src/main/keep.kt"), root.join("src/alias.kt")).unwrap();
    symlink(root.join("src"), root.join("src/loop")).unwrap();
    symlink(outside, root.join("src/outside.kt")).unwrap();
    symlink(external.path(), root.join("escape")).unwrap();
    let mut project = configured(root, &["src", "src/main"]);
    assert_eq!(relative(&collect(&project).0), ["src/main/keep.kt"]);
    assert!(project.add_source_root(Path::new("escape")).is_err());
    project.config.source_roots = vec![".".into()];
    let mut collector = ScanCollector::new(project.generation);
    let complete = collector.begin().unwrap();
    let summary = scan(&project, complete, CancellationToken::new(), |batch| {
        collector.batch(batch);
        Ok(())
    })
    .unwrap();
    collector.complete(complete, Ok(summary));
    assert_eq!(collector.files.len(), 2);
    let blocked = root.join("blocked");
    let permissions = fs::metadata(&blocked).unwrap().permissions();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o200)).unwrap();
    let (files, summary) = collect(&project);
    fs::set_permissions(&blocked, permissions).unwrap();
    assert_eq!(relative(&files), ["src/main/keep.kt"]);
    assert!(summary.incomplete);
    assert!(!summary.warnings.is_empty());
    let partial = collector.begin().unwrap();
    for file in files {
        collector.batch(ScanBatch {
            tag: partial,
            files: vec![file],
            progress: summary.progress,
        });
    }
    collector.complete(
        partial,
        Ok(ScanSummary {
            tag: partial,
            ..summary
        }),
    );
    assert_eq!(collector.files.len(), 2);
    assert!(collector.error.is_some());
}

struct TestClock;
impl Clock for TestClock {
    fn now_millis(&self) -> u64 {
        0
    }
}
#[tokio::test]
async fn actual_bounded_worker_scan_emits_batches_and_main_thread_publishes_summary() {
    let directory = tempfile::tempdir().unwrap();
    for index in 0..260 {
        put(directory.path(), &format!("src/{index}.rs"));
    }
    let project = configured(directory.path(), &["src"]);
    let mut collector = ScanCollector::new(project.generation);
    let tag = collector.begin().unwrap();
    let batches = Arc::new(Mutex::new(vec![]));
    let sink = batches.clone();
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(TestClock)));
    scheduler
        .submit_blocking(OwnerId(1), tag, Delivery::Cancellable, move |cancel| {
            scan(&project, tag, cancel, |batch| {
                sink.lock().unwrap().push(batch);
                Ok(())
            })
        })
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap();
    for batch in batches.lock().unwrap().drain(..) {
        assert!(collector.batch(batch));
    }
    assert!(collector.complete(result.tag, result.result));
    assert_eq!(collector.files.len(), 260);
    assert_eq!(collector.progress.accepted, 260);
    assert!(scheduler.shutdown().await.is_empty());
}
