//! Hold Git's real index lock while libgit2 edits a private copied index.
//! Publication replaces the index once; failed preparation leaves it unchanged.
use crate::repository::error;
use git2::{Index, Repository};
use kode_core::{DomainError, RequestId, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
};
pub(super) struct IndexTransaction {
    pub index: Index,
    original: PathBuf,
    lock_path: PathBuf,
    lock: File,
    stage: tempfile::TempDir,
    published: bool,
}
fn io(path: &std::path::Path, operation: &'static str, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
struct PreparingLock {
    file: File,
    path: PathBuf,
    armed: bool,
}
impl Drop for PreparingLock {
    fn drop(&mut self) {
        if self.armed && own_file(&self.file, &self.path) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn own_file(file: &File, path: &std::path::Path) -> bool {
    let (Ok(actual), Ok(expected)) = (fs::symlink_metadata(path), file.metadata()) else {
        return false;
    };
    if actual.file_type().is_symlink() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        actual.dev() == expected.dev() && actual.ino() == expected.ino()
    }
    #[cfg(not(unix))]
    {
        actual.len() == expected.len() && actual.modified().ok() == expected.modified().ok()
    }
}
impl IndexTransaction {
    pub fn new(repo: &Repository) -> Result<Self> {
        let original = repo
            .index()
            .map_err(|failure| error(repo.path(), "open index transaction", failure))?
            .path()
            .ok_or_else(|| DomainError::Unsupported("repository index has no file path".into()))?
            .to_owned();
        let mut lock_name = original.as_os_str().to_owned();
        lock_name.push(".lock");
        let lock_path = PathBuf::from(lock_name);
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|failure| io(&lock_path, "acquire Git index lock", failure))?;
        let mut preparing = PreparingLock {
            file,
            path: lock_path.clone(),
            armed: true,
        };
        let nonce = format!(
            "kode-rs index preparation {} {}\n",
            std::process::id(),
            RequestId::fresh()?.0
        );
        preparing
            .file
            .write_all(nonce.as_bytes())
            .map_err(|failure| io(&lock_path, "mark Git index lock", failure))?;
        let parent = original
            .parent()
            .ok_or_else(|| DomainError::Unsupported("index has no parent".into()))?;
        let stage = tempfile::Builder::new()
            .prefix(".kode-rs-index-")
            .tempdir_in(parent)
            .map_err(|failure| io(parent, "prepare Git index directory", failure))?;
        let staged = stage.path().join("index");
        match fs::symlink_metadata(&original) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(DomainError::Unsupported(
                        "Git index must be a regular file".into(),
                    ));
                }
                if metadata.len() > 128 * 1024 * 1024 {
                    return Err(DomainError::Unsupported(
                        "Git index exceeds 128 MiB write limit".into(),
                    ));
                }
                let mut input = kode_platform::files::open_regular_file(&original)?;
                let mut output = File::create(&staged)
                    .map_err(|failure| io(&staged, "create private Git index", failure))?;
                std::io::copy(&mut input, &mut output)
                    .map_err(|failure| io(&staged, "copy Git index", failure))?;
                preparing
                    .file
                    .set_permissions(metadata.permissions())
                    .map_err(|failure| io(&lock_path, "preserve index permissions", failure))?;
            }
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {}
            Err(failure) => return Err(io(&original, "stat Git index", failure)),
        }
        let mut index = Index::open(&staged)
            .map_err(|failure| error(repo.path(), "open private index", failure))?;
        repo.set_index(&mut index)
            .map_err(|failure| error(repo.path(), "attach private index", failure))?;
        let lock = preparing
            .file
            .try_clone()
            .map_err(|failure| io(&lock_path, "retain index lock", failure))?;
        preparing.armed = false;
        Ok(Self {
            index,
            original,
            lock_path,
            lock,
            stage,
            published: false,
        })
    }
    pub fn publish(&mut self, repo: &Repository) -> Result<Vec<String>> {
        self.index
            .write()
            .map_err(|failure| error(repo.path(), "write private index", failure))?;
        let staged = self.stage.path().join("index");
        let mut input =
            File::open(&staged).map_err(|failure| io(&staged, "read prepared index", failure))?;
        self.lock
            .seek(SeekFrom::Start(0))
            .map_err(|failure| io(&self.lock_path, "rewind index lock", failure))?;
        self.lock
            .set_len(0)
            .map_err(|failure| io(&self.lock_path, "prepare index publication", failure))?;
        std::io::copy(&mut input, &mut self.lock)
            .map_err(|failure| io(&self.lock_path, "write index publication", failure))?;
        self.lock
            .sync_all()
            .map_err(|failure| io(&self.lock_path, "flush index publication", failure))?;
        if !own_file(&self.lock, &self.lock_path) {
            return Err(DomainError::Git {
                operation: "publish index".into(),
                message: "index lock was replaced by another process".into(),
            });
        }
        fs::rename(&self.lock_path, &self.original)
            .map_err(|failure| io(&self.original, "publish Git index", failure))?;
        self.published = true;
        let mut warnings = vec![];
        #[cfg(unix)]
        if let Some(parent) = self.original.parent()
            && let Err(failure) = File::open(parent).and_then(|directory| directory.sync_all())
        {
            warnings.push(format!(
                "Index published, but directory flush failed: {failure}"
            ));
        }
        Ok(warnings)
    }
}
impl Drop for IndexTransaction {
    fn drop(&mut self) {
        if !self.published && own_file(&self.lock, &self.lock_path) {
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}
