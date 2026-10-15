use kode_git::{
    diff::{self, DiffPlan, RestoreJob},
    model::{Content, ContentKind, DiffContents},
    read::ReadRequest,
    write::{WriteJob, WriteRequest},
};
use kode_platform::jobs::CancellationToken;
use std::{fs, path::Path, sync::Arc};
fn stage(root: &Path) {
    WriteJob {
        root: root.into(),
        request: WriteRequest::Stage(vec!["file.txt".into()]),
    }
    .run(&CancellationToken::new())
    .unwrap();
}
fn fixture(root: &Path, old: &[u8], new: Option<&[u8]>) {
    WriteJob {
        root: root.into(),
        request: WriteRequest::Init,
    }
    .run(&CancellationToken::new())
    .unwrap();
    let repo = git2::Repository::open(root).unwrap();
    let mut config = repo.config().unwrap();
    config.set_bool("core.autocrlf", false).unwrap();
    config
        .set_str(
            "core.attributesfile",
            root.join(".git/missing-attributes").to_str().unwrap(),
        )
        .unwrap();
    config
        .set_str(
            "core.excludesfile",
            root.join(".git/missing-excludes").to_str().unwrap(),
        )
        .unwrap();
    fs::write(root.join("file.txt"), old).unwrap();
    stage(root);
    if let Some(new) = new {
        fs::write(root.join("file.txt"), new).unwrap();
    } else {
        fs::remove_file(root.join("file.txt")).unwrap();
    }
}
fn plan(root: &Path) -> Arc<DiffPlan> {
    diff::read(
        root,
        ReadRequest::Diff {
            path: "file.txt".into(),
            staged: false,
        },
        &CancellationToken::new(),
    )
    .unwrap()
}
fn restore(root: &Path, plan: Arc<DiffPlan>, hunk: usize) -> RestoreJob {
    RestoreJob {
        root: root.into(),
        plan,
        hunk,
    }
}
#[test]
fn actual_selected_hunks_restore_insert_delete_replace_and_final_newline_without_touching_other_chunks_or_index()
 {
    for (old, new) in [
        (
            b"a\nb\nc\nd\ne\nf\ng\n".as_slice(),
            b"a\ninsert\nb\nc\nd\nCHANGED\nf\ng\n".as_slice(),
        ),
        (
            b"repeat\nx\nrepeat\ny\nrepeat\nz\n",
            b"repeat\nX\nrepeat\ny\nrepeat\nZ\n",
        ),
        (b"last\n", b"last"),
        (b"last", b"last\n"),
        (
            b"\xef\xbb\xbfhello\r\nkeep\r\nold\r\n",
            b"\xef\xbb\xbfhello\r\nkeep\r\nnew\r\n",
        ),
        (b"remove\nkeep\n", b"keep\n"),
        (b"", b"added\n"),
        (b"deleted\n", b""),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture(root, old, Some(new));
        let before = fs::read(root.join(".git/index")).unwrap();
        let plan = plan(root);
        assert!(!plan.hunks().is_empty(), "{old:?} → {new:?}");
        let selected = plan.hunks().len() - 1;
        let expected = plan.restored_bytes(selected).unwrap();
        restore(root, plan.clone(), selected)
            .run(&CancellationToken::new())
            .unwrap();
        assert_eq!(fs::read(root.join("file.txt")).unwrap(), expected);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), before);
        assert!(!root.join(".git/index.lock").exists());
        if plan.hunks().len() == 1 {
            assert_eq!(expected, old);
        } else {
            assert_ne!(expected, old, "unselected chunk must remain changed");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path(), b"deleted file\r\n", None);
    restore(dir.path(), plan(dir.path()), 0)
        .run(&CancellationToken::new())
        .unwrap();
    assert_eq!(
        fs::read(dir.path().join("file.txt")).unwrap(),
        b"deleted file\r\n"
    );
}
#[test]
fn stale_worktree_or_index_cancel_external_lock_and_staged_historical_binary_previews_refuse_restore()
 {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fixture(root, b"old\n", Some(b"new\n"));
    let captured = plan(root);
    let file = fs::OpenOptions::new()
        .write(true)
        .open(root.join("file.txt"))
        .unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(1))
        .unwrap();
    assert!(
        restore(root, captured, 0)
            .run(&CancellationToken::new())
            .is_err(),
        "identical bytes with a new disk revision are stale"
    );
    let captured = plan(root);
    fs::write(root.join("file.txt"), b"external\n").unwrap();
    assert!(
        restore(root, captured.clone(), 0)
            .run(&CancellationToken::new())
            .is_err()
    );
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"external\n");
    fs::write(root.join("file.txt"), b"new\n").unwrap();
    stage(root);
    assert!(
        restore(root, captured.clone(), 0)
            .run(&CancellationToken::new())
            .is_err()
    );
    let token = CancellationToken::new();
    token.cancel();
    assert!(restore(root, captured.clone(), 0).run(&token).is_err());
    let lock = root.join(".git/index.lock");
    fs::write(&lock, b"external").unwrap();
    assert!(
        restore(root, captured.clone(), 0)
            .run(&CancellationToken::new())
            .is_err()
    );
    assert_eq!(fs::read(&lock).unwrap(), b"external");
    fs::remove_file(lock).unwrap();
    for kind in 0..3 {
        let mut contents = captured.contents().clone();
        if kind == 0 {
            contents.staged = true;
        }
        if kind == 1 {
            contents.revision = Some("history".into());
        }
        if kind == 2 {
            contents.new.kind = ContentKind::Binary;
        }
        let plan = DiffPlan::build(contents, &CancellationToken::new()).unwrap();
        assert!(!plan.restorable());
        assert!(plan.restored_bytes(0).is_err());
    }
    assert_eq!(fs::read(root.join("file.txt")).unwrap(), b"new\n");
}

