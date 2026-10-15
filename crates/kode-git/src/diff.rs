//! Immutable, zero-context native diff hunks. Restore uses these exact byte
//! ranges and revalidates both sources; it never recomputes chunk numbering.
use crate::{
    model::*,
    read::{ReadJob, ReadOutput, ReadRequest},
    repository::{check, open, relative},
};
use git2::{DiffOptions, Patch};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, Result, Revision,
    document::encoding,
    intelligence::DocumentSnapshot,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::{
    files::{NativeDocumentIo, disk_identity},
    jobs::CancellationToken,
};
use sha2::{Digest, Sha256};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_lines: Range<usize>,
    pub new_lines: Range<usize>,
    pub old_bytes: Range<usize>,
    pub new_bytes: Range<usize>,
}
#[derive(Debug)]
pub struct DiffPlan {
    contents: DiffContents,
    hunks: Vec<Hunk>,
    old_lines: Vec<Range<usize>>,
    new_lines: Vec<Range<usize>>,
    notice: Option<String>,
    worktree_identity: Option<(PathBuf, Option<DiskIdentity>)>,
}
fn failure(message: impl Into<String>) -> DomainError {
    DomainError::Git {
        operation: "prepare/restore diff".into(),
        message: message.into(),
    }
}
fn lines(bytes: &[u8]) -> Vec<Range<usize>> {
    let mut start = 0;
    let mut result = vec![];
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            result.push(start..index + 1);
            start = index + 1;
            if result.len() > 200_000 {
                break;
            }
        }
    }
    if start < bytes.len() {
        result.push(start..bytes.len());
    }
    result
}
fn span(
    lines: &[Range<usize>],
    start: u32,
    count: u32,
    bytes: usize,
) -> Result<(Range<usize>, Range<usize>)> {
    let start = if count == 0 {
        start as usize
    } else {
        start.saturating_sub(1) as usize
    };
    let end = start.saturating_add(count as usize);
    if end > lines.len() {
        return Err(failure("native hunk exceeds its captured source"));
    }
    let byte_start = lines.get(start).map_or(bytes, |line| line.start);
    let byte_end = if count == 0 {
        byte_start
    } else {
        lines[end - 1].end
    };
    Ok((start..end, byte_start..byte_end))
}
impl DiffPlan {
    pub fn build(contents: DiffContents, cancel: &CancellationToken) -> Result<Self> {
        check(cancel)?;
        let text = |kind| matches!(kind, ContentKind::Text | ContentKind::Missing);
        let notice = if !text(contents.old.kind) || !text(contents.new.kind) {
            Some(format!(
                "Content preview unavailable: {:?} → {:?}",
                contents.old.kind, contents.new.kind
            ))
        } else {
            None
        };
        let mut plan = Self {
            contents,
            hunks: vec![],
            old_lines: vec![],
            new_lines: vec![],
            notice,
            worktree_identity: None,
        };
        if plan.notice.is_some() {
            return Ok(plan);
        }
        if plan
            .contents
            .old
            .bytes
            .len()
            .max(plan.contents.new.bytes.len())
            > 8 * 1024 * 1024
        {
            plan.notice = Some("Diff exceeds 8 MiB per-source limit".into());
            return Ok(plan);
        }
        plan.old_lines = lines(&plan.contents.old.bytes);
        plan.new_lines = lines(&plan.contents.new.bytes);
        if plan.old_lines.len().saturating_add(plan.new_lines.len()) > 200_000 {
            plan.old_lines.clear();
            plan.new_lines.clear();
            plan.notice = Some("Diff exceeds 200,000 combined source lines".into());
            return Ok(plan);
        }
        let mut options = DiffOptions::new();
        options.context_lines(0).interhunk_lines(0).force_text(true);
        let patch = Patch::from_buffers(
            &plan.contents.old.bytes,
            None,
            &plan.contents.new.bytes,
            None,
            Some(&mut options),
        )
        .map_err(|error| failure(error.to_string()))?;
        for index in 0..patch.num_hunks() {
            check(cancel)?;
            let (hunk, _) = patch
                .hunk(index)
                .map_err(|error| failure(error.to_string()))?;
            let (old_lines, old_bytes) = span(
                &plan.old_lines,
                hunk.old_start(),
                hunk.old_lines(),
                plan.contents.old.bytes.len(),
            )?;
            let (new_lines, new_bytes) = span(
                &plan.new_lines,
                hunk.new_start(),
                hunk.new_lines(),
                plan.contents.new.bytes.len(),
            )?;
            plan.hunks.push(Hunk {
                old_lines,
                new_lines,
                old_bytes,
                new_bytes,
            });
        }
        drop(patch);
        Ok(plan)
    }
    pub fn contents(&self) -> &DiffContents {
        &self.contents
    }
    pub fn capture(
        root: &Path,
        contents: DiffContents,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let mut plan = Self::build(contents, cancel)?;
        if plan.restorable() {
            let repo = open(root, cancel)?;
            let root = repo
                .workdir()
                .ok_or_else(|| failure("bare repository has no worktree"))?;
            let path = root.join(relative(&plan.contents.path)?);
            let identity = if plan.contents.new.kind == ContentKind::Missing {
                None
            } else {
                let identity = disk_identity(&path)?;
                if identity.content_hash
                    != format!("{:x}", Sha256::digest(&plan.contents.new.bytes))
                {
                    return Err(DomainError::SaveConflict(path));
                }
                Some(identity)
            };
            plan.worktree_identity = Some((path, identity));
        }
        check(cancel)?;
        Ok(plan)
    }
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }
    pub fn old_lines(&self) -> &[Range<usize>] {
        &self.old_lines
    }
    pub fn new_lines(&self) -> &[Range<usize>] {
        &self.new_lines
    }
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
    pub fn restorable(&self) -> bool {
        self.notice.is_none() && !self.contents.staged && self.contents.revision.is_none()
    }
    pub fn restored_bytes(&self, hunk: usize) -> Result<Vec<u8>> {
        if !self.restorable() {
            return Err(failure(
                "only captured unstaged text chunks can be restored",
            ));
        }
        let hunk = self
            .hunks
            .get(hunk)
            .ok_or_else(|| failure("unknown captured chunk"))?;
        let mut bytes = self.contents.new.bytes[..hunk.new_bytes.start].to_vec();
        bytes.extend_from_slice(&self.contents.old.bytes[hunk.old_bytes.clone()]);
        bytes.extend_from_slice(&self.contents.new.bytes[hunk.new_bytes.end..]);
        Ok(bytes)
    }
}
pub fn read(
    root: &Path,
    request: ReadRequest,
    cancel: &CancellationToken,
) -> Result<Arc<DiffPlan>> {
    match ReadJob::new(root.into(), request).run(cancel)? {
        ReadOutput::Diff(contents) => DiffPlan::capture(root, contents, cancel).map(Arc::new),
        _ => Err(failure("expected a diff request")),
    }
}
#[derive(Debug, Clone)]
pub struct RestoreJob {
    pub root: PathBuf,
    pub plan: Arc<DiffPlan>,
    pub hunk: usize,
}
#[derive(Debug)]
pub struct RestoreReceipt {
    pub path: PathBuf,
    pub disk_identity: DiskIdentity,
}
impl RestoreJob {
    pub fn run(&self, cancel: &CancellationToken) -> Result<RestoreReceipt> {
        check(cancel)?;
        let bytes = self.plan.restored_bytes(self.hunk)?;
        let path = relative(&self.plan.contents.path)?;
        let repo = open(&self.root, cancel)?;
        let _index_lock = crate::write::index::IndexTransaction::new(&repo)?;
        let root = repo
            .workdir()
            .ok_or_else(|| failure("bare repository has no worktree"))?;
        let target = root.join(&path);
        let (captured_path, captured_identity) = self
            .plan
            .worktree_identity
            .as_ref()
            .ok_or_else(|| failure("restore requires a captured worktree identity"))?;
        if &target != captured_path {
            return Err(DomainError::SaveConflict(target));
        }
        let canonical_root = root.canonicalize().map_err(|source| DomainError::Io {
            operation: "resolve restore root",
            path: root.into(),
            source,
        })?;
        let parent = target
            .parent()
            .ok_or_else(|| failure("restore target has no parent"))?
            .canonicalize()
            .map_err(|source| DomainError::Io {
                operation: "resolve restore parent",
                path: target.clone(),
                source,
            })?;
        if !parent.starts_with(&canonical_root) {
            return Err(failure("restore parent escapes repository"));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&target)
            && !metadata.is_file()
        {
            return Err(DomainError::SaveConflict(target));
        }
        let current = ReadJob::new(
            self.root.clone(),
            ReadRequest::Diff {
                path,
                staged: false,
            },
        )
        .run(cancel)?;
        if !matches!(current, ReadOutput::Diff(ref diff) if diff == &self.plan.contents) {
            return Err(DomainError::SaveConflict(target));
        }
        let expected = if self.plan.contents.new.kind == ContentKind::Missing {
            None
        } else {
            let identity = disk_identity(&target)?;
            if Some(&identity) != captured_identity.as_ref() {
                return Err(DomainError::SaveConflict(target));
            }
            if !identity.canonical_path.starts_with(&canonical_root) {
                return Err(DomainError::SaveConflict(target));
            }
            // The identity read must describe exactly the captured bytes, not a
            // concurrent replacement that happened after the source comparison.
            let hash = format!("{:x}", Sha256::digest(&self.plan.contents.new.bytes));
            if identity.content_hash != hash
                || identity.length != self.plan.contents.new.bytes.len() as u64
            {
                return Err(DomainError::SaveConflict(target));
            }
            Some(identity)
        };
        check(cancel)?;
        let decoded = encoding::decode(&bytes)?;
        let snapshot = DocumentSnapshot {
            id: DocumentId::fresh()?,
            revision: Revision(0),
            origin: DocumentOrigin::File(target.clone()),
            language: None,
            format: decoded.format,
            text: decoded.text.into(),
        };
        let disk_identity = NativeDocumentIo.save(&target, &snapshot, expected.as_ref())?;
        Ok(RestoreReceipt {
            path: target,
            disk_identity,
        })
    }
}
