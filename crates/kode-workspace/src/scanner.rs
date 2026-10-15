use super::{
    exclusions::{Exclusions, IgnoreDecision, IgnoreRules, index_metadata},
    project::Project,
};
use ignore::WalkBuilder;
use kode_core::{DomainError, JobTag, RequestId, Result, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub const BATCH_SIZE: usize = 128;
pub const WARNING_LIMIT: usize = 128;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub length: u64,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ScanProgress {
    pub visited: u64,
    pub accepted: u64,
    pub skipped: u64,
}
#[derive(Debug, Clone)]
pub struct ScanBatch {
    pub tag: JobTag,
    pub files: Vec<ScannedFile>,
    pub progress: ScanProgress,
}
#[derive(Debug, Clone)]
pub struct ScanSummary {
    pub tag: JobTag,
    pub progress: ScanProgress,
    pub warnings: Vec<String>,
    pub omitted_warnings: usize,
    pub incomplete: bool,
}
#[derive(Default)]
struct Counters {
    visited: AtomicU64,
    skipped: AtomicU64,
}
struct Issues {
    warnings: Vec<String>,
    omitted: usize,
}
impl Issues {
    fn add(&mut self, message: String) {
        if self.warnings.len() < WARNING_LIMIT {
            self.warnings.push(message);
        } else {
            self.omitted += 1;
        }
    }
}
fn hidden(entry: &ignore::DirEntry) -> bool {
    if entry.file_name().to_string_lossy().starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        entry
            .metadata()
            .is_ok_and(|metadata| metadata.file_attributes() & 0x2 != 0)
    }
    #[cfg(not(windows))]
    false
}
fn directory_readable(_entry: &ignore::DirEntry) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // No read/execute bits means no usable contents even when tests run with
        // elevated effective privileges. Other permission failures are reported
        // by the walker and preserve the previous publication.
        _entry.metadata().is_ok_and(|metadata| {
            metadata.permissions().mode() & 0o444 != 0 && metadata.permissions().mode() & 0o111 != 0
        })
    }
    #[cfg(not(unix))]
    true
}

