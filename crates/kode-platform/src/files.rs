use kode_core::{DocumentOrigin, DomainError, JobTag, Result};
use kode_core::{
    document::encoding::encode,
    intelligence::DocumentSnapshot,
    services::{DiskIdentity, DocumentIo, LoadedDocument},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Debug, Default)]
pub struct NativeDocumentIo;
fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn identity(path: &Path, metadata: &fs::Metadata, bytes: &[u8]) -> DiskIdentity {
    DiskIdentity {
        canonical_path: path.into(),
        length: bytes.len() as u64,
        modified_nanos: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos()),
        content_hash: format!("{:x}", Sha256::digest(bytes)),
    }
}
pub fn disk_identity(path: &Path) -> Result<DiskIdentity> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_error("resolve file", path, source))?;
    let mut file =
        fs::File::open(&canonical).map_err(|source| io_error("open file", path, source))?;
    let metadata = file
        .metadata()
        .map_err(|source| io_error("stat file", path, source))?;
    let mut bytes = vec![];
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error("read file", path, source))?;
    Ok(identity(&canonical, &metadata, &bytes))
}
impl DocumentIo for NativeDocumentIo {
    fn load(&self, origin: &DocumentOrigin, tag: JobTag) -> Result<LoadedDocument> {
        let DocumentOrigin::File(path) = origin else {
            return Err(DomainError::NoSaveTarget);
        };
        let canonical = fs::canonicalize(path)
            .map_err(|source| io_error("resolve load target", path, source))?;
        let mut file =
            fs::File::open(&canonical).map_err(|source| io_error("open document", path, source))?;
        let before = file
            .metadata()
            .map_err(|source| io_error("stat document", path, source))?;
        let mut bytes = vec![];
        file.read_to_end(&mut bytes)
            .map_err(|source| io_error("read document", path, source))?;
        let after = file
            .metadata()
            .map_err(|source| io_error("stat loaded document", path, source))?;
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err(DomainError::SaveConflict(path.clone()));
        }
        let disk_identity = identity(&canonical, &after, &bytes);
        Ok(LoadedDocument {
            tag,
            origin: origin.clone(),
            bytes,
            disk_identity: Some(disk_identity),
        })
    }
    fn save(
        &self,
        path: &Path,
        snapshot: &DocumentSnapshot,
        expected: Option<&DiskIdentity>,
    ) -> Result<DiskIdentity> {
        if !matches!(snapshot.origin, DocumentOrigin::File(_)) {
            return Err(DomainError::NoSaveTarget);
        }
        let target = if let Some(expected) = expected {
            let actual = disk_identity(path).map_err(|error| match error {
                DomainError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                    DomainError::SaveConflict(path.into())
                }
                other => other,
            })?;
            if &actual != expected {
                return Err(DomainError::SaveConflict(path.into()));
            }
            actual.canonical_path
        } else {
            if path.exists() || fs::symlink_metadata(path).is_ok() {
                return Err(DomainError::SaveConflict(path.into()));
            }
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let parent = fs::canonicalize(parent)
                .map_err(|source| io_error("resolve save directory", parent, source))?;
            parent.join(
                path.file_name()
                    .ok_or_else(|| DomainError::Unsupported("missing save filename".into()))?,
            )
        };
        let parent: PathBuf = target
            .parent()
            .ok_or_else(|| DomainError::Unsupported("save target has no parent".into()))?
            .into();
        let permissions = if expected.is_some() {
            let metadata = fs::metadata(&target)
                .map_err(|source| io_error("stat save target", path, source))?;
            if metadata.permissions().readonly() {
                return Err(io_error(
                    "save read-only file",
                    path,
                    std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                ));
            }
            Some(metadata.permissions())
        } else {
            None
        };
        let bytes = encode(&snapshot.text, snapshot.format);
        let mut temporary = tempfile::NamedTempFile::new_in(&parent)
            .map_err(|source| io_error("create atomic save", path, source))?;
        #[cfg(unix)]
        if expected.is_some() {
            use nix::unistd::{Gid, Uid, fchown};
            use std::os::unix::fs::MetadataExt;
            let original = fs::metadata(&target)
                .map_err(|source| io_error("stat save ownership", path, source))?;
            let staged = temporary
                .as_file()
                .metadata()
                .map_err(|source| io_error("stat temporary ownership", path, source))?;
            if original.uid() != staged.uid() || original.gid() != staged.gid() {
                fchown(
                    temporary.as_file(),
                    Some(Uid::from_raw(original.uid())),
                    Some(Gid::from_raw(original.gid())),
                )
                .map_err(|error| {
                    io_error(
                        "preserve save ownership",
                        path,
                        std::io::Error::from_raw_os_error(error as i32),
                    )
                })?;
            }
        }
        if let Some(permissions) = permissions {
            temporary
                .as_file()
                .set_permissions(permissions)
                .map_err(|source| io_error("preserve file permissions", path, source))?;
        }
        temporary
            .write_all(&bytes)
            .map_err(|source| io_error("write atomic save", path, source))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|source| io_error("flush atomic save", path, source))?;
        // Recheck immediately before replacement, after the potentially slow write.
        if let Some(expected) = expected {
            if disk_identity(path)? != *expected {
                return Err(DomainError::SaveConflict(path.into()));
            }
            temporary
                .persist(&target)
                .map_err(|error| io_error("replace saved document", path, error.error))?;
        } else {
            temporary
                .persist_noclobber(&target)
                .map_err(|error| io_error("create saved document", path, error.error))?;
        }
        #[cfg(unix)]
        fs::File::open(&parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| io_error("flush save directory", &parent, source))?;
        disk_identity(path)
    }
}
