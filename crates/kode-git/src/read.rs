mod content;
use crate::{
    model::*,
    repository::{check, error, location, open, path, string},
    status,
};
use git2::{BranchType, DiffFindOptions, DiffOptions, ErrorCode, ObjectType, Repository, Sort};
use kode_core::{DomainError, JobTag, Result};
use kode_platform::jobs::{CancellationToken, Delivery, OwnerId, WorkerScheduler};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum ReadRequest {
    Discover,
    Head,
    Status,
    Branches,
    Tags,
    Commits { limit: Option<usize> },
    Files { revision: String },
    Content { revision: String, path: PathBuf },
    Diff { path: PathBuf, staged: bool },
    IgnoredPatterns,
    Snapshot { history_limit: Option<usize> },
}
#[derive(Debug)]
pub enum ReadOutput {
    Location(Option<RepositoryLocation>),
    Head(Head),
    Status(Listing<StatusEntry>),
    Branches(Listing<Branch>),
    Tags(Listing<Tag>),
    Commits(Listing<Commit>),
    Files(Listing<PathBuf>),
    Content(Content),
    Diff(DiffContents),
    IgnoredPatterns(Vec<String>),
    Snapshot(Box<RepositorySnapshot>),
}
#[derive(Debug, Clone)]
pub struct ReadJob {
    pub root: PathBuf,
    pub request: ReadRequest,
    pub limits: ReadLimits,
}
impl ReadJob {
    pub fn new(root: PathBuf, request: ReadRequest) -> Self {
        Self {
            root,
            request,
            limits: ReadLimits::default(),
        }
    }
    pub fn schedule(
        self,
        scheduler: &mut dyn WorkerScheduler<ReadOutput>,
        owner: OwnerId,
        tag: JobTag,
    ) -> Result<()> {
        scheduler.schedule_blocking(
            owner,
            tag,
            Delivery::Cancellable,
            Box::new(move |cancel| self.run(&cancel)),
        )
    }
    /// Run inside an F03 blocking job. No repository/index/tree handles escape.
    pub fn run(&self, cancel: &CancellationToken) -> Result<ReadOutput> {
        if matches!(self.request, ReadRequest::Discover) {
            return crate::repository::discover(&self.root, cancel).map(ReadOutput::Location);
        }
        let repo = open(&self.root, cancel)?;
        let limits = self.limits.bounded();
        let output = match &self.request {
            ReadRequest::Discover => unreachable!(),
            ReadRequest::Head => ReadOutput::Head(head(&repo)?),
            ReadRequest::Status => ReadOutput::Status(status::read(&repo, &limits, cancel)?),
            ReadRequest::Branches => ReadOutput::Branches(branches(&repo, &limits, cancel)?),
            ReadRequest::Tags => ReadOutput::Tags(tags(&repo, &limits, cancel)?),
            ReadRequest::Commits { limit } => {
                ReadOutput::Commits(commits(&repo, *limit, &limits, cancel)?)
            }
            ReadRequest::Files { revision } => {
                ReadOutput::Files(files(&repo, revision, &limits, cancel)?)
            }
            ReadRequest::Content { revision, path } => {
                ReadOutput::Content(content::at_commit(&repo, revision, path, &limits, cancel)?)
            }
            ReadRequest::Diff { path, staged } => {
                ReadOutput::Diff(content::diff(&repo, path, *staged, &limits, cancel)?)
            }
            ReadRequest::IgnoredPatterns => {
                ReadOutput::IgnoredPatterns(ignored_patterns(&repo, cancel)?)
            }
            ReadRequest::Snapshot { history_limit } => {
                ReadOutput::Snapshot(Box::new(snapshot(&repo, *history_limit, &limits, cancel)?))
            }
        };
        check(cancel)?;
        Ok(output)
    }
}
fn head(repo: &Repository) -> Result<Head> {
    let reference = match repo.head() {
        Ok(reference) => reference,
        Err(failure) if failure.code() == ErrorCode::UnbornBranch => {
            let reference = repo
                .find_reference("HEAD")
                .map_err(|failure| error(repo.path(), "read unborn HEAD", failure))?;
            let target = reference
                .symbolic_target_bytes()
                .ok_or_else(|| DomainError::Git {
                    operation: "read unborn HEAD".into(),
                    message: "HEAD has no symbolic branch".into(),
                })?;
            let reference = string(target, "HEAD reference")?;
            let name = reference
                .strip_prefix("refs/heads/")
                .unwrap_or(&reference)
                .into();
            return Ok(Head::Unborn { reference, name });
        }
        Err(failure) => return Err(error(repo.path(), "read HEAD", failure)),
    };
    let commit = reference
        .peel_to_commit()
        .map_err(|failure| error(repo.path(), "resolve HEAD commit", failure))?
        .id()
        .to_string();
    if reference.is_branch() {
        let reference = string(reference.name_bytes(), "branch reference")?;
        let name = reference
            .strip_prefix("refs/heads/")
            .unwrap_or(&reference)
            .into();
        Ok(Head::Branch {
            reference,
            name,
            commit,
        })
    } else {
        Ok(Head::Detached { commit })
    }
}
fn branches(
    repo: &Repository,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Listing<Branch>> {
    let iterator = repo
        .branches(Some(BranchType::Local))
        .map_err(|failure| error(repo.path(), "list branches", failure))?;
    let mut entries = vec![];
    let mut truncated = false;
    for item in iterator {
        check(cancel)?;
        if entries.len() == limits.entries {
            truncated = true;
            break;
        }
        let (branch, _) = item.map_err(|failure| error(repo.path(), "read branch", failure))?;
        let name = branch
            .name_bytes()
            .map_err(|failure| error(repo.path(), "read branch name", failure))?;
        entries.push(Branch {
            name: string(name, "branch name")?,
            reference: string(branch.get().name_bytes(), "branch reference")?,
            commit: branch.get().target().map(|oid| oid.to_string()),
            current: branch.is_head(),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Listing { entries, truncated })
}
fn tags(
    repo: &Repository,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Listing<Tag>> {
    let references = repo
        .references_glob("refs/tags/*")
        .map_err(|failure| error(repo.path(), "list tags", failure))?;
    let mut entries = vec![];
    let mut truncated = false;
    for reference in references {
        check(cancel)?;
        if entries.len() == limits.entries {
            truncated = true;
            break;
        }
        let reference =
            reference.map_err(|failure| error(repo.path(), "read tag reference", failure))?;
        let name = string(reference.name_bytes(), "tag name")?
            .strip_prefix("refs/tags/")
            .unwrap_or("")
            .to_owned();
        let resolved = reference
            .resolve()
            .map_err(|failure| error(repo.path(), "resolve tag reference", failure))?;
        let oid = resolved.target().ok_or_else(|| DomainError::Git {
            operation: "read tag target".into(),
            message: format!("tag {name} has no object"),
        })?;
        let object = repo
            .find_object(oid, None)
            .map_err(|failure| error(repo.path(), "read tag object", failure))?;
        let annotated = object.kind() == Some(ObjectType::Tag);
        let peeled = object
            .peel(ObjectType::Any)
            .map_err(|failure| error(repo.path(), "peel tag", failure))?;
        let commit = (peeled.kind() == Some(ObjectType::Commit)).then(|| peeled.id().to_string());
        entries.push(Tag {
            name,
            object: oid.to_string(),
            commit,
            annotated,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(Listing { entries, truncated })
}
/// Match Kotlin's digit extraction, 10-bit slots, signed Long and masked shifts.
/// This is the source's numeric score, not a SemVer ordering implementation.
pub fn score_semantic_tag(tag: &str) -> Option<i64> {
    let numbers: Vec<_> = tag
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<i64>().ok())
        .collect();
    if numbers.is_empty() {
        return None;
    }
    let mut score = 0i64;
    for (index, value) in numbers.iter().enumerate() {
        if *value >= 1024 {
            return None;
        }
        score |= value.wrapping_shl(((numbers.len() - 1 - index) * 10) as u32);
    }
    Some(score)
}
fn preferred_tag<'a>(tags: &'a [Tag], hash: &str) -> Option<&'a str> {
    let mut best: Option<(i64, &str)> = None;
    for tag in tags
        .iter()
        .filter(|tag| tag.annotated && tag.commit.as_deref() == Some(hash))
    {
        let display = tag.name.rsplit('/').next().unwrap_or(&tag.name);
        let score = score_semantic_tag(display).unwrap_or(0);
        if best.is_none_or(|(previous, _)| score > previous) {
            best = Some((score, display));
        }
    }
    best.map(|(_, tag)| tag)
}
fn commits(
    repo: &Repository,
    limit: Option<usize>,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Listing<Commit>> {
    if matches!(head(repo)?, Head::Unborn { .. }) {
        return Ok(Listing {
            entries: vec![],
            truncated: false,
        });
    }
    let tags = tags(repo, limits, cancel)?;
    if tags.truncated {
        return Err(DomainError::Git {
            operation: "list commits".into(),
            message: "tag limit exceeded; cannot choose authoritative commit tags".into(),
        });
    }
    let mut walk = repo
        .revwalk()
        .map_err(|failure| error(repo.path(), "create history walk", failure))?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|failure| error(repo.path(), "sort history", failure))?;
    walk.push_head()
        .map_err(|failure| error(repo.path(), "walk HEAD", failure))?;
    let mut entries = vec![];
    let mut truncated = false;
    let limit = limit.unwrap_or(limits.commits).min(limits.commits);
    for oid in walk {
        check(cancel)?;
        if entries.len() == limit {
            truncated = true;
            break;
        }
        let oid = oid.map_err(|failure| error(repo.path(), "walk history", failure))?;
        let commit = repo
            .find_commit(oid)
            .map_err(|failure| error(repo.path(), "read history commit", failure))?;
        let author = commit.author();
        let hash = oid.to_string();
        let tag = preferred_tag(&tags.entries, &hash).map(str::to_owned);
        entries.push(Commit {
            hash,
            author: string(author.name_bytes(), "author name")?,
            timestamp: author.when().seconds(),
            offset_minutes: author.when().offset_minutes(),
            message: string(commit.message_bytes(), "commit message")?
                .trim()
                .into(),
            parents: commit.parent_ids().map(|oid| oid.to_string()).collect(),
            tag,
        });
    }
    Ok(Listing { entries, truncated })
}
fn files(
    repo: &Repository,
    revision: &str,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Listing<PathBuf>> {
    check(cancel)?;
    let commit = repo
        .revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map_err(|failure| error(repo.path(), "resolve commit files", failure))?;
    let new = commit
        .tree()
        .map_err(|failure| error(repo.path(), "read commit tree", failure))?;
    let old = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .and_then(|parent| parent.tree())
                .map_err(|failure| error(repo.path(), "read parent tree", failure))?,
        )
    } else {
        None
    };
    let mut options = DiffOptions::new();
    options.skip_binary_check(true).include_typechange(true);
    let mut diff = repo
        .diff_tree_to_tree(old.as_ref(), Some(&new), Some(&mut options))
        .map_err(|failure| error(repo.path(), "list commit files", failure))?;
    let mut find = DiffFindOptions::new();
    // libgit2's similarity pass loads candidate blobs. Only permit it after
    // header-only size checks; exact object renames never need blob contents.
    let odb = repo
        .odb()
        .map_err(|failure| error(repo.path(), "open rename object database", failure))?;
    let mut candidate_bytes = 0usize;
    let mut bounded = true;
    for delta in diff
        .deltas()
        .filter(|delta| matches!(delta.status(), git2::Delta::Added | git2::Delta::Deleted))
    {
        check(cancel)?;
        let file = if delta.status() == git2::Delta::Deleted {
            delta.old_file()
        } else {
            delta.new_file()
        };
        if file.mode() == git2::FileMode::Commit {
            continue;
        }
        let (size, _) = odb
            .read_header(file.id())
            .map_err(|failure| error(repo.path(), "read rename candidate size", failure))?;
        candidate_bytes = candidate_bytes.saturating_add(size);
        if size > limits.blob_bytes || candidate_bytes > 32 * 1024 * 1024 {
            bounded = false;
            break;
        }
    }
    find.renames(true)
        .rename_threshold(50)
        .rename_limit(1000)
        .exact_match_only(!bounded);
    diff.find_similar(Some(&mut find))
        .map_err(|failure| error(repo.path(), "detect commit renames", failure))?;
    let mut entries = vec![];
    let mut truncated = false;
    for delta in diff.deltas() {
        check(cancel)?;
        if entries.len() == limits.entries {
            truncated = true;
            break;
        }
        let name = if delta.status() == git2::Delta::Deleted {
            delta.old_file()
        } else {
            delta.new_file()
        };
        if let Some(name) = name.path_bytes() {
            entries.push(path(name)?);
        }
    }
    entries.sort();
    entries.dedup();
    Ok(Listing { entries, truncated })
}
fn ignored_patterns(repo: &Repository, cancel: &CancellationToken) -> Result<Vec<String>> {
    use std::io::Read;
    let Some(root) = repo.workdir() else {
        return Ok(vec![]);
    };
    let path = root.join(".gitignore");
    if !path.exists() {
        return Ok(vec![]);
    }
    let file = kode_platform::files::open_regular_file(&path)?;
    let mut bytes = vec![];
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| DomainError::Io {
            operation: "read Git ignore patterns",
            path,
            source,
        })?;
    check(cancel)?;
    if bytes.len() > 1024 * 1024 {
        return Err(DomainError::Unsupported(
            "Git ignore patterns exceed 1 MiB".into(),
        ));
    }
    let text = string(&bytes, "ignore patterns")?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(str::to_owned)
        .collect())
}
fn section<T>(
    result: Result<T>,
    errors: &mut Vec<String>,
    fallback: impl FnOnce() -> T,
) -> Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(DomainError::Cancelled) => Err(DomainError::Cancelled),
        Err(failure) => {
            errors.push(failure.to_string());
            Ok(fallback())
        }
    }
}
fn empty<T>() -> Listing<T> {
    Listing {
        entries: vec![],
        truncated: false,
    }
}
fn snapshot(
    repo: &Repository,
    limit: Option<usize>,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<RepositorySnapshot> {
    let mut errors = vec![];
    let head = section(head(repo).map(Some), &mut errors, || None)?;
    let status = section(status::read(repo, limits, cancel), &mut errors, empty)?;
    let branches = section(branches(repo, limits, cancel), &mut errors, empty)?;
    let tags = section(tags(repo, limits, cancel), &mut errors, empty)?;
    let commits = section(commits(repo, limit, limits, cancel), &mut errors, empty)?;
    check(cancel)?;
    Ok(RepositorySnapshot {
        location: location(repo),
        head,
        status,
        branches,
        tags,
        commits,
        errors,
    })
}
