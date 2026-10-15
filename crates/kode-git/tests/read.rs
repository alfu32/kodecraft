use git2::{Oid, Repository, RepositoryInitOptions, Signature};
use kode_core::{DomainError, JobTag, RequestId, WorkspaceGeneration};
use kode_git::{
    model::*,
    read::{ReadJob, ReadOutput, ReadRequest, score_semantic_tag},
    repository::discover,
};
use kode_platform::jobs::{CancellationToken, MonotonicClock, Notifications, OwnerId, Scheduler};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
fn init(path: &Path) -> Repository {
    let mut options = RepositoryInitOptions::new();
    options.initial_head("main");
    options.external_template(false);
    let repo = Repository::init_opts(path, &options).unwrap();
    repo.config()
        .unwrap()
        .set_bool("core.autocrlf", false)
        .unwrap();
    repo.config()
        .unwrap()
        .set_str(
            "core.attributesfile",
            path.join(".git/no-global-attributes").to_str().unwrap(),
        )
        .unwrap();
    repo
}
fn stage(repo: &Repository, path: &str) {
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(path)).unwrap();
    index.write().unwrap();
}
fn remove(repo: &Repository, path: &str) {
    let mut index = repo.index().unwrap();
    index.remove_path(Path::new(path)).unwrap();
    index.write().unwrap();
}
fn commit(repo: &Repository, message: &str) -> Oid {
    let mut index = repo.index().unwrap();
    let oid = index.write_tree().unwrap();
    let tree = repo.find_tree(oid).unwrap();
    let signature = Signature::new(
        "Fixture Author",
        "fixture@example.invalid",
        &git2::Time::new(1_700_000_000, 120),
    )
    .unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
    let parents = parent.iter().collect::<Vec<_>>();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
    .unwrap()
}
fn run(root: &Path, request: ReadRequest) -> ReadOutput {
    ReadJob::new(root.into(), request)
        .run(&CancellationToken::new())
        .unwrap()
}
fn statuses(root: &Path) -> Listing<StatusEntry> {
    match run(root, ReadRequest::Status) {
        ReadOutput::Status(status) => status,
        _ => panic!("wrong status output"),
    }
}
fn diff(root: &Path, path: &str, staged: bool) -> DiffContents {
    match run(
        root,
        ReadRequest::Diff {
            path: path.into(),
            staged,
        },
    ) {
        ReadOutput::Diff(content) => content,
        _ => panic!("wrong diff output"),
    }
}
fn files(root: &Path, oid: Oid) -> Listing<PathBuf> {
    match run(
        root,
        ReadRequest::Files {
            revision: oid.to_string(),
        },
    ) {
        ReadOutput::Files(files) => files,
        _ => panic!("wrong file output"),
    }
}
fn fingerprint(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, output);
            } else if metadata.is_file() {
                output.insert(
                    path.strip_prefix(root).unwrap().into(),
                    (fs::read(&path).unwrap(), metadata.modified().unwrap()),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn discovery_unborn_detached_nested_linked_and_bare_repositories_remain_read_only() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let repo = init(&root);
    fs::create_dir_all(root.join("src/deep")).unwrap();
    let before = fingerprint(repo.path());
    assert!(
        matches!(run(&root,ReadRequest::Head),ReadOutput::Head(Head::Unborn {name,..}) if name=="main")
    );
    let location = discover(&root.join("src/deep"), &CancellationToken::new())
        .unwrap()
        .unwrap();
    assert_eq!(location.work_tree.as_deref(), Some(root.as_path()));
    assert!(!location.bare);
    assert!(
        matches!(run(&root,ReadRequest::Commits {limit:None}),ReadOutput::Commits(Listing{entries,truncated:false}) if entries.is_empty())
    );
    assert_eq!(fingerprint(repo.path()), before);
    fs::write(root.join("file.txt"), "base\n").unwrap();
    stage(&repo, "file.txt");
    assert!(
        statuses(&root)
            .entries
            .iter()
            .any(|entry| entry.path == Path::new("file.txt")
                && entry.staged()
                && entry.code == "A")
    );
    let unborn = diff(&root, "file.txt", true);
    assert_eq!(unborn.old.kind, ContentKind::Missing);
    assert_eq!(unborn.new.text().as_deref(), Some("base\n"));
    let oid = commit(&repo, "first");
    assert_eq!(
        discover(&root.join("file.txt"), &CancellationToken::new())
            .unwrap()
            .unwrap(),
        location
    );
    repo.set_head_detached(oid).unwrap();
    assert!(
        matches!(run(&root,ReadRequest::Head),ReadOutput::Head(Head::Detached {commit}) if commit==oid.to_string())
    );
    repo.set_head("refs/heads/main").unwrap();
    let linked = directory.path().join("linked");
    repo.worktree("linked", &linked, None).unwrap();
    let linked_location = discover(&linked, &CancellationToken::new())
        .unwrap()
        .unwrap();
    assert_eq!(linked_location.work_tree.as_deref(), Some(linked.as_path()));
    assert_ne!(linked_location.git_dir, location.git_dir);
    let before = fingerprint(repo.path());
    let linked_before = fingerprint(&linked_location.git_dir);
    assert!(
        matches!(run(&linked,ReadRequest::Head),ReadOutput::Head(Head::Branch {name,..}) if name=="linked")
    );
    statuses(&linked);
    assert_eq!(fingerprint(repo.path()), before);
    assert_eq!(fingerprint(&linked_location.git_dir), linked_before);
    let bare = directory.path().join("bare.git");
    Repository::init_bare(&bare).unwrap();
    assert!(
        discover(&bare, &CancellationToken::new())
            .unwrap()
            .unwrap()
            .bare
    );
    match run(
        &bare,
        ReadRequest::Snapshot {
            history_limit: None,
        },
    ) {
        ReadOutput::Snapshot(snapshot) => {
            assert!(snapshot.errors.iter().any(|error| error.contains("status")));
            assert!(snapshot.commits.entries.is_empty());
        }
        _ => panic!("wrong snapshot output"),
    }
}

#[test]
fn staged_unstaged_untracked_deleted_and_renamed_rows_have_exact_index_head_worktree_contents_without_writes()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    for (name, text) in [
        ("tracked.txt", "base\n"),
        ("delete.txt", "gone\n"),
        ("old.txt", "rename body\n"),
        (".gitignore", "# comment\nignored/\n!ignored/keep\n"),
    ] {
        fs::write(root.join(name), text).unwrap();
        stage(&repo, name);
    }
    commit(&repo, "baseline");
    fs::write(root.join("tracked.txt"), "staged\n").unwrap();
    stage(&repo, "tracked.txt");
    fs::write(root.join("tracked.txt"), "worktree\n").unwrap();
    fs::remove_file(root.join("delete.txt")).unwrap();
    fs::rename(root.join("old.txt"), root.join("new.txt")).unwrap();
    remove(&repo, "old.txt");
    stage(&repo, "new.txt");
    fs::write(root.join("binary.dat"), [0, 1, 2, 255]).unwrap();
    fs::create_dir(root.join("ignored")).unwrap();
    fs::write(root.join("ignored/skipped"), "ignore").unwrap();
    fs::write(root.join("naïve 😀.txt"), "unicode\r\n").unwrap();
    let before = fingerprint(repo.path());
    let rows = statuses(root).entries;
    assert_eq!(
        rows.iter()
            .filter(|row| row.path == Path::new("tracked.txt"))
            .map(|row| (row.layer, row.code.as_str()))
            .collect::<Vec<_>>(),
        [(StatusLayer::Index, "M"), (StatusLayer::Worktree, "M")]
    );
    for (name, layer, code) in [
        ("delete.txt", StatusLayer::Worktree, "D"),
        ("old.txt", StatusLayer::Index, "D"),
        ("new.txt", StatusLayer::Index, "A"),
        ("binary.dat", StatusLayer::Worktree, "??"),
        ("naïve 😀.txt", StatusLayer::Worktree, "??"),
    ] {
        assert!(
            rows.iter()
                .any(|row| row.path == Path::new(name) && row.layer == layer && row.code == code),
            "missing {name}: {rows:?}"
        );
    }
    assert!(!rows.iter().any(|row| row.path.starts_with("ignored")));
    let staged = diff(root, "tracked.txt", true);
    assert_eq!(staged.old.text().as_deref(), Some("base\n"));
    assert_eq!(staged.new.text().as_deref(), Some("staged\n"));
    let unstaged = diff(root, "tracked.txt", false);
    assert_eq!(unstaged.old.text().as_deref(), Some("staged\n"));
    assert_eq!(unstaged.new.text().as_deref(), Some("worktree\n"));
    assert_eq!(
        diff(root, "delete.txt", false).new.kind,
        ContentKind::Missing
    );
    let binary = diff(root, "binary.dat", false);
    assert_eq!(binary.old.kind, ContentKind::Missing);
    assert_eq!(binary.new.kind, ContentKind::Binary);
    assert_eq!(binary.new.bytes, [0, 1, 2, 255]);
    assert!(
        matches!(run(root,ReadRequest::IgnoredPatterns),ReadOutput::IgnoredPatterns(patterns) if patterns==["ignored/","!ignored/keep"])
    );
    assert_eq!(
        fingerprint(repo.path()),
        before,
        "read operations changed repository bytes or mtimes"
    );
}

