use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Revision, TextRange,
    WorkspaceGeneration, commands::EditKind, document::Document,
};
use kode_platform::jobs::{CancellationToken, MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_workspace::{mime::MimeDetector, project::Project, search::*};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
fn put(root: &Path, path: &str, text: &[u8]) -> PathBuf {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}
fn job(root: &Path, query: &str, filter: &str) -> ProjectSearchJob {
    ProjectSearchJob {
        tag: JobTag {
            workspace: WorkspaceGeneration(1),
            request: RequestId::fresh().unwrap(),
            document: None,
        },
        project: Project::new(root, WorkspaceGeneration(1)).unwrap(),
        query: query.into(),
        file_filter: filter.into(),
        detector: MimeDetector::compiled(),
        open_documents: vec![],
        styler: None,
    }
}
#[test]
fn whole_project_scope_includes_non_source_hidden_ignored_excluded_and_build_text_only() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for path in [
        "src/code.rs",
        "other/note.txt",
        ".hidden/note.txt",
        "ignored/note.txt",
        "excluded/note.txt",
        "build/output.txt",
        "target/output.txt",
    ] {
        put(root, path, b"needle");
    }
    put(root, ".gitignore", b"ignored/\n");
    for path in [
        ".git/internal",
        ".kode/private.txt",
        ".kode-rs/private.txt",
        ".kode.json",
        ".kode-rs.json",
        ".kode-rs.json.previous",
    ] {
        put(root, path, b"needle");
    }
    put(root, "binary.bin", b"\0needle\0");
    put(root, "image.png", b"\x89PNG\r\n\x1a\nneedle");
    let mut job = job(root, "needle", "");
    job.project.config.exclusions.push("excluded".into());
    let report = search(&job, &CancellationToken::new()).unwrap();
    assert_eq!(report.hits.len(), 7);
    assert!(report.warnings.is_empty());
    assert!(
        report
            .hits
            .iter()
            .any(|hit| hit.path.ends_with("target/output.txt"))
    );
    assert!(report.hits.iter().all(|hit| hit.disk_identity.is_some()));
    job.file_filter = r"\.rs$".into();
    let report = search(&job, &CancellationToken::new()).unwrap();
    assert_eq!(report.hits.len(), 1);
    assert!(report.hits[0].path.ends_with("src/code.rs"));
}
#[test]
fn per_line_java_regex_utf8_ranges_crlf_bom_zero_width_and_file_filter_errors_are_exact() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "unicode.txt", "\u{feff}a😀b\r\na😀b\r\n".as_bytes());
    let report = search(&job(root, r"(?<=a)😀", ""), &CancellationToken::new()).unwrap();
    assert_eq!(report.hits.len(), 2);
    assert_eq!(
        report.hits[0].range,
        TextRange {
            start: ByteOffset(1),
            end: ByteOffset(5)
        }
    );
    assert_eq!(
        report.hits[1].range,
        TextRange {
            start: ByteOffset(9),
            end: ByteOffset(13)
        }
    );
    assert_eq!(report.hits[1].location.line.0, 1);
    assert_eq!(report.hits[1].location.column.0, 1);
    assert!(
        search(&job(root, r"b\ra", ""), &CancellationToken::new())
            .unwrap()
            .hits
            .is_empty()
    );
    let report = search(&job(root, "^", ""), &CancellationToken::new()).unwrap();
    assert_eq!(report.hits.len(), 2);
    assert!(report.hits.iter().all(|hit| hit.range.is_empty()));
    assert!(search(&job(root, "[", ""), &CancellationToken::new()).is_err());
    assert!(search(&job(root, "needle", "["), &CancellationToken::new()).is_err());
    assert!(
        search(&job(root, "   ", "["), &CancellationToken::new())
            .unwrap()
            .hits
            .is_empty()
    );
    put(root, "empty.txt", b"");
    let report = search(&job(root, "^", "empty"), &CancellationToken::new()).unwrap();
    assert!(report.hits.is_empty());
}
#[test]
fn current_unsaved_snapshot_is_searched_without_reloading_or_replacing_its_history() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let path = put(root, "code.rs", b"disk");
    let mut document = Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::File(path.clone()),
        Some("rust".into()),
        "disk",
    );
    document
        .apply_edit(
            Revision(0),
            TextRange {
                start: ByteOffset(0),
                end: ByteOffset(4),
            },
            "unsaved needle",
            EditKind::Typing,
        )
        .unwrap();
    let snapshot = document.snapshot();
    let mut job = job(root, "needle", "");
    job.open_documents.push(snapshot.clone());
    let report = search(&job, &CancellationToken::new()).unwrap();
    assert_eq!(report.hits.len(), 1);
    let hit = &report.hits[0];
    assert_eq!(hit.document, Some((snapshot.id, snapshot.revision)));
    assert!(hit.matches_snapshot(&snapshot));
    assert_eq!(fs::read_to_string(path).unwrap(), "disk");
    document
        .apply_edit(
            snapshot.revision,
            TextRange {
                start: ByteOffset(0),
                end: ByteOffset(0),
            },
            "prefix ",
            EditKind::Typing,
        )
        .unwrap();
    assert!(!hit.matches_snapshot(&document.snapshot()));
    assert!(document.is_dirty());
    assert_eq!(document.history_depth(), (2, 0));
}
#[test]
fn cancellation_old_generation_symlink_cycles_and_non_regular_targets_do_not_publish() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "file.txt", b"needle");
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        search(&job(root, "needle", ""), &cancel),
        Err(DomainError::Cancelled)
    ));
    let mut old = job(root, "needle", "");
    old.tag.workspace = WorkspaceGeneration(2);
    assert!(matches!(
        search(&old, &CancellationToken::new()),
        Err(DomainError::Cancelled)
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let file = put(outside.path(), "outside.txt", b"needle");
        symlink(file, root.join("outside.txt")).unwrap();
        symlink(root, root.join("cycle")).unwrap();
        symlink(root.join("file.txt"), root.join("alias.txt")).unwrap();
        let report = search(&job(root, "needle", ""), &CancellationToken::new()).unwrap();
        assert_eq!(report.hits.len(), 1);
    }
}
#[test]
fn size_decode_and_match_limits_report_failure_without_unbounded_or_partial_success() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    put(root, "invalid.json", b"\xff");
    let huge = fs::File::create(root.join("huge.txt")).unwrap();
    huge.set_len(MAX_FILE_BYTES + 1).unwrap();
    let report = search(&job(root, "needle", ""), &CancellationToken::new()).unwrap();
    assert!(report.hits.is_empty());
    assert_eq!(report.warnings.len(), 2);
    put(root, "matches.txt", &vec![b'x'; MAX_MATCHES + 1]);
    assert!(matches!(
        search(&job(root, "x", "matches"), &CancellationToken::new()),
        Err(DomainError::Unsupported(_))
    ));
}
#[tokio::test]
async fn actual_search_worker_returns_owned_ranges_and_prepared_syntax() {
    struct Styler;
    impl SearchStyler for Styler {
        fn line_spans(
            &self,
            _: &Path,
            _: Option<&str>,
            text: &str,
        ) -> kode_core::Result<Vec<kode_core::intelligence::HighlightSpan>> {
            Ok(vec![kode_core::intelligence::HighlightSpan {
                range: TextRange {
                    start: ByteOffset(0),
                    end: ByteOffset(text.len()),
                },
                class: "keyword".into(),
            }])
        }
    }
    let directory = tempfile::tempdir().unwrap();
    put(directory.path(), "code.rs", b"needle needle");
    let mut job = job(directory.path(), "needle", "");
    job.styler = Some(Arc::new(Styler));
    let tag = job.tag;
    let mut scheduler = Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_search(&mut scheduler, OwnerId(8), job).unwrap();
    let completion = scheduler.next().await.unwrap();
    assert_eq!(completion.tag, tag);
    let report = completion.result.unwrap();
    assert_eq!(report.hits.len(), 2);
    assert_eq!(report.hits[0].syntax.len(), 1);
    assert_eq!(report.hits[0].text, report.hits[1].text);
    assert!(scheduler.shutdown().await.is_empty());
}
