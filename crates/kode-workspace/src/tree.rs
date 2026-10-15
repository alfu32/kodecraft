//! Filesystem work is prepared here; the tree reducer and flattening never do IO.
use crate::{
    exclusions::{FolderStatus, IgnoreRules, folder_status},
    project::{Project, lexical_path},
};
use kode_core::{DomainError, JobTag, RequestId, Result, WorkspaceGeneration};
use kode_platform::jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    pub bytes: u64,
    pub child_count: Option<usize>,
    pub status: Option<FolderStatus>,
}
#[derive(Debug, Clone)]
pub struct FlatEntry {
    pub entry: TreeEntry,
    pub depth: usize,
    pub open: bool,
}
pub struct TreeJob {
    pub tag: JobTag,
    pub project: Project,
    pub expanded: BTreeSet<PathBuf>,
}
pub struct TreeSnapshot {
    pub listings: BTreeMap<PathBuf, Result<Vec<TreeEntry>>>,
}
pub struct FileTree {
    pub root: PathBuf,
    workspace: WorkspaceGeneration,
    expanded: BTreeSet<PathBuf>,
    listings: BTreeMap<PathBuf, Vec<TreeEntry>>,
    pending: Option<JobTag>,
    pub errors: Vec<String>,
}
impl FileTree {
    /// The supplied project root has already been validated by the worker.
    pub fn new(project: &Project) -> Self {
        Self {
            root: project.root.clone(),
            workspace: project.generation,
            expanded: BTreeSet::from([project.root.clone()]),
            listings: BTreeMap::new(),
            pending: None,
            errors: vec![],
        }
    }
    pub fn begin_refresh(&mut self, project: &Project) -> Result<TreeJob> {
        if project.root != self.root || project.generation != self.workspace {
            return Err(DomainError::Unsupported(
                "tree belongs to another workspace".into(),
            ));
        }
        let tag = JobTag {
            workspace: self.workspace,
            request: RequestId::fresh()?,
            document: None,
        };
        self.pending = Some(tag);
        Ok(TreeJob {
            tag,
            project: project.clone(),
            expanded: self.expanded.clone(),
        })
    }
    pub fn publish(&mut self, tag: JobTag, result: Result<TreeSnapshot>) -> bool {
        if self.pending != Some(tag) {
            return false;
        }
        self.pending = None;
        self.errors.clear();
        match result {
            Err(error) => self.errors.push(error.to_string()),
            Ok(snapshot) => {
                for (directory, result) in snapshot.listings {
                    match result {
                        Ok(entries) => {
                            self.listings.insert(directory, entries);
                        }
                        Err(error) => self.errors.push(error.to_string()),
                    }
                }
                // Deleted and replaced nodes cannot leave unreachable cached subtrees.
                let mut reachable = BTreeSet::from([self.root.clone()]);
                let mut stack = vec![self.root.clone()];
                while let Some(directory) = stack.pop() {
                    if let Some(entries) = self.listings.get(&directory) {
                        for entry in entries
                            .iter()
                            .filter(|entry| entry.kind == EntryKind::Directory)
                        {
                            if reachable.insert(entry.path.clone()) {
                                stack.push(entry.path.clone());
                            }
                        }
                    }
                }
                self.listings.retain(|path, _| reachable.contains(path));
                self.expanded.retain(|path| reachable.contains(path));
            }
        }
        true
    }
    pub fn toggle(&mut self, path: &Path) -> bool {
        if path == self.root {
            return false;
        }
        if !self
            .listings
            .values()
            .flatten()
            .any(|entry| entry.path == path && entry.kind == EntryKind::Directory)
        {
            return false;
        }
        if !self.expanded.remove(path) {
            self.expanded.insert(path.into());
        }
        true
    }
    pub fn is_open(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }
    /// Expands ancestors without inventing filesystem entries. The next worker
    /// refresh discovers the actual path (and rejects symlink directory traversal).
    pub fn open_path(&mut self, path: &Path) -> Result<()> {
        let path = lexical_path(&self.root, path)?;
        let mut parent = path.parent();
        while let Some(path) = parent.filter(|path| path.starts_with(&self.root)) {
            self.expanded.insert(path.into());
            if path == self.root {
                break;
            }
            parent = path.parent();
        }
        Ok(())
    }
    pub fn flattened(&self) -> Vec<FlatEntry> {
        let mut entries = vec![];
        let mut stack: Vec<_> = self
            .listings
            .get(&self.root)
            .into_iter()
            .flatten()
            .rev()
            .map(|entry| (entry, 0))
            .collect();
        while let Some((entry, depth)) = stack.pop() {
            let open = self.expanded.contains(&entry.path);
            entries.push(FlatEntry {
                entry: entry.clone(),
                depth,
                open,
            });
            if entry.kind == EntryKind::Directory
                && open
                && let Some(children) = self.listings.get(&entry.path)
            {
                stack.extend(children.iter().rev().map(|entry| (entry, depth + 1)));
            }
        }
        entries
    }
    pub fn contains_path(&self, path: &Path) -> bool {
        self.listings
            .get(path.parent().unwrap_or(&self.root))
            .is_some_and(|entries| entries.iter().any(|entry| entry.path == path))
    }
    pub fn reconcile_move(&mut self, from: &Path, to: &Path) {
        self.pending = None;
        self.expanded = self
            .expanded
            .iter()
            .map(|path| moved_path(path, from, to).unwrap_or_else(|| path.clone()))
            .collect();
        let listings = std::mem::take(&mut self.listings);
        for (path, mut entries) in listings {
            for entry in &mut entries {
                if let Some(path) = moved_path(&entry.path, from, to) {
                    entry.name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    entry.path = path;
                }
            }
            sort_entries(&mut entries);
            self.listings
                .insert(moved_path(&path, from, to).unwrap_or(path), entries);
        }
    }
}
pub fn moved_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    path.strip_prefix(from).ok().map(|suffix| to.join(suffix))
}
pub fn tree_metadata(name: &str) -> bool {
    matches!(name, ".kode" | ".kode.json" | ".kode-rs" | ".kode-rs.json")
        || name.starts_with(".kode.json.")
        || name.starts_with(".kode-rs.json.")
}
fn sort_entries(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Directory, &a.path).cmp(&(b.kind != EntryKind::Directory, &b.path))
    });
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn cancelled(token: &CancellationToken) -> Result<()> {
    if token.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
/// Browser policy: visible Git-ignored, hidden, build and outside-source entries;
/// hide only session metadata, never follow directory symlinks or leave the root.
pub fn read_tree(job: &TreeJob, token: &CancellationToken) -> Result<TreeSnapshot> {
    let rules = IgnoreRules::new(&job.project);
    rules.validate()?;
    let mut snapshot = TreeSnapshot {
        listings: BTreeMap::new(),
    };
    for path in &job.expanded {
        cancelled(token)?;
        let result = read_directory(&job.project, path, &rules, token);
        if matches!(&result, Err(DomainError::Cancelled)) {
            return Err(DomainError::Cancelled);
        }
        snapshot.listings.insert(path.clone(), result);
    }
    Ok(snapshot)
}
fn read_directory(
    project: &Project,
    directory: &Path,
    rules: &IgnoreRules,
    token: &CancellationToken,
) -> Result<Vec<TreeEntry>> {
    let directory = lexical_path(&project.root, directory)?;
    // Check each component, including ancestors supplied by open_path.
    let mut current = project.root.clone();
    for part in directory
        .strip_prefix(&project.root)
        .expect("inside root")
        .components()
    {
        current.push(part);
        if fs::symlink_metadata(&current)
            .map_err(|error| io("stat tree folder", &current, error))?
            .file_type()
            .is_symlink()
        {
            return Err(DomainError::Unsupported(
                "tree does not traverse directory symlinks".into(),
            ));
        }
    }
    let mut entries = vec![];
    for child in
        fs::read_dir(&directory).map_err(|error| io("list tree folder", &directory, error))?
    {
        cancelled(token)?;
        let child = child.map_err(|error| io("list tree entry", &directory, error))?;
        let name = child.file_name().to_string_lossy().into_owned();
        if tree_metadata(&name) {
            continue;
        }
        let path = child.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| io("stat tree entry", &path, error))?;
        let kind = if metadata.file_type().is_symlink() {
            EntryKind::Symlink
        } else if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let child_count = if kind == EntryKind::Directory {
            // Counts are metadata only, not eager recursive expansion. Failure is
            // represented as unknown instead of incorrectly claiming zero.
            fs::read_dir(&path).ok().and_then(|children| {
                let mut count = 0;
                for child in children {
                    if token.is_cancelled() {
                        return None;
                    }
                    let child = child.ok()?;
                    if !tree_metadata(&child.file_name().to_string_lossy()) {
                        count += 1;
                    }
                }
                Some(count)
            })
        } else {
            None
        };
        cancelled(token)?;
        let status = if kind == EntryKind::Directory {
            folder_status(project, &path, rules)?
        } else {
            None
        };
        entries.push(TreeEntry {
            path,
            name,
            kind,
            bytes: metadata.len(),
            child_count,
            status,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}
pub fn schedule_tree(
    scheduler: &mut dyn WorkerScheduler<TreeSnapshot>,
    owner: OwnerId,
    job: TreeJob,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        job.tag,
        Delivery::Cancellable,
        Box::new(move |token| read_tree(&job, &token)),
    )
}

#[derive(Debug, Clone)]
pub enum FileAction {
    CreateFile { parent: PathBuf, name: String },
    CreateDirectory { parent: PathBuf, name: String },
    Rename { path: PathBuf, name: String },
    Delete { path: PathBuf },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    Created(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Deleted(PathBuf),
}
fn filename(name: &str) -> Result<&std::ffi::OsStr> {
    let path = Path::new(name);
    if name.is_empty()
        || name.chars().any(|ch| ch == '\0' || ch == '/' || ch == '\\')
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
        || tree_metadata(name)
    {
        return Err(DomainError::Unsupported(
            "enter one filename; project metadata is reserved".into(),
        ));
    }
    Ok(path.as_os_str())
}
fn action_path(project: &Project, path: &Path) -> Result<PathBuf> {
    let path = lexical_path(&project.root, path)?;
    if path == project.root
        || path
            .strip_prefix(&project.root)
            .expect("inside root")
            .components()
            .any(|part| tree_metadata(&part.as_os_str().to_string_lossy()))
    {
        return Err(DomainError::Unsupported(
            "project root and metadata cannot be modified here".into(),
        ));
    }
    // Resolve the parent, leaving the final symlink itself as the action target.
    let parent = project.resolve_existing(path.parent().expect("non-root entry"))?;
    Ok(parent.join(path.file_name().expect("non-root entry")))
}
/// Run only in a worker. Failed operations emit no path reconciliation event.
pub fn perform_action(
    project: &Project,
    action: &FileAction,
    token: &CancellationToken,
) -> Result<FileChange> {
    cancelled(token)?;
    match action {
        FileAction::CreateFile { parent, name } | FileAction::CreateDirectory { parent, name } => {
            let parent = project.resolve_existing(parent)?;
            let path = action_path(project, &parent.join(filename(name)?))?;
            if matches!(action, FileAction::CreateFile { .. }) {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| io("create file", &path, error))?;
            } else {
                fs::create_dir(&path).map_err(|error| io("create folder", &path, error))?;
            }
            Ok(FileChange::Created(path))
        }
        FileAction::Rename { path, name } => {
            let from = action_path(project, path)?;
            let to = from.parent().expect("entry parent").join(filename(name)?);
            fs::symlink_metadata(&from).map_err(|error| io("stat rename source", &from, error))?;
            if from != to {
                rename_noreplace(&from, &to)?;
            }
            Ok(FileChange::Renamed { from, to })
        }
        FileAction::Delete { path } => {
            let path = action_path(project, path)?;
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| io("stat delete target", &path, error))?;
            let result = if metadata.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            result.map_err(|error| io("delete tree entry", &path, error))?;
            Ok(FileChange::Deleted(path))
        }
    }
}
fn rename_noreplace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use nix::fcntl::{RenameFlags, renameat2};
        let directory = fs::File::open(from.parent().expect("entry parent"))
            .map_err(|error| io("open rename folder", from, error))?;
        renameat2(
            &directory,
            from.file_name().expect("entry name"),
            &directory,
            to.file_name().expect("entry name"),
            RenameFlags::RENAME_NOREPLACE,
        )
        .map_err(|error| {
            io(
                "rename tree entry",
                from,
                std::io::Error::from_raw_os_error(error as i32),
            )
        })?;
        Ok(())
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        let _ = (from, to);
        Err(DomainError::Unsupported(
            "atomic rename without replacement is not qualified on this platform".into(),
        ))
    }
}
pub fn schedule_action(
    scheduler: &mut dyn WorkerScheduler<FileChange>,
    owner: OwnerId,
    tag: JobTag,
    project: Project,
    action: FileAction,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        Delivery::Critical,
        Box::new(move |token| perform_action(&project, &action, &token)),
    )
}

pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    for unit in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.;
        if value < 1024. || unit == "EiB" {
            return format!("{value:.1} {unit}");
        }
    }
    unreachable!()
}