#[test]
fn original_large_object_regression_and_bounded_blob_reads_do_not_break_status_or_normalize_text() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    repo.config()
        .unwrap()
        .set_str("core.bigFileThreshold", "1k")
        .unwrap();
    let original: Vec<_> = (0..2048).map(|index| (index % 251) as u8).collect();
    fs::write(root.join("large.txt"), &original).unwrap();
    stage(&repo, "large.txt");
    fs::write(root.join("huge.txt"), vec![b'a'; 16 * 1024 * 1024]).unwrap();
    stage(&repo, "huge.txt");
    let exact = "\u{feff}// naïve 😀\r\nlast line".as_bytes();
    fs::write(root.join("exact.kt"), exact).unwrap();
    stage(&repo, "exact.kt");
    let oid = commit(&repo, "large blobs");
    fs::write(
        root.join("large.txt"),
        (0..2048)
            .map(|index| ((index + 1) % 251) as u8)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        statuses(root)
            .entries
            .iter()
            .any(|row| row.path == Path::new("large.txt") && row.code == "M" && !row.staged())
    );
    assert_eq!(diff(root, "large.txt", false).old.kind, ContentKind::Binary);
    for request in [
        ReadRequest::Content {
            revision: oid.to_string(),
            path: "huge.txt".into(),
        },
        ReadRequest::Diff {
            path: "huge.txt".into(),
            staged: false,
        },
    ] {
        match run(root, request) {
            ReadOutput::Content(content) => {
                assert_eq!(content.kind, ContentKind::TooLarge);
                assert_eq!(content.length, 16 * 1024 * 1024);
                assert!(content.bytes.is_empty());
            }
            ReadOutput::Diff(content) => {
                assert_eq!(content.old.kind, ContentKind::TooLarge);
                assert_eq!(content.new.kind, ContentKind::TooLarge);
                assert!(content.old.bytes.is_empty() && content.new.bytes.is_empty());
            }
            _ => panic!("wrong bounded output"),
        }
    }
    match run(
        root,
        ReadRequest::Content {
            revision: oid.to_string(),
            path: "exact.kt".into(),
        },
    ) {
        ReadOutput::Content(content) => {
            assert_eq!(content.kind, ContentKind::Text);
            assert_eq!(content.bytes, exact);
            assert_eq!(content.text().as_deref(), Some("// naïve 😀\r\nlast line"));
        }
        _ => panic!("wrong text output"),
    }
    assert_eq!(files(root, oid).entries.len(), 3);
}

