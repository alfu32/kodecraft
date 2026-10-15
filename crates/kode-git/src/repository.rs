use crate::model::RepositoryLocation;
use git2::{ErrorCode, Repository};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use std::path::{Component, Path, PathBuf};
pub(crate) fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
pub(crate) fn error(root: &Path, operation: &str, error: git2::Error) -> DomainError {
    DomainError::Git {
        operation: operation.into(),
        message: format!(
            "{}: {} ({:?}/{:?})",
            root.display(),
            error.message(),
            error.class(),
            error.code()
        ),
    }
}
pub fn discover(start: &Path, cancel: &CancellationToken) -> Result<Option<RepositoryLocation>> {
    check(cancel)?;
    let path = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    match Repository::discover(path) {
        Ok(repo) => {
            check(cancel)?;
            Ok(Some(location(&repo)))
        }
        Err(failure) if failure.code() == ErrorCode::NotFound && !has_marker(path) => Ok(None),
        Err(failure) => Err(error(start, "discover repository", failure)),
    }
}
fn has_marker(path: &Path) -> bool {
    if std::fs::symlink_metadata(path.join(".git")).is_ok() {
        return true;
    }
    path.ancestors().any(|ancestor| {
        let marker = ancestor.join(".git");
        // An unrelated empty .git directory above the requested workspace is
        // not evidence that this workspace owns a damaged repository.
        marker.is_file()
            || marker.join("HEAD").exists()
            || marker.join("config").exists()
            || ancestor.join("HEAD").is_file() && ancestor.join("objects").is_dir()
    })
}
pub(crate) fn location(repo: &Repository) -> RepositoryLocation {
    RepositoryLocation {
        git_dir: repo.path().into(),
        work_tree: repo.workdir().map(Path::to_path_buf),
        bare: repo.is_bare(),
    }
}
pub(crate) fn open(start: &Path, cancel: &CancellationToken) -> Result<Repository> {
    check(cancel)?;
    let path = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    let repo =
        Repository::discover(path).map_err(|failure| error(start, "open repository", failure))?;
    check(cancel)?;
    Ok(repo)
}
pub(crate) fn relative(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty()
        || path.components().any(|part| match part {
            Component::Normal(name) => name.to_string_lossy().eq_ignore_ascii_case(".git"),
            _ => true,
        })
    {
        return Err(DomainError::Git {
            operation: "validate repository path".into(),
            message: format!(
                "expected a relative worktree path without parent or Git metadata components: {}",
                path.display()
            ),
        });
    }
    Ok(path.into())
}
pub(crate) fn path(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| DomainError::Unsupported("non-UTF-8 Git path on this platform".into()))
    }
}
pub(crate) fn string(bytes: &[u8], field: &str) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| DomainError::Unsupported(format!("non-UTF-8 Git {field}")))
}
