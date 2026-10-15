//! Whole-project, per-line regex search; deliberately independent of index roots.
use crate::{
    mime::{MimeCategory, MimeDetector},
    project::Project,
    tree::tree_metadata,
};
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, Result, Revision,
    TextRange,
    document::{Document, TextSnapshot, Utf16Position, encoding::decode},
    editor::search_regex::SearchRegex,
    intelligence::HighlightSpan,
    services::DiskIdentity,
};
use kode_platform::{
    files::open_regular_file,
    jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_MATCHES: usize = 100_000;
pub const MAX_WARNINGS: usize = 128;
pub const SEARCH_SCOPE: &str = "All project text files, including hidden, ignored, excluded and build files; skips Git/session metadata, directory links and binary files. Regex matches each line separately.";
pub trait SearchStyler: Send + Sync {
    /// Worker hook for configured lexical/semantic highlighting. Returned ranges
    /// are relative to this raw source line and must be valid UTF-8 boundaries.
    fn line_spans(
        &self,
        path: &Path,
        language: Option<&str>,
        text: &str,
    ) -> Result<Vec<HighlightSpan>>;
}
pub struct ProjectSearchJob {
    pub tag: JobTag,
    pub project: Project,
    pub query: String,
    pub file_filter: String,
    pub open_documents: Vec<TextSnapshot>,
    pub detector: MimeDetector,
    pub styler: Option<Arc<dyn SearchStyler>>,
}
#[derive(Debug, Clone)]
pub struct ProjectSearchHit {
    pub path: PathBuf,
    pub line: LineIndex,
    pub text: Arc<str>,
    pub range_in_line: TextRange,
    pub range: TextRange,
    pub location: Utf16Position,
    pub document: Option<(DocumentId, Revision)>,
    pub disk_identity: Option<DiskIdentity>,
    pub language: Option<String>,
    pub syntax: Vec<HighlightSpan>,
}
impl ProjectSearchHit {
    pub fn matches_snapshot(&self, snapshot: &TextSnapshot) -> bool {
        if self
            .document
            .is_some_and(|stamp| stamp != (snapshot.id, snapshot.revision))
        {
            return false;
        }
        snapshot
            .line_content(self.line)
            .is_ok_and(|line| line == self.text.as_ref())
            && snapshot.line_start(self.line).is_ok_and(|start| {
                start.0.checked_add(self.range_in_line.start.0) == Some(self.range.start.0)
            })
            && snapshot.byte_to_char(self.range.start).is_ok()
            && snapshot.byte_to_char(self.range.end).is_ok()
    }
}
pub struct ProjectSearchReport {
    pub tag: JobTag,
    pub query: String,
    pub file_filter: String,
    pub hits: Vec<ProjectSearchHit>,
    pub warnings: Vec<String>,
    pub omitted_warnings: usize,
    pub files_searched: usize,
}
fn cancelled(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
fn warning(report: &mut ProjectSearchReport, error: impl ToString) {
    if report.warnings.len() < MAX_WARNINGS {
        report.warnings.push(error.to_string());
    } else {
        report.omitted_warnings += 1;
    }
}
fn metadata_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).map_or(true, |relative| {
        relative.components().any(|part| {
            let name = part.as_os_str().to_string_lossy();
            name == ".git" || tree_metadata(&name)
        })
    })
}
pub fn search(job: &ProjectSearchJob, token: &CancellationToken) -> Result<ProjectSearchReport> {
    cancelled(token)?;
    if job.tag.workspace != job.project.generation {
        return Err(DomainError::Cancelled);
    }
    let mut report = ProjectSearchReport {
        tag: job.tag,
        query: job.query.clone(),
        file_filter: job.file_filter.clone(),
        hits: vec![],
        warnings: vec![],
        omitted_warnings: 0,
        files_searched: 0,
    };
    if job.query.trim().is_empty() {
        return Ok(report);
    }
    let pattern = SearchRegex::compile(&job.query)?;
    let filter = if job.file_filter.trim().is_empty() {
        None
    } else {
        Some(SearchRegex::compile(&job.file_filter)?)
    };
    let root = job.project.root.clone();
    let mut builder = ignore::WalkBuilder::new(&root);
    builder
        .standard_filters(false)
        .follow_links(false)
        .sort_by_file_name(Ord::cmp);
    let boundary = root.clone();
    builder.filter_entry(move |entry| !metadata_path(&boundary, entry.path()));
    let mut seen = HashSet::new();
    let mut walker = builder.build();
    loop {
        cancelled(token)?;
        let Some(entry) = walker.next() else {
            break;
        };
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warning(&mut report, error);
                continue;
            }
        };
        let path = entry.path();
        if !entry
            .file_type()
            .is_some_and(|kind| kind.is_file() || kind.is_symlink())
        {
            continue;
        }
        let relative = path.strip_prefix(&root).unwrap_or(path).to_string_lossy();
        if let Some(filter) = &filter
            && !filter.is_match(&relative)?
        {
            continue;
        }
        let path = match fs::canonicalize(path) {
            Ok(path) if path.starts_with(&root) && !metadata_path(&root, &path) => path,
            Ok(_) => continue,
            Err(error) => {
                warning(&mut report, format!("{}: {error}", path.display()));
                continue;
            }
        };
        if entry.file_type().is_some_and(|kind| kind.is_symlink()) && !path.is_file() {
            continue;
        }
        if !seen.insert(path.clone()) {
            continue;
        }
        match search_file(job, &pattern, &path, token, &mut report) {
            Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
            Err(
                error @ DomainError::Parse {
                    format: "search regex",
                    ..
                },
            ) => return Err(error),
            Err(error @ DomainError::Unsupported(_)) if report.hits.len() >= MAX_MATCHES => {
                return Err(error);
            }
            Err(error) => warning(&mut report, error),
            Ok(()) => {}
        }
    }
    Ok(report)
}
fn search_file(
    job: &ProjectSearchJob,
    pattern: &SearchRegex,
    path: &Path,
    token: &CancellationToken,
    report: &mut ProjectSearchReport,
) -> Result<()> {
    cancelled(token)?;
    let detected = job.detector.detect_file(path, token)?;
    if detected.category != MimeCategory::Text {
        return Ok(());
    }
    let open = job.open_documents.iter().find(
        |snapshot| matches!(&snapshot.origin, DocumentOrigin::File(origin) if origin == path),
    );
    let (snapshot, identity) = if let Some(snapshot) = open {
        (snapshot.clone(), None)
    } else {
        let mut file = open_regular_file(path)?;
        let before = file.metadata().map_err(|source| DomainError::Io {
            operation: "stat project search file",
            path: path.into(),
            source,
        })?;
        if before.len() > MAX_FILE_BYTES {
            return Err(DomainError::Unsupported(format!(
                "{} exceeds 16 MiB search limit",
                path.display()
            )));
        }
        let mut bytes = vec![];
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| DomainError::Io {
                operation: "read project search file",
                path: path.into(),
                source,
            })?;
        cancelled(token)?;
        let after = file.metadata().map_err(|source| DomainError::Io {
            operation: "stat searched file",
            path: path.into(),
            source,
        })?;
        if bytes.len() as u64 > MAX_FILE_BYTES
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err(DomainError::SaveConflict(path.into()));
        }
        let identity = DiskIdentity {
            canonical_path: path.into(),
            length: bytes.len() as u64,
            modified_nanos: after
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|time| time.as_nanos()),
            content_hash: format!("{:x}", Sha256::digest(&bytes)),
        };
        let document = Document::from_decoded(
            DocumentId(0),
            DocumentOrigin::File(path.into()),
            detected.language.clone(),
            decode(&bytes)?,
        );
        (document.snapshot(), Some(identity))
    };
    if snapshot.len_bytes() as u64 > MAX_FILE_BYTES {
        return Err(DomainError::Unsupported(format!(
            "{} exceeds 16 MiB search limit",
            path.display()
        )));
    }
    report.files_searched += 1;
    for line in 0..snapshot.len_lines() {
        cancelled(token)?;
        let index = LineIndex(line);
        let content = snapshot.line_content(index)?;
        // Files.readAllLines does not add a synthetic trailing empty line.
        if content.len_bytes() == 0 && line + 1 == snapshot.len_lines() {
            continue;
        }
        let text: Arc<str> = content.to_string().into();
        let mut syntax = None;
        pattern.for_each(&text, |range| {
            cancelled(token)?;
            if report.hits.len() >= MAX_MATCHES {
                return Err(DomainError::Unsupported(
                    "project search exceeds 100000 matches; narrow the pattern".into(),
                ));
            }
            if syntax.is_none() {
                let spans = job
                    .styler
                    .as_ref()
                    .map(|styler| styler.line_spans(path, detected.language.as_deref(), &text))
                    .transpose()?
                    .unwrap_or_default();
                for span in &spans {
                    span.range.validate(&text)?;
                }
                syntax = Some(spans);
            }
            let line_start = snapshot.line_start(index)?;
            let absolute = TextRange {
                start: ByteOffset(line_start.0 + range.start.0),
                end: ByteOffset(line_start.0 + range.end.0),
            };
            report.hits.push(ProjectSearchHit {
                path: path.into(),
                line: index,
                text: text.clone(),
                range_in_line: range,
                range: absolute,
                location: snapshot.byte_to_utf16(absolute.start)?,
                document: open.map(|snapshot| (snapshot.id, snapshot.revision)),
                disk_identity: identity.clone(),
                language: detected.language.clone(),
                syntax: syntax.as_ref().expect("prepared line styles").clone(),
            });
            Ok(())
        })?;
    }
    Ok(())
}
pub fn schedule_search(
    scheduler: &mut dyn WorkerScheduler<ProjectSearchReport>,
    owner: OwnerId,
    job: ProjectSearchJob,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Cancellable,
        Box::new(move |token| search(&job, &token)),
    )
}