#[test]
fn history_branches_root_commit_files_edited_renames_and_annotated_tag_scoring_match_source_rules()
{
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    let original = (0..20)
        .map(|line| format!("unchanged line {line}\n"))
        .collect::<String>();
    fs::write(root.join("old.txt"), &original).unwrap();
    fs::write(root.join("remove.txt"), "delete\n").unwrap();
    stage(&repo, "old.txt");
    stage(&repo, "remove.txt");
    let first = commit(&repo, "  first message\n\nbody\n ");
    let first_commit = repo.find_commit(first).unwrap();
    repo.branch("feature/topic", &first_commit, false).unwrap();
    repo.branch("other/topic", &first_commit, false).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let object = repo.find_object(first, None).unwrap();
    for tag in ["v1.2", "v2.0", "release/v1.12", "plain"] {
        repo.tag(tag, &object, &signature, "annotated", false)
            .unwrap();
    }
    repo.tag_lightweight("v900.0", &object, false).unwrap();
    let blob = repo.blob(b"tagged blob").unwrap();
    repo.tag(
        "blob-tag",
        &repo.find_object(blob, None).unwrap(),
        &signature,
        "not a commit",
        false,
    )
    .unwrap();
    fs::rename(root.join("old.txt"), root.join("new.txt")).unwrap();
    fs::write(
        root.join("new.txt"),
        original.replacen("unchanged line 0", "edited line zero", 1),
    )
    .unwrap();
    fs::remove_file(root.join("remove.txt")).unwrap();
    remove(&repo, "old.txt");
    remove(&repo, "remove.txt");
    stage(&repo, "new.txt");
    let second = commit(&repo, "second");
    assert_eq!(
        files(root, first).entries,
        [PathBuf::from("old.txt"), PathBuf::from("remove.txt")]
    );
    assert_eq!(
        files(root, second).entries,
        [PathBuf::from("new.txt"), PathBuf::from("remove.txt")]
    );
    match run(root, ReadRequest::Commits { limit: None }) {
        ReadOutput::Commits(history) => {
            assert_eq!(history.entries.len(), 2);
            assert!(!history.truncated);
            assert_eq!(history.entries[0].hash, second.to_string());
            let root_commit = &history.entries[1];
            assert_eq!(root_commit.tag.as_deref(), Some("v2.0"));
            assert_eq!(root_commit.message, "first message\n\nbody");
            assert_eq!(root_commit.author, "Fixture Author");
            assert_eq!(root_commit.timestamp, 1_700_000_000);
            assert_eq!(root_commit.offset_minutes, 120);
        }
        _ => panic!("wrong history output"),
    }
    assert!(
        matches!(run(root,ReadRequest::Commits {limit:Some(1)}),ReadOutput::Commits(Listing {entries,truncated:true}) if entries.len()==1)
    );
    assert!(
        matches!(run(root,ReadRequest::Branches),ReadOutput::Branches(Listing {entries,..}) if entries.iter().map(|branch|branch.name.as_str()).collect::<Vec<_>>()==["feature/topic","main","other/topic"])
    );
    match run(root, ReadRequest::Tags) {
        ReadOutput::Tags(tags) => {
            assert_eq!(tags.entries.len(), 6);
            assert!(
                tags.entries
                    .iter()
                    .any(|tag| tag.name == "v900.0" && !tag.annotated)
            );
            assert!(
                tags.entries
                    .iter()
                    .any(|tag| tag.name == "blob-tag" && tag.commit.is_none())
            );
        }
        _ => panic!("wrong tags output"),
    }
    assert_eq!(
        score_semantic_tag("release-v1.12.3"),
        Some((1 << 20) | (12 << 10) | 3)
    );
    assert_eq!(score_semantic_tag("plain"), None);
    assert_eq!(score_semantic_tag("v1024.1"), None);
    // The source filters unparseable Long digit runs before scoring the rest.
    assert_eq!(score_semantic_tag("v999999999999999999999999.2"), Some(2));
    match run(
        root,
        ReadRequest::Content {
            revision: first.to_string(),
            path: "old.txt".into(),
        },
    ) {
        ReadOutput::Content(content) => {
            assert_eq!(content.text().as_deref(), Some(original.as_str()))
        }
        _ => panic!("wrong old content"),
    }
    assert!(matches!(
        run(
            root,
            ReadRequest::Content {
                revision: second.to_string(),
                path: "old.txt".into()
            }
        ),
        ReadOutput::Content(Content {
            kind: ContentKind::Missing,
            ..
        })
    ));
}

