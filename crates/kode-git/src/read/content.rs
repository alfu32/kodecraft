use super::*;
use crate::repository::relative;
use git2::{Oid, Tree};
use kode_core::document::encoding;
use std::{fs, io::Read, path::Path};
fn data(bytes: Vec<u8>, object: Option<String>, mode: u32) -> Content {
    let kind = if mode == 0o120000 {
        ContentKind::Symlink
    } else if bytes.contains(&0) || encoding::decode(&bytes).is_err() {
        ContentKind::Binary
    } else {
        ContentKind::Text
    };
    Content {
        kind,
        length: bytes.len() as u64,
        bytes,
        object,
        mode,
    }
}
fn object(
    repo: &Repository,
    oid: Oid,
    mode: u32,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Content> {
    check(cancel)?;
    if mode == 0o160000 {
        return Ok(Content {
            kind: ContentKind::Submodule,
            bytes: vec![],
            length: 0,
            object: Some(oid.to_string()),
            mode,
        });
    }
    let odb = repo
        .odb()
        .map_err(|failure| error(repo.path(), "open object database", failure))?;
    let (length, kind) = odb
        .read_header(oid)
        .map_err(|failure| error(repo.path(), "read blob header", failure))?;
    if kind != ObjectType::Blob {
        return Err(DomainError::Git {
            operation: "read blob".into(),
            message: format!("{oid} is {kind:?}, not a file blob"),
        });
    }
    if length > limits.blob_bytes {
        return Ok(Content {
            kind: ContentKind::TooLarge,
            bytes: vec![],
            length: length as u64,
            object: Some(oid.to_string()),
            mode,
        });
    }
    check(cancel)?;
    let blob = repo
        .find_blob(oid)
        .map_err(|failure| error(repo.path(), "read blob contents", failure))?;
    check(cancel)?;
    Ok(data(blob.content().to_vec(), Some(oid.to_string()), mode))
}
fn in_tree(
    repo: &Repository,
    tree: &Tree<'_>,
    path: &Path,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Content> {
    let entry = match tree.get_path(path) {
        Ok(entry) => entry,
        Err(failure) if failure.code() == ErrorCode::NotFound => return Ok(Content::missing()),
        Err(failure) => return Err(error(repo.path(), "find file in tree", failure)),
    };
    if entry.kind() == Some(ObjectType::Tree) {
        return Err(DomainError::Unsupported(format!(
            "Git content target is a directory: {}",
            path.display()
        )));
    }
    object(repo, entry.id(), entry.filemode() as u32, limits, cancel)
}
pub(super) fn at_commit(
    repo: &Repository,
    revision: &str,
    path: &Path,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Content> {
    let path = relative(path)?;
    check(cancel)?;
    let commit = repo
        .revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map_err(|failure| error(repo.path(), "resolve content commit", failure))?;
    let tree = commit
        .tree()
        .map_err(|failure| error(repo.path(), "read content tree", failure))?;
    in_tree(repo, &tree, &path, limits, cancel)
}
fn index(
    repo: &Repository,
    path: &Path,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Content> {
    let index = repo
        .index()
        .map_err(|failure| error(repo.path(), "read content index", failure))?;
    if let Some(entry) = index.get_path(path, 0) {
        return object(repo, entry.id, entry.mode, limits, cancel);
    }
    if (1..=3).any(|stage| index.get_path(path, stage).is_some()) {
        return Err(DomainError::Git {
            operation: "read index content".into(),
            message: format!("{} has unresolved merge stages", path.display()),
        });
    }
    Ok(Content::missing())
}
fn working(
    repo: &Repository,
    path: &Path,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<Content> {
    let root = repo.workdir().ok_or_else(|| {
        DomainError::Unsupported("bare repository has no worktree content".into())
    })?;
    let absolute = root.join(path);
    let io_error = |source| DomainError::Io {
        operation: "read Git worktree content",
        path: absolute.clone(),
        source,
    };
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Content::missing());
        }
        Err(failure) => return Err(io_error(failure)),
    };
    let canonical_root = root.canonicalize().map_err(io_error)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| DomainError::Unsupported("worktree file has no parent".into()))?
        .canonicalize()
        .map_err(io_error)?;
    if !parent.starts_with(&canonical_root) {
        return Err(DomainError::Git {
            operation: "read worktree content".into(),
            message: format!("{} has a parent outside the repository", path.display()),
        });
    }
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(&absolute).map_err(io_error)?;
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            target.as_os_str().as_bytes().to_vec()
        };
        #[cfg(not(unix))]
        let bytes = target
            .to_str()
            .ok_or_else(|| DomainError::Unsupported("non-UTF-8 Git symlink target".into()))?
            .as_bytes()
            .to_vec();
        let oid = Oid::hash_object(ObjectType::Blob, &bytes)
            .map_err(|failure| error(repo.path(), "hash symlink content", failure))?;
        return Ok(data(bytes, Some(oid.to_string()), 0o120000));
    }
    if metadata.is_dir() {
        let index = repo
            .index()
            .map_err(|failure| error(repo.path(), "read submodule index", failure))?;
        if let Some(entry) = index.get_path(path, 0)
            && entry.mode == 0o160000
        {
            let sub = Repository::open_ext(
                &absolute,
                git2::RepositoryOpenFlags::NO_SEARCH,
                [] as [&std::ffi::OsStr; 0],
            )
            .ok();
            let oid = sub
                .as_ref()
                .and_then(|repo| repo.head().ok())
                .and_then(|reference| reference.target())
                .unwrap_or(entry.id);
            return Ok(Content {
                kind: ContentKind::Submodule,
                bytes: vec![],
                length: 0,
                object: Some(oid.to_string()),
                mode: 0o160000,
            });
        }
        return Err(DomainError::Unsupported(format!(
            "Git worktree content target is a directory: {}",
            path.display()
        )));
    }
    let mode = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 != 0 {
                0o100755
            } else {
                0o100644
            }
        }
        #[cfg(not(unix))]
        {
            0o100644
        }
    };
    if metadata.len() > limits.blob_bytes as u64 {
        return Ok(Content {
            kind: ContentKind::TooLarge,
            bytes: vec![],
            length: metadata.len(),
            object: None,
            mode,
        });
    }
    let mut file = kode_platform::files::open_regular_file(&absolute)?;
    let before = file.metadata().map_err(io_error)?;
    let mut bytes = vec![];
    (&mut file)
        .take(limits.blob_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    check(cancel)?;
    let after = file.metadata().map_err(io_error)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(absolute));
    }
    if bytes.len() > limits.blob_bytes {
        return Ok(Content {
            kind: ContentKind::TooLarge,
            bytes: vec![],
            length: bytes.len() as u64,
            object: None,
            mode,
        });
    }
    let oid = Oid::hash_object(ObjectType::Blob, &bytes)
        .map_err(|failure| error(repo.path(), "hash worktree content", failure))?;
    Ok(data(bytes, Some(oid.to_string()), mode))
}
pub(super) fn diff(
    repo: &Repository,
    path: &Path,
    staged: bool,
    limits: &ReadLimits,
    cancel: &CancellationToken,
) -> Result<DiffContents> {
    let path = relative(path)?;
    check(cancel)?;
    let (old, new) = if staged {
        let old = match super::head(repo)? {
            Head::Unborn { .. } => Content::missing(),
            _ => at_commit(repo, "HEAD", &path, limits, cancel)?,
        };
        (old, index(repo, &path, limits, cancel)?)
    } else {
        (
            index(repo, &path, limits, cancel)?,
            working(repo, &path, limits, cancel)?,
        )
    };
    Ok(DiffContents {
        path,
        staged,
        old,
        new,
    })
}