#[cfg(unix)]
#[test]
fn replacing_a_previewed_file_with_a_symlink_cannot_restore_into_an_external_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repo");
    fs::create_dir(&root).unwrap();
    fixture(&root, b"old\n", Some(b"new\n"));
    let captured = plan(&root);
    let external = directory.path().join("external.txt");
    fs::write(&external, b"new\n").unwrap();
    fs::remove_file(root.join("file.txt")).unwrap();
    std::os::unix::fs::symlink(&external, root.join("file.txt")).unwrap();
    assert!(
        restore(&root, captured, 0)
            .run(&CancellationToken::new())
            .is_err()
    );
    assert_eq!(fs::read(external).unwrap(), b"new\n");
    assert!(
        fs::symlink_metadata(root.join("file.txt"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[test]
fn source_and_line_limits_are_visible_and_empty_diff_has_no_fake_chunk() {
    let make = |bytes: Vec<u8>| DiffContents {
        path: "file".into(),
        staged: false,
        revision: None,
        old: Content::missing(),
        new: Content {
            kind: ContentKind::Text,
            length: bytes.len() as u64,
            bytes,
            object: None,
            mode: 0o100644,
        },
    };
    let empty = DiffPlan::build(make(vec![]), &CancellationToken::new()).unwrap();
    assert!(empty.hunks().is_empty());
    assert!(empty.restored_bytes(0).is_err());
    for bytes in [vec![b'a'; 8 * 1024 * 1024 + 1], vec![b'\n'; 200_001]] {
        let plan = DiffPlan::build(make(bytes), &CancellationToken::new()).unwrap();
        assert!(plan.notice().unwrap().contains("exceeds"));
        assert!(!plan.restorable());
    }
}

#[test]
fn native_repeated_line_algorithm_fixture() {
    for (index, (old, new)) in [
        ("a\nb\nc\na\nb\nc\n", "a\nc\nb\na\nc\nb\n"),
        ("a\nb\na\nb\na\n", "b\na\nb\na\nb\n"),
        (
            "repeat\nx\nrepeat\ny\nrepeat\nz\n",
            "repeat\nX\nrepeat\ny\nrepeat\nZ\n",
        ),
        ("a\nb\nc\nd\ne\nf\n", "b\nc\nNEW\nd\na\ne\nf\n"),
    ]
    .into_iter()
    .enumerate()
    {
        let make = |text: &str| Content {
            kind: ContentKind::Text,
            length: text.len() as u64,
            bytes: text.as_bytes().into(),
            object: None,
            mode: 0o100644,
        };
        let plan = DiffPlan::build(
            DiffContents {
                path: "file".into(),
                staged: false,
                revision: None,
                old: make(old),
                new: make(new),
            },
            &CancellationToken::new(),
        )
        .unwrap();
        let expected = match index {
            0 => vec![(1..2, 1..1), (3..4, 2..2), (5..5, 3..4), (6..6, 5..6)],
            1 => vec![(0..1, 0..0), (5..5, 4..5)],
            2 => vec![(1..2, 1..2), (5..6, 5..6)],
            _ => vec![(0..1, 0..0), (3..3, 2..3), (4..4, 4..5)],
        };
        assert_eq!(
            plan.hunks()
                .iter()
                .map(|hunk| (hunk.old_lines.clone(), hunk.new_lines.clone()))
                .collect::<Vec<_>>(),
            expected
        );
    }
}