#[test]
fn real_merge_conflicts_are_reported_and_never_substituted_with_empty_index_content() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("both.txt"), "base\n").unwrap();
    stage(&repo, "both.txt");
    let base = commit(&repo, "base");
    fs::write(root.join("both.txt"), "ours\n").unwrap();
    fs::write(root.join("added.txt"), "ours add\n").unwrap();
    stage(&repo, "both.txt");
    stage(&repo, "added.txt");
    let ours = commit(&repo, "ours");
    let mut index = repo.index().unwrap();
    for (name, body) in [("both.txt", "theirs\n"), ("added.txt", "theirs add\n")] {
        let mut entry = index.get_path(Path::new(name), 0).unwrap();
        entry.id = repo.blob(body.as_bytes()).unwrap();
        entry.file_size = body.len() as u32;
        index.add(&entry).unwrap();
    }
    let tree = index.write_tree().unwrap();
    let tree = repo.find_tree(tree).unwrap();
    let signature = Signature::now("Fixture", "fixture@example.invalid").unwrap();
    let theirs = repo
        .commit(
            Some("refs/heads/side"),
            &signature,
            &signature,
            "theirs",
            &tree,
            &[&repo.find_commit(base).unwrap()],
        )
        .unwrap();
    // The temporary index used above is not written; the disk index/worktree
    // still match ours before invoking an actual libgit2 merge.
    assert_eq!(repo.head().unwrap().target(), Some(ours));
    index.read(true).unwrap();
    repo.merge(&[&repo.find_annotated_commit(theirs).unwrap()], None, None)
        .unwrap();
    let before = fingerprint(repo.path());
    let rows = statuses(root).entries;
    for (name, code) in [("both.txt", "UU"), ("added.txt", "AA")] {
        assert!(
            rows.iter().any(|row| row.path == Path::new(name)
                && row.layer == StatusLayer::Conflict
                && row.code == code),
            "{rows:?}"
        );
    }
    let failure = ReadJob::new(
        root.into(),
        ReadRequest::Diff {
            path: "both.txt".into(),
            staged: false,
        },
    )
    .run(&CancellationToken::new())
    .unwrap_err();
    assert!(failure.to_string().contains("unresolved merge stages"));
    assert_eq!(fingerprint(repo.path()), before);
}

