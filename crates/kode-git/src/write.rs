pub(crate) mod index;
use crate::repository::{check, error, open, relative, string};
use git2::{BranchType, ErrorCode, IndexEntry, IndexTime, ObjectType, Repository, Signature};
use index::IndexTransaction;
use kode_core::{DomainError, JobTag, Result};
use kode_platform::jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub enum WriteRequest {
    Init,
    Stage(Vec<PathBuf>),
    Unstage(Vec<PathBuf>),
    Commit {
        message: String,
    },
    Checkout {
        branch: String,
    },
    Tag {
        name: String,
        revision: String,
        move_existing: bool,
    },
}
#[derive(Debug, Clone)]
pub struct WriteReceipt {
    pub message: String,
    pub commit: Option<String>,
    pub worktree_changed: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct WriteJob {
    pub root: PathBuf,
    pub request: WriteRequest,
}
impl WriteJob {
    pub fn schedule(
        self,
        scheduler: &mut dyn WorkerScheduler<WriteReceipt>,
        owner: OwnerId,
        tag: JobTag,
    ) -> Result<()> {
        scheduler.schedule_blocking(
            owner,
            tag,
            Delivery::Critical,
            Box::new(move |cancel| self.run(&cancel)),
        )
    }
    /// Cancellation can stop preparation. Once publication begins, return its
    /// actual outcome; never report a committed operation as cancelled.
    pub fn run(&self, cancel: &CancellationToken) -> Result<WriteReceipt> {
        self.run_protecting(cancel, &[])
    }
    /// App-owned files can be protected from an in-place branch change while
    /// their stores are open. Ordinary library callers need no protected paths.
    pub fn run_protecting(
        &self,
        cancel: &CancellationToken,
        protected_paths: &[PathBuf],
    ) -> Result<WriteReceipt> {
        check(cancel)?;
        if matches!(self.request, WriteRequest::Init) {
            return init(&self.root, cancel);
        }
        let repo = open(&self.root, cancel)?;
        match &self.request {
            WriteRequest::Init => unreachable!(),
            WriteRequest::Stage(paths) => stage(&repo, paths, cancel),
            WriteRequest::Unstage(paths) => unstage(&repo, paths, cancel),
            WriteRequest::Commit { message } => commit(&repo, message, cancel),
            WriteRequest::Checkout { branch } => checkout(&repo, branch, cancel, protected_paths),
            WriteRequest::Tag {
                name,
                revision,
                move_existing,
            } => tag(&repo, name, revision, *move_existing, cancel),
        }
    }
}
fn failure(operation: &str, message: impl Into<String>) -> DomainError {
    DomainError::Git {
        operation: operation.into(),
        message: message.into(),
    }
}
fn receipt(message: String, commit: Option<String>, warnings: Vec<String>) -> WriteReceipt {
    WriteReceipt {
        message,
        commit,
        worktree_changed: false,
        warnings,
    }
}
fn root(repo: &Repository) -> Result<&Path> {
    repo.workdir()
        .ok_or_else(|| failure("edit worktree", "bare repository has no worktree"))
}
fn paths(repo: &Repository, paths: &[PathBuf], cancel: &CancellationToken) -> Result<Vec<PathBuf>> {
    if paths.is_empty() || paths.len() > 100_000 {
        return Err(failure(
            "validate paths",
            "select between 1 and 100,000 files",
        ));
    }
    let root = root(repo)?;
    let mut selected = vec![];
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        check(cancel)?;
        let path = relative(path)?;
        if let Some(parent) = path.parent() {
            let mut at = root.to_owned();
            for part in parent.components() {
                at.push(part);
                match fs::symlink_metadata(&at) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(failure(
                            "validate paths",
                            format!("cannot stage through directory link: {}", path.display()),
                        ));
                    }
                    Ok(_) => {}
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => break,
                    Err(source) => {
                        return Err(DomainError::Io {
                            operation: "validate Git path parent",
                            path: at,
                            source,
                        });
                    }
                }
            }
        }
        if seen.insert(path.clone()) {
            selected.push(path);
        }
    }
    Ok(selected)
}
fn init(root: &Path, cancel: &CancellationToken) -> Result<WriteReceipt> {
    if !root.is_dir() {
        return Err(failure(
            "initialize repository",
            "workspace must already be an existing directory",
        ));
    }
    if crate::repository::discover(root, cancel)?.is_some() {
        return Err(failure(
            "initialize repository",
            "a repository already exists at or above this workspace",
        ));
    }
    if fs::symlink_metadata(root.join(".git")).is_ok() {
        return Err(failure(
            "initialize repository",
            "existing Git metadata must be inspected before initialization",
        ));
    }
    let mut options = git2::RepositoryInitOptions::new();
    options
        .external_template(false)
        .no_reinit(true)
        .mkdir(false);
    check(cancel)?;
    let repo = Repository::init_opts(root, &options)
        .map_err(|error_| error(root, "initialize repository", error_))?;
    Ok(receipt(
        format!("Initialized {}", repo.path().display()),
        None,
        vec![],
    ))
}
fn stage(
    repo: &Repository,
    selected: &[PathBuf],
    cancel: &CancellationToken,
) -> Result<WriteReceipt> {
    let selected = paths(repo, selected, cancel)?;
    let mut transaction = IndexTransaction::new(repo)?;
    for path in &selected {
        check(cancel)?;
        let absolute = root(repo)?.join(path);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) => {
                if metadata.is_dir() && !absolute.join(".git").exists() {
                    return Err(failure(
                        "stage files",
                        "select individual files rather than a directory",
                    ));
                }
                if transaction.index.get_path(path, 0).is_none()
                    && repo
                        .is_path_ignored(path)
                        .map_err(|failure| error(repo.path(), "check ignored path", failure))?
                {
                    return Err(failure(
                        "stage files",
                        format!("ignored file: {}", path.display()),
                    ));
                }
                transaction
                    .index
                    .add_path(path)
                    .map_err(|failure| error(repo.path(), "stage file", failure))?;
            }
            Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {
                if transaction.index.conflict_get(path).is_ok() {
                    transaction
                        .index
                        .conflict_remove(path)
                        .map_err(|failure| error(repo.path(), "stage deleted conflict", failure))?;
                } else if transaction.index.get_path(path, 0).is_some() {
                    transaction
                        .index
                        .remove_path(path)
                        .map_err(|failure| error(repo.path(), "stage deleted file", failure))?;
                } else {
                    return Err(failure(
                        "stage deleted file",
                        format!("path is neither tracked nor present: {}", path.display()),
                    ));
                }
            }
            Err(source) => {
                return Err(DomainError::Io {
                    operation: "stage Git file",
                    path: absolute,
                    source,
                });
            }
        }
    }
    check(cancel)?;
    let warnings = transaction.publish(repo)?;
    Ok(receipt(
        format!("Staged {} file(s)", selected.len()),
        None,
        warnings,
    ))
}
fn unstage(
    repo: &Repository,
    selected: &[PathBuf],
    cancel: &CancellationToken,
) -> Result<WriteReceipt> {
    let selected = paths(repo, selected, cancel)?;
    let mut transaction = IndexTransaction::new(repo)?;
    let head = match repo.head() {
        Ok(head) => Some(
            head.peel_to_tree()
                .map_err(|failure| error(repo.path(), "read unstage HEAD tree", failure))?,
        ),
        Err(failure) if failure.code() == ErrorCode::UnbornBranch => None,
        Err(failure) => return Err(error(repo.path(), "read unstage HEAD", failure)),
    };
    for path in &selected {
        check(cancel)?;
        let entry = match head.as_ref().map(|tree| tree.get_path(path)) {
            Some(Ok(entry)) => Some(entry),
            Some(Err(failure)) if failure.code() == ErrorCode::NotFound => None,
            Some(Err(failure)) => {
                return Err(error(repo.path(), "read unstage tree entry", failure));
            }
            None => None,
        };
        if entry
            .as_ref()
            .is_some_and(|entry| entry.kind() == Some(ObjectType::Tree))
        {
            return Err(failure(
                "unstage files",
                "select individual files rather than a directory",
            ));
        }
        if transaction.index.conflict_get(path).is_ok() {
            transaction
                .index
                .conflict_remove(path)
                .map_err(|failure| error(repo.path(), "unstage conflict", failure))?;
        }
        if transaction.index.get_path(path, 0).is_some() {
            transaction
                .index
                .remove_path(path)
                .map_err(|failure| error(repo.path(), "remove index entry", failure))?;
        }
        if let Some(entry) = entry {
            let bytes = path.as_os_str().as_encoded_bytes().to_vec();
            #[cfg(windows)]
            let bytes = bytes
                .into_iter()
                .map(|byte| if byte == b'\\' { b'/' } else { byte })
                .collect();
            let item = IndexEntry {
                ctime: IndexTime::new(0, 0),
                mtime: IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: entry.filemode() as u32,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: entry.id(),
                flags: 0,
                flags_extended: 0,
                path: bytes,
            };
            transaction
                .index
                .add(&item)
                .map_err(|failure| error(repo.path(), "restore HEAD index entry", failure))?;
        }
    }
    check(cancel)?;
    let warnings = transaction.publish(repo)?;
    Ok(receipt(
        format!("Unstaged {} file(s)", selected.len()),
        None,
        warnings,
    ))
}
fn signature(repo: &Repository) -> Result<Signature<'static>> {
    let signature = repo
        .signature()
        .map_err(|failure| error(repo.path(), "read configured Git identity", failure))?;
    if signature.name_bytes().iter().all(u8::is_ascii_whitespace)
        || signature.email_bytes().iter().all(u8::is_ascii_whitespace)
    {
        return Err(failure(
            "read configured Git identity",
            "configure non-empty user.name and user.email",
        ));
    }
    Ok(signature)
}
fn commit(repo: &Repository, message: &str, cancel: &CancellationToken) -> Result<WriteReceipt> {
    root(repo)?;
    if message.trim().is_empty() || message.contains('\0') || message.len() > 128 * 1024 {
        return Err(failure(
            "validate commit message",
            "enter a non-empty message without NUL, at most 128 KiB",
        ));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(failure(
            "commit",
            "finish or abort the existing repository operation first",
        ));
    }
    let signature = signature(repo)?;
    let mut transaction = IndexTransaction::new(repo)?;
    if transaction.index.has_conflicts() {
        return Err(failure(
            "commit",
            "resolve index conflicts before committing",
        ));
    }
    let parent = match repo.head() {
        Ok(head) => Some(
            head.peel_to_commit()
                .map_err(|failure| error(repo.path(), "read commit parent", failure))?,
        ),
        Err(failure) if failure.code() == ErrorCode::UnbornBranch => None,
        Err(failure) => return Err(error(repo.path(), "read commit HEAD", failure)),
    };
    check(cancel)?;
    if transaction.index.is_empty() && parent.is_none() {
        return Err(failure("commit", "nothing staged to commit"));
    }
    let tree = transaction
        .index
        .write_tree()
        .map_err(|failure| error(repo.path(), "prepare commit tree", failure))?;
    if parent
        .as_ref()
        .is_some_and(|parent| parent.tree_id() == tree)
    {
        return Err(failure("commit", "nothing staged to commit"));
    }
    let tree = repo
        .find_tree(tree)
        .map_err(|failure| error(repo.path(), "read prepared commit tree", failure))?;
    let parents: Vec<_> = parent.iter().collect();
    check(cancel)?;
    let oid = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(|failure| error(repo.path(), "create commit", failure))?;
    Ok(receipt(
        format!("Committed {oid}"),
        Some(oid.to_string()),
        vec![],
    ))
}
fn tag(
    repo: &Repository,
    name: &str,
    revision: &str,
    move_existing: bool,
    cancel: &CancellationToken,
) -> Result<WriteReceipt> {
    let reference = format!("refs/tags/{name}");
    if name.is_empty()
        || name.len() > 1024
        || name.contains('\0')
        || !git2::Reference::is_valid_name(&reference)
    {
        return Err(failure("validate tag name", "enter a valid Git tag name"));
    }
    let object = repo
        .revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map_err(|failure| error(repo.path(), "resolve tag commit", failure))?;
    let signature = signature(repo)?;
    check(cancel)?;
    repo.tag(name, object.as_object(), &signature, name, move_existing)
        .map_err(|failure| error(repo.path(), "create or move tag", failure))?;
    Ok(receipt(
        format!(
            "{} tag {name}",
            if move_existing {
                "Created/moved"
            } else {
                "Created"
            }
        ),
        Some(object.id().to_string()),
        vec![],
    ))
}
fn checkout(
    repo: &Repository,
    branch: &str,
    cancel: &CancellationToken,
    protected_paths: &[PathBuf],
) -> Result<WriteReceipt> {
    root(repo)?;
    if branch.contains('\0') || !git2::Reference::is_valid_name(&format!("refs/heads/{branch}")) {
        return Err(failure(
            "validate branch name",
            "select a valid local branch",
        ));
    }
    let branch_ref = repo
        .find_branch(branch, BranchType::Local)
        .map_err(|failure| error(repo.path(), "find checkout branch", failure))?;
    let reference = string(branch_ref.get().name_bytes(), "checkout branch reference")?;
    if branch_ref.is_head() {
        return Ok(receipt(format!("Already on {branch}"), None, vec![]));
    }
    if repo.state() != git2::RepositoryState::Clean {
        return Err(failure(
            "checkout branch",
            "finish or abort the existing repository operation first",
        ));
    }
    check_branch_worktrees(repo, &reference)?;
    let mut transaction = IndexTransaction::new(repo)?;
    let status = crate::status::read(repo, &crate::model::ReadLimits::default(), cancel)?;
    if status
        .entries
        .iter()
        .any(|entry| entry.kind != crate::model::ChangeKind::Untracked)
        || status.truncated
    {
        return Err(failure(
            "checkout branch",
            "save, commit or otherwise preserve local changes before checkout",
        ));
    }
    let mut refs = repo
        .transaction()
        .map_err(|failure| error(repo.path(), "begin checkout reference transaction", failure))?;
    refs.lock_ref("HEAD")
        .map_err(|failure| error(repo.path(), "lock checkout HEAD", failure))?;
    refs.lock_ref(&reference)
        .map_err(|failure| error(repo.path(), "lock checkout branch", failure))?;
    let current = repo
        .find_reference("HEAD")
        .map_err(|failure| error(repo.path(), "read checkout HEAD", failure))?;
    if let Some(previous) = current
        .symbolic_target()
        .map_err(|failure| error(repo.path(), "resolve previous checkout branch", failure))?
        && previous != reference
    {
        refs.lock_ref(previous)
            .map_err(|failure| error(repo.path(), "lock previous branch", failure))?;
    }
    let target = repo
        .find_reference(&reference)
        .and_then(|reference| reference.peel_to_commit())
        .map_err(|failure| error(repo.path(), "resolve locked checkout commit", failure))?;
    let old_tree = match repo.head() {
        Ok(head) => Some(
            head.peel_to_tree()
                .map_err(|failure| error(repo.path(), "read protected HEAD tree", failure))?,
        ),
        Err(failure) if failure.code() == ErrorCode::UnbornBranch => None,
        Err(failure) => return Err(error(repo.path(), "read protected HEAD", failure)),
    };
    let target_tree = target
        .tree()
        .map_err(|failure| error(repo.path(), "read protected checkout tree", failure))?;
    for path in protected_paths {
        let path = relative(path)?;
        let old = tree_entry(repo, old_tree.as_ref(), &path)?;
        let new = tree_entry(repo, Some(&target_tree), &path)?;
        if old != new {
            return Err(failure(
                "checkout branch",
                format!(
                    "checkout changes active workspace metadata {}; close this workspace before checking out that branch",
                    path.display()
                ),
            ));
        }
    }
    let signature = signature(repo)?;
    refs.set_symbolic_target(
        "HEAD",
        &reference,
        Some(&signature),
        &format!("checkout: moving to {branch}"),
    )
    .map_err(|failure| error(repo.path(), "prepare checkout HEAD", failure))?;
    check(cancel)?;
    let mut options = git2::build::CheckoutBuilder::new();
    options.safe().overwrite_ignored(false);
    repo.checkout_tree(target.as_object(), Some(&mut options))
        .map_err(|failure| {
            error(
                repo.path(),
                "checkout files (on I/O failure inspect worktree before retry)",
                failure,
            )
        })?;
    let warnings = transaction.publish(repo)?;
    refs.commit().map_err(|failure| {
        error(
            repo.path(),
            "update checkout HEAD (files/index changed; inspect before retry)",
            failure,
        )
    })?;
    let mut result = receipt(
        format!("Checked out {branch}"),
        Some(target.id().to_string()),
        warnings,
    );
    result.worktree_changed = true;
    Ok(result)
}
fn tree_entry(
    repo: &Repository,
    tree: Option<&git2::Tree<'_>>,
    path: &Path,
) -> Result<Option<(git2::Oid, i32)>> {
    match tree.map(|tree| tree.get_path(path)) {
        Some(Ok(entry)) => Ok(Some((entry.id(), entry.filemode()))),
        None => Ok(None),
        Some(Err(failure)) if failure.code() == ErrorCode::NotFound => Ok(None),
        Some(Err(failure)) => Err(error(repo.path(), "read protected path", failure)),
    }
}
fn check_branch_worktrees(repo: &Repository, reference: &str) -> Result<()> {
    let common = Repository::open(repo.commondir())
        .map_err(|failure| error(repo.path(), "open common checkout repository", failure))?;
    let names = common
        .worktrees()
        .map_err(|failure| error(repo.path(), "list checkout worktrees", failure))?;
    let mut others = vec![];
    for name in names.iter() {
        let name = name
            .map_err(|failure| error(repo.path(), "decode worktree name", failure))?
            .ok_or_else(|| DomainError::Unsupported("non-UTF-8 worktree name".into()))?;
        let worktree = common
            .find_worktree(name)
            .map_err(|failure| error(repo.path(), "read checkout worktree", failure))?;
        others.push(
            Repository::open_from_worktree(&worktree)
                .map_err(|failure| error(repo.path(), "open checkout worktree", failure))?,
        );
    }
    others.push(common);
    for other in others {
        if other.workdir() == repo.workdir() {
            continue;
        }
        match other.head() {
            Ok(head) if head.name_bytes() == reference.as_bytes() => {
                return Err(failure(
                    "checkout branch",
                    format!(
                        "branch is already checked out at {}",
                        other.workdir().unwrap_or(other.path()).display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(failure)
                if matches!(
                    failure.code(),
                    ErrorCode::UnbornBranch | ErrorCode::NotFound
                ) => {}
            Err(failure) => return Err(error(other.path(), "read other worktree HEAD", failure)),
        }
    }
    Ok(())
}
