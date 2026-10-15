use git2::{Oid, Repository};
use kode_core::DomainError;
use kode_git::{
    model::*,
    read::{ReadJob, ReadOutput, ReadRequest},
    write::{WriteJob, WriteReceipt, WriteRequest},
};
use kode_platform::jobs::CancellationToken;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn run(root: &Path, request: WriteRequest) -> kode_core::Result<WriteReceipt> {
    WriteJob {
        root: root.into(),
        request,
    }
    .run(&CancellationToken::new())
}
fn init(root: &Path) -> Repository {
    run(root, WriteRequest::Init).unwrap();
    let repo = Repository::open(root).unwrap();
    let mut config = repo.config().unwrap();
    config.set_bool("core.autocrlf", false).unwrap();
    config
        .set_str(
            "core.attributesfile",
            root.join(".git/no-global-attributes").to_str().unwrap(),
        )
        .unwrap();
    config.set_str("user.name", "Synthetic Author").unwrap();
    config
        .set_str("user.email", "synthetic@example.invalid")
        .unwrap();
    repo
}
fn stage(root: &Path, names: &[&str]) {
    run(
        root,
        WriteRequest::Stage(names.iter().map(PathBuf::from).collect()),
    )
    .unwrap();
}
fn commit(root: &Path, message: &str) -> Oid {
    Oid::from_str(
        &run(
            root,
            WriteRequest::Commit {
                message: message.into(),
            },
        )
        .unwrap()
        .commit
        .unwrap(),
    )
    .unwrap()
}
fn status(root: &Path) -> Vec<StatusEntry> {
    match ReadJob::new(root.into(), ReadRequest::Status)
        .run(&CancellationToken::new())
        .unwrap()
    {
        ReadOutput::Status(status) => status.entries,
        _ => panic!("wrong status"),
    }
}
fn index(repo: &Repository) -> git2::Index {
    let mut index = repo.index().unwrap();
    index.read(true).unwrap();
    index
}
fn assert_no_lock(repo: &Repository) {
    assert!(!repo.path().join("index.lock").exists());
    assert!(!fs::read_dir(repo.path()).unwrap().any(|item| {
        item.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".kode-rs-index-")
    }));
}
#[test]
fn stage_unstage_multiple_deleted_unborn_and_literal_glob_paths_publish_the_expected_index_only() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    for name in ["a.txt", "b.txt", "literal*.txt"] {
        fs::write(root.join(name), format!("{name}\n")).unwrap();
    }
    stage(root, &["a.txt", "b.txt", "literal*.txt"]);
    assert_eq!(index(&repo).len(), 3);
    run(root, WriteRequest::Unstage(vec!["literal*.txt".into()])).unwrap();
    assert!(
        index(&repo)
            .get_path(Path::new("literal*.txt"), 0)
            .is_none()
    );
    assert!(index(&repo).get_path(Path::new("a.txt"), 0).is_some());
    let first = commit(root, "first commit");
    assert_eq!(repo.head().unwrap().target(), Some(first));
    let worktree = fs::read(root.join("a.txt")).unwrap();
    fs::write(root.join("a.txt"), "staged change\n").unwrap();
    fs::remove_file(root.join("b.txt")).unwrap();
    stage(root, &["a.txt", "b.txt"]);
    assert!(index(&repo).get_path(Path::new("b.txt"), 0).is_none());
    run(
        root,
        WriteRequest::Unstage(vec!["a.txt".into(), "b.txt".into()]),
    )
    .unwrap();
    assert_eq!(
        repo.find_blob(index(&repo).get_path(Path::new("a.txt"), 0).unwrap().id)
            .unwrap()
            .content(),
        worktree
    );
    assert!(index(&repo).get_path(Path::new("b.txt"), 0).is_some());
    assert!(!root.join("b.txt").exists());
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"staged change\n");
    stage(root, &["b.txt"]);
    let deleted = commit(root, "delete b");
    assert!(
        repo.find_commit(deleted)
            .unwrap()
            .tree()
            .unwrap()
            .get_path(Path::new("b.txt"))
            .is_err()
    );
    assert_no_lock(&repo);
}
#[test]
fn failed_or_cancelled_preparation_and_external_index_locks_never_publish_half_a_stage() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("one"), "base\n").unwrap();
    stage(root, &["one"]);
    commit(root, "baseline");
    let before = fs::read(repo.path().join("index")).unwrap();
    fs::write(root.join("one"), "edited\n").unwrap();
    let failure = run(
        root,
        WriteRequest::Stage(vec!["one".into(), "nonexistent".into()]),
    )
    .unwrap_err();
    assert!(failure.to_string().contains("stage deleted file"));
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    assert_no_lock(&repo);
    fs::write(
        repo.path().join("index.lock"),
        "other process owns this lock",
    )
    .unwrap();
    assert!(run(root, WriteRequest::Stage(vec!["one".into()])).is_err());
    assert_eq!(
        fs::read(repo.path().join("index.lock")).unwrap(),
        b"other process owns this lock"
    );
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    fs::remove_file(repo.path().join("index.lock")).unwrap();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        WriteJob {
            root: root.into(),
            request: WriteRequest::Stage(vec!["one".into()])
        }
        .run(&cancel),
        Err(DomainError::Cancelled)
    ));
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    assert_no_lock(&repo);
    for path in ["../outside", ".git/config"] {
        assert!(run(root, WriteRequest::Stage(vec![path.into()])).is_err());
    }
}
#[test]
fn commits_require_configured_identity_valid_message_and_staged_changes_and_keep_the_worktree_untouched()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("file"), "index snapshot\n").unwrap();
    stage(root, &["file"]);
    repo.config().unwrap().set_str("user.name", "").unwrap();
    repo.config().unwrap().set_str("user.email", "").unwrap();
    assert!(
        run(
            root,
            WriteRequest::Commit {
                message: "message".into()
            }
        )
        .is_err()
    );
    assert!(repo.head().is_err());
    assert_no_lock(&repo);
    repo.config()
        .unwrap()
        .set_str("user.name", "Synthetic Author")
        .unwrap();
    repo.config()
        .unwrap()
        .set_str("user.email", "synthetic@example.invalid")
        .unwrap();
    for message in ["", "  \n\t", "bad\0message"] {
        assert!(
            run(
                root,
                WriteRequest::Commit {
                    message: message.into()
                }
            )
            .is_err()
        );
        assert!(repo.head().is_err());
    }
    fs::write(root.join("file"), "newer unsaved worktree bytes\n").unwrap();
    let oid = commit(root, "Unicode naïve 😀\n\nbody");
    let committed = repo.find_commit(oid).unwrap();
    assert_eq!(committed.author().name_bytes(), b"Synthetic Author");
    assert_eq!(
        committed.message_bytes(),
        "Unicode naïve 😀\n\nbody".as_bytes()
    );
    let entry = committed
        .tree()
        .unwrap()
        .get_path(Path::new("file"))
        .unwrap();
    assert_eq!(
        repo.find_blob(entry.id()).unwrap().content(),
        b"index snapshot\n"
    );
    assert_eq!(
        fs::read(root.join("file")).unwrap(),
        b"newer unsaved worktree bytes\n"
    );
    assert!(
        run(
            root,
            WriteRequest::Commit {
                message: "nothing new".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("nothing staged")
    );
    assert_eq!(repo.head().unwrap().target(), Some(oid));
    assert_no_lock(&repo);
}
#[test]
fn checkout_preserves_dirty_and_untracked_files_never_forces_and_moves_head_only_after_success() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("file"), "first\n").unwrap();
    stage(root, &["file"]);
    let first = commit(root, "first");
    repo.branch("feature/topic", &repo.find_commit(first).unwrap(), false)
        .unwrap();
    fs::write(root.join("file"), "main second\n").unwrap();
    stage(root, &["file"]);
    let second = commit(root, "second");
    fs::write(root.join("file"), "dirty draft\n").unwrap();
    let before = fs::read(repo.path().join("index")).unwrap();
    assert!(
        run(
            root,
            WriteRequest::Checkout {
                branch: "feature/topic".into()
            }
        )
        .is_err()
    );
    assert_eq!(repo.head().unwrap().target(), Some(second));
    assert_eq!(fs::read(root.join("file")).unwrap(), b"dirty draft\n");
    assert_eq!(fs::read(repo.path().join("index")).unwrap(), before);
    assert_no_lock(&repo);
    fs::write(root.join("file"), "main second\n").unwrap();
    fs::write(root.join("unrelated-private"), "keep\n").unwrap();
    let receipt = run(
        root,
        WriteRequest::Checkout {
            branch: "feature/topic".into(),
        },
    )
    .unwrap();
    assert!(receipt.worktree_changed);
    assert_eq!(
        repo.head().unwrap().name_bytes(),
        b"refs/heads/feature/topic"
    );
    assert_eq!(repo.head().unwrap().target(), Some(first));
    assert_eq!(fs::read(root.join("file")).unwrap(), b"first\n");
    assert_eq!(fs::read(root.join("unrelated-private")).unwrap(), b"keep\n");
    assert!(
        status(root)
            .iter()
            .all(|entry| entry.kind == ChangeKind::Untracked)
    );
    assert!(
        !run(
            root,
            WriteRequest::Checkout {
                branch: "feature/topic".into()
            }
        )
        .unwrap()
        .worktree_changed
    );
    assert!(
        run(
            root,
            WriteRequest::Checkout {
                branch: "missing".into()
            }
        )
        .is_err()
    );
    assert_no_lock(&repo);
}
#[test]
fn tag_create_duplicate_and_explicit_move_keep_the_previous_tag_on_invalid_target() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let repo = init(root);
    fs::write(root.join("file"), "one\n").unwrap();
    stage(root, &["file"]);
    let first = commit(root, "first");
    fs::write(root.join("file"), "two\n").unwrap();
    stage(root, &["file"]);
    let second = commit(root, "second");
    let request = |revision: Oid, move_existing| WriteRequest::Tag {
        name: "release/v1.0".into(),
        revision: revision.to_string(),
        move_existing,
    };
    run(root, request(first, false)).unwrap();
    let previous = repo.refname_to_id("refs/tags/release/v1.0").unwrap();
    assert!(run(root, request(second, false)).is_err());
    assert_eq!(
        repo.refname_to_id("refs/tags/release/v1.0").unwrap(),
        previous
    );
    assert!(
        run(
            root,
            WriteRequest::Tag {
                name: "release/v1.0".into(),
                revision: "missing-target".into(),
                move_existing: true
            }
        )
        .is_err()
    );
    assert_eq!(
        repo.refname_to_id("refs/tags/release/v1.0").unwrap(),
        previous
    );
    run(root, request(second, true)).unwrap();
    assert_eq!(
        repo.find_reference("refs/tags/release/v1.0")
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id(),
        second
    );
    for name in ["bad..name", "bad\0name", "/bad", ""] {
        assert!(
            run(
                root,
                WriteRequest::Tag {
                    name: name.into(),
                    revision: first.to_string(),
                    move_existing: false
                }
            )
            .is_err()
        );
    }
}
#[test]
fn initialization_refuses_existing_metadata_and_checkout_refuses_a_branch_owned_by_another_worktree()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("project");
    fs::create_dir(&root).unwrap();
    let repo = init(&root);
    assert!(run(&root, WriteRequest::Init).is_err());
    fs::create_dir(root.join("nested")).unwrap();
    assert!(run(&root.join("nested"), WriteRequest::Init).is_err());
    assert!(!root.join("nested/.git").exists());
    fs::write(root.join("file"), "base\n").unwrap();
    stage(&root, &["file"]);
    commit(&root, "first");
    let linked = directory.path().join("linked");
    repo.worktree("linked", &linked, None).unwrap();
    let failure = run(
        &root,
        WriteRequest::Checkout {
            branch: "linked".into(),
        },
    )
    .unwrap_err();
    assert!(failure.to_string().contains("already checked out"));
    assert_no_lock(&repo);
    let head = Repository::open(&root)
        .unwrap()
        .head()
        .unwrap()
        .name_bytes()
        .to_vec();
    assert!(
        run(
            &linked,
            WriteRequest::Checkout {
                branch: String::from_utf8(head)
                    .unwrap()
                    .strip_prefix("refs/heads/")
                    .unwrap()
                    .into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already checked out")
    );
    let corrupt = directory.path().join("corrupt");
    fs::create_dir_all(corrupt.join(".git")).unwrap();
    fs::write(corrupt.join(".git/user.keep"), "keep").unwrap();
    assert!(run(&corrupt, WriteRequest::Init).is_err());
    assert_eq!(fs::read(corrupt.join(".git/user.keep")).unwrap(), b"keep");
}