#[test]
fn absent_corrupt_repositories_and_partial_refresh_errors_preserve_context_instead_of_panicking() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    assert!(discover(root, &CancellationToken::new()).unwrap().is_none());
    fs::create_dir(root.join(".git")).unwrap();
    assert!(
        matches!(discover(root,&CancellationToken::new()),Err(DomainError::Git {operation,..}) if operation=="discover repository")
    );
    let repo = init(root);
    fs::write(root.join("file.txt"), "text\n").unwrap();
    stage(&repo, "file.txt");
    let oid = commit(&repo, "baseline");
    let git_dir = repo.path().to_owned();
    drop(repo);
    let id = oid.to_string();
    let corrupt = git_dir.join("corrupt-commit-fixture");
    fs::write(&corrupt, b"corrupt fixture commit").unwrap();
    fs::rename(
        corrupt,
        git_dir.join("objects").join(&id[..2]).join(&id[2..]),
    )
    .unwrap();
    match run(
        root,
        ReadRequest::Snapshot {
            history_limit: None,
        },
    ) {
        ReadOutput::Snapshot(snapshot) => {
            assert!(snapshot.head.is_none());
            assert!(!snapshot.errors.is_empty());
            assert!(snapshot.errors.iter().any(|error| error.contains("HEAD")));
            assert!(
                snapshot
                    .branches
                    .entries
                    .iter()
                    .any(|branch| branch.name == "main")
            );
        }
        _ => panic!("wrong failure snapshot"),
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        ReadJob::new(root.into(), ReadRequest::Status).run(&cancelled),
        Err(DomainError::Cancelled)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_non_utf8_paths_submodule_content_and_parent_escape_are_explicit() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    let repo = init(&root);
    let outside = directory.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("file"), "do not read through parent link").unwrap();
    symlink(outside.join("file"), root.join("link")).unwrap();
    stage(&repo, "link");
    let non_utf8 = PathBuf::from(std::ffi::OsString::from_vec(b"path-\xff.txt".to_vec()));
    fs::write(root.join(&non_utf8), "raw path\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(&non_utf8).unwrap();
    index.write().unwrap();
    let child = init(&root.join("module"));
    fs::write(root.join("module/child.txt"), "submodule\n").unwrap();
    stage(&child, "child.txt");
    let child_first = commit(&child, "child first");
    stage(&repo, "module");
    let oid = commit(&repo, "links and paths");
    let link = diff(&root, "link", false);
    assert_eq!(link.old.kind, ContentKind::Symlink);
    assert_eq!(link.new.kind, ContentKind::Symlink);
    assert_ne!(
        link.new.text().as_deref(),
        Some("do not read through parent link")
    );
    match run(
        &root,
        ReadRequest::Content {
            revision: oid.to_string(),
            path: non_utf8.clone(),
        },
    ) {
        ReadOutput::Content(content) => assert_eq!(content.text().as_deref(), Some("raw path\n")),
        _ => panic!("wrong raw path content"),
    }
    fs::write(root.join(&non_utf8), "changed\n").unwrap();
    assert!(
        statuses(&root)
            .entries
            .iter()
            .any(|row| row.path == non_utf8)
    );
    fs::write(root.join("module/child.txt"), "submodule changed\n").unwrap();
    stage(&child, "child.txt");
    let child_second = commit(&child, "child second");
    let module = diff(&root, "module", false);
    assert_eq!(module.old.kind, ContentKind::Submodule);
    assert_eq!(module.old.object, Some(child_first.to_string()));
    assert_eq!(module.new.object, Some(child_second.to_string()));
    symlink(&outside, root.join("escape")).unwrap();
    for name in ["../outside/file", ".git/config", "escape/file"] {
        assert!(
            ReadJob::new(
                root.clone(),
                ReadRequest::Diff {
                    path: name.into(),
                    staged: false
                }
            )
            .run(&CancellationToken::new())
            .is_err(),
            "accepted {name}"
        );
    }
}

#[tokio::test]
async fn real_f03_blocking_jobs_deliver_owned_tagged_results_and_honor_limits() {
    let directory = tempfile::tempdir().unwrap();
    init(directory.path());
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    let tag = JobTag {
        workspace: WorkspaceGeneration(42),
        request: RequestId::fresh().unwrap(),
        document: None,
    };
    let owner = OwnerId(77);
    ReadJob::new(directory.path().into(), ReadRequest::Head)
        .schedule(&mut scheduler, owner, tag)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(completion) = scheduler.try_next() {
            assert_eq!(completion.tag, tag);
            assert_eq!(completion.owner, owner);
            assert!(matches!(
                completion.result.unwrap(),
                ReadOutput::Head(Head::Unborn { .. })
            ));
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(scheduler.shutdown().await.is_empty());
    fs::write(directory.path().join("one"), "1").unwrap();
    fs::write(directory.path().join("two"), "2").unwrap();
    let mut job = ReadJob::new(directory.path().into(), ReadRequest::Status);
    job.limits.entries = 1;
    assert!(
        matches!(job.run(&CancellationToken::new()).unwrap(),ReadOutput::Status(Listing {entries,truncated:true}) if entries.len()==1)
    );
}