/// Cancellable worker traversal, not a renderer operation. Rule syntax comes
/// from `ignore`, with every built-in filter disabled and explicit policies here.
/// Only configured roots (and ancestors necessary to reach them) are traversed.
pub fn scan(
    project: &Project,
    tag: JobTag,
    cancel: CancellationToken,
    mut emit: impl FnMut(ScanBatch) -> Result<()>,
) -> Result<ScanSummary> {
    if cancel.is_cancelled() || tag.workspace != project.generation {
        return Err(DomainError::Cancelled);
    }
    let (roots, root_warnings) = project.resolved_roots();
    let counters = Arc::new(Counters::default());
    let issues = Arc::new(Mutex::new(Issues {
        warnings: vec![],
        omitted: 0,
    }));
    for message in root_warnings {
        issues.lock().expect("scan issues").add(message);
    }
    let rules = Arc::new(IgnoreRules::new(project));
    rules.validate()?;
    let exclusions = Arc::new(Exclusions::new(project)?);
    let mut accepted = 0;
    if !roots.is_empty() {
        let root = project.root.clone();
        let filtered_roots = roots.clone();
        let counts = counters.clone();
        let filter_issues = issues.clone();
        let filter_rules = rules.clone();
        let filter_exclusions = exclusions.clone();
        let token = cancel.clone();
        let mut walker = WalkBuilder::new(&project.root);
        walker
            .standard_filters(false)
            .follow_links(false)
            .same_file_system(false)
            .sort_by_file_name(|left, right| left.cmp(right))
            .filter_entry(move |entry| {
                counts.visited.fetch_add(1, Ordering::Relaxed);
                let reject = || {
                    counts.skipped.fetch_add(1, Ordering::Relaxed);
                    false
                };
                if token.is_cancelled() {
                    return reject();
                }
                let path = entry.path();
                let directory = entry.file_type().is_some_and(|kind| kind.is_dir());
                if index_metadata(&root, path) || filter_exclusions.contains(path) {
                    return reject();
                }
                let in_root = filtered_roots.iter().any(|source| path.starts_with(source));
                let ancestor =
                    directory && filtered_roots.iter().any(|source| source.starts_with(path));
                if !in_root && !ancestor {
                    return reject();
                }
                let decision = match filter_rules.entry(path, directory) {
                    Ok(decision) => decision,
                    Err(error) => {
                        filter_issues
                            .lock()
                            .expect("scan issues")
                            .add(error.to_string());
                        return reject();
                    }
                };
                if decision == IgnoreDecision::Ignore
                    || path != root && hidden(entry) && decision != IgnoreDecision::Include
                {
                    return reject();
                }
                if directory && !directory_readable(entry) {
                    filter_issues
                        .lock()
                        .expect("scan issues")
                        .add(format!("directory is unreadable: {}", path.display()));
                    return reject();
                }
                true
            });
        let mut seen = HashSet::new();
        let mut files = Vec::with_capacity(BATCH_SIZE);
        let mut walk = walker.build();
        loop {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let Some(item) = walk.next() else {
                break;
            };
            let entry = match item {
                Ok(entry) => entry,
                Err(error) => {
                    issues.lock().expect("scan issues").add(error.to_string());
                    continue;
                }
            };
            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = match fs::canonicalize(entry.path()) {
                Ok(path) => path,
                Err(error) => {
                    issues
                        .lock()
                        .expect("scan issues")
                        .add(format!("{}: {error}", entry.path().display()));
                    continue;
                }
            };
            if !path.starts_with(&project.root)
                || index_metadata(&project.root, &path)
                || exclusions.contains(&path)
            {
                counters.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    issues
                        .lock()
                        .expect("scan issues")
                        .add(format!("{}: {error}", path.display()));
                    continue;
                }
            };
            // Directory symlinks, devices, sockets and pipes never become files.
            if !metadata.is_file() || !seen.insert(path.clone()) {
                counters.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if path != entry.path() && rules.is_ignored(&path, false)? {
                counters.skipped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let relative = path
                .strip_prefix(&project.root)
                .expect("inside project")
                .to_path_buf();
            files.push(ScannedFile {
                path,
                relative,
                length: metadata.len(),
            });
            accepted += 1;
            if files.len() == BATCH_SIZE {
                emit(ScanBatch {
                    tag,
                    files: std::mem::replace(&mut files, Vec::with_capacity(BATCH_SIZE)),
                    progress: progress(&counters, accepted),
                })?;
            }
        }
        if !files.is_empty() {
            emit(ScanBatch {
                tag,
                files,
                progress: progress(&counters, accepted),
            })?;
        }
    }
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    for warning in rules.take_warnings() {
        issues.lock().expect("scan issues").add(warning);
    }
    let mut issues = issues.lock().expect("scan issues");
    Ok(ScanSummary {
        tag,
        progress: progress(&counters, accepted),
        incomplete: !issues.warnings.is_empty() || issues.omitted > 0,
        warnings: std::mem::take(&mut issues.warnings),
        omitted_warnings: issues.omitted,
    })
}
fn progress(counters: &Counters, accepted: u64) -> ScanProgress {
    ScanProgress {
        visited: counters.visited.load(Ordering::Relaxed),
        accepted,
        skipped: counters.skipped.load(Ordering::Relaxed),
    }
}

/// Main-thread staging: cancel/failure/workspace changes cannot replace a good
/// file list with a partial or unreadable scan. Index progress is a separate owner.
pub struct ScanCollector {
    workspace: WorkspaceGeneration,
    active: Option<JobTag>,
    pending: Vec<ScannedFile>,
    pub files: Arc<Vec<ScannedFile>>,
    pub progress: ScanProgress,
    pub summary: Option<ScanSummary>,
    pub error: Option<DomainError>,
}
impl ScanCollector {
    pub fn new(workspace: WorkspaceGeneration) -> Self {
        Self {
            workspace,
            active: None,
            pending: vec![],
            files: Arc::new(vec![]),
            progress: ScanProgress::default(),
            summary: None,
            error: None,
        }
    }
    pub fn begin(&mut self) -> Result<JobTag> {
        let tag = JobTag {
            workspace: self.workspace,
            request: RequestId::fresh()?,
            document: None,
        };
        self.active = Some(tag);
        self.pending.clear();
        self.progress = ScanProgress::default();
        self.summary = None;
        self.error = None;
        Ok(tag)
    }
    pub fn batch(&mut self, batch: ScanBatch) -> bool {
        if self.active != Some(batch.tag) {
            return false;
        }
        self.progress = batch.progress;
        self.pending.extend(batch.files);
        true
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<ScanSummary>) -> bool {
        if self.active != Some(tag) {
            return false;
        }
        match result {
            Ok(summary) if summary.tag == tag => {
                self.progress = summary.progress;
                if summary.progress.accepted != self.pending.len() as u64 {
                    self.error = Some(DomainError::Unsupported("scan final count does not match delivered batches; previous file list retained".into()));
                    self.pending.clear();
                } else if summary.incomplete {
                    self.error = Some(DomainError::Unsupported(
                        "scan incomplete; previous file list retained; inspect scan warnings"
                            .into(),
                    ));
                    self.pending.clear();
                } else {
                    self.files = Arc::new(std::mem::take(&mut self.pending));
                }
                self.summary = Some(summary);
            }
            Ok(_) => return false,
            Err(error) => {
                self.error = Some(error);
                self.pending.clear();
            }
        }
        self.active = None;
        true
    }
    pub fn cancel(&mut self) {
        self.active = None;
        self.pending.clear();
    }
    pub fn switch_workspace(&mut self, workspace: WorkspaceGeneration) {
        self.cancel();
        self.workspace = workspace;
        self.files = Arc::new(vec![]);
        self.summary = None;
        self.progress = ScanProgress::default();
        self.error = None;
    }
}
