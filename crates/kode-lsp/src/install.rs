//! Blocking-worker installation. Network futures use the existing Tokio runtime;
//! no private runtime, shell command, worker pool or background child is created.
mod archive;
use crate::{
    catalog::{Extra, InstallManifest, ServerEntry, invalid, validate_id},
    discovery::DiscoveryOptions,
};
use kode_core::{DomainError, RequestId, Result};
use kode_platform::jobs::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_ARCHIVE_FILES: usize = 100_000;
#[derive(Debug, Clone)]
pub struct InstallProgress {
    pub phase: &'static str,
    pub completed: u64,
    pub total: Option<u64>,
}
#[derive(Debug, Clone)]
pub struct InstallReceipt {
    pub manifest: InstallManifest,
    pub directory: PathBuf,
    pub previous: Option<PathBuf>,
}
pub trait PackageFetcher: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        output: &mut File,
        cancel: &CancellationToken,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()>;
}
pub struct HttpFetcher {
    client: OnceLock<reqwest::Client>,
    runtime: tokio::runtime::Handle,
}
impl HttpFetcher {
    /// Captures only the scheduler handle. CA/TLS initialization belongs to an
    /// explicit download worker, so a missing trust store cannot stop the editor.
    pub fn new(runtime: tokio::runtime::Handle) -> Result<Self> {
        Ok(Self {
            client: OnceLock::new(),
            runtime,
        })
    }
    fn client(&self) -> Result<reqwest::Client> {
        if let Some(client) = self.client.get() {
            return Ok(client.clone());
        }
        let client = reqwest::Client::builder()
            .user_agent("kode-rs/0.1 lsp-installer")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|_| DomainError::HttpTransport("Cannot configure the LSP download client; check system CA certificates and TLS support, then retry".into()))?;
        // Failed construction is not cached: installing/repairing the trust
        // store permits an explicit retry in the same workspace.
        let _ = self.client.set(client.clone());
        Ok(self.client.get().cloned().unwrap_or(client))
    }
}
impl PackageFetcher for HttpFetcher {
    /// Must run on a blocking worker, never the application/Tokio async thread.
    fn fetch(
        &self,
        url: &str,
        output: &mut File,
        cancel: &CancellationToken,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<()> {
        check(cancel)?;
        let client = self.client()?;
        self.runtime.block_on(async {
            tokio::select! {
                biased;
                _=cancel.cancelled()=>Err(DomainError::Cancelled),
                result=async {
                    let mut response=client.get(url).send().await.map_err(|e|DomainError::HttpTransport(e.to_string()))?.error_for_status().map_err(|e|DomainError::HttpTransport(e.to_string()))?;
                    let total=response.content_length();
                    if total.is_some_and(|total|total>MAX_DOWNLOAD_BYTES) { return Err(invalid("package exceeds 256 MiB download limit")); }
                    let mut count=0u64;
                    while let Some(chunk)=response.chunk().await.map_err(|e|DomainError::HttpTransport(e.to_string()))? {
                        count=count.saturating_add(chunk.len() as u64);
                        if count>MAX_DOWNLOAD_BYTES { return Err(invalid("package exceeds 256 MiB download limit")); }
                        output.write_all(&chunk).map_err(|e|io("write downloaded package",Path::new("<temporary package>"),e))?;
                        progress(count,total);
                    }
                    if total.is_some_and(|total|total!=count) { return Err(invalid("truncated package download")); }
                    Ok(())
                }=>result,
            }
        })
    }
}
pub(super) fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
pub(super) fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid(format!(
            "expected a regular install directory: {}",
            path.display()
        ))),
        Err(error) => Err(io("inspect install directory", path, error)),
    }
}
struct InstallLock {
    path: PathBuf,
    nonce: Vec<u8>,
}
impl InstallLock {
    fn acquire(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).map_err(|e| io("create LSP install root", root, e))?;
        directory(root)?;
        let path = root.join(".kode-rs-install.lock");
        let nonce = format!("{}:{}", std::process::id(), RequestId::fresh()?.0).into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                io(
                    "lock LSP install root (another operation or retained lock)",
                    &path,
                    e,
                )
            })?;
        file.write_all(&nonce)
            .map_err(|e| io("write install lock", &path, e))?;
        Ok(Self { path, nonce })
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        if fs::read(&self.path).ok().as_deref() == Some(&self.nonce) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallJob {
    pub entry: ServerEntry,
    pub options: DiscoveryOptions,
}
impl InstallJob {
    pub fn run(
        &self,
        fetcher: &dyn PackageFetcher,
        cancel: &CancellationToken,
        mut progress: impl FnMut(InstallProgress),
    ) -> Result<InstallReceipt> {
        validate_id(&self.entry.id)?;
        check(cancel)?;
        let (version, download) = self.entry.download(&self.options.platform).ok_or_else(|| {
            let instructions = self
                .entry
                .versions
                .first()
                .filter(|version| !version.install_instructions.is_empty())
                .map_or(String::new(), |version| {
                    format!(
                        "; manual setup: {}",
                        serde_json::to_string(&version.install_instructions).unwrap_or_default()
                    )
                });
            invalid(format!(
                "No download for {} on {}/{}; configure a local executable{instructions}",
                self.entry.name, self.options.platform.os, self.options.platform.arch
            ))
        })?;
        let _lock = InstallLock::acquire(&self.options.install_root)?;
        let destination = self.options.directory(&self.entry.id)?;
        if destination
            .try_exists()
            .map_err(|e| io("inspect previous server", &destination, e))?
        {
            directory(&destination)?;
        }
        let temporary = tempfile::Builder::new()
            .prefix(".kode-rs-lsp-stage-")
            .tempdir_in(&self.options.install_root)
            .map_err(|e| io("stage LSP package", &self.options.install_root, e))?;
        let package = temporary.path().join("package.download");
        let contents = temporary.path().join("contents");
        fs::create_dir(&contents).map_err(|e| io("stage LSP contents", &contents, e))?;
        progress(InstallProgress {
            phase: "Downloading",
            completed: 0,
            total: None,
        });
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&package)
            .map_err(|e| io("create package download", &package, e))?;
        fetcher.fetch(&download.url, &mut file, cancel, &mut |completed, total| {
            progress(InstallProgress {
                phase: "Downloading",
                completed,
                total,
            })
        })?;
        file.sync_all()
            .map_err(|e| io("sync package", &package, e))?;
        drop(file);
        check(cancel)?;
        let size = fs::metadata(&package)
            .map_err(|e| io("stat package", &package, e))?
            .len();
        if size > MAX_DOWNLOAD_BYTES {
            return Err(invalid("package exceeds download limit"));
        }
        progress(InstallProgress {
            phase: "Checking checksum",
            completed: 0,
            total: Some(size),
        });
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut file =
            File::open(&package).map_err(|e| io("read package checksum", &package, e))?;
        loop {
            check(cancel)?;
            let count = file
                .read(&mut buffer)
                .map_err(|e| io("read package checksum", &package, e))?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let checksum = format!("{:x}", digest.finalize());
        if download
            .sha256
            .as_ref()
            .is_some_and(|expected| !expected.eq_ignore_ascii_case(&checksum))
        {
            return Err(invalid(
                "package SHA-256 checksum mismatch; previous installation retained",
            ));
        }
        progress(InstallProgress {
            phase: "Extracting",
            completed: 0,
            total: None,
        });
        let command = download
            .command
            .clone()
            .or_else(|| self.entry.preferred_command.clone());
        archive::extract(
            &package,
            &contents,
            download,
            command.as_deref(),
            cancel,
            &mut |completed| {
                progress(InstallProgress {
                    phase: "Extracting",
                    completed,
                    total: None,
                })
            },
        )?;
        let command = archive::installed_command(
            &contents,
            download.unpack_subdir.as_deref(),
            command,
            &self.options.platform,
        )?;
        let manifest = InstallManifest {
            id: self.entry.id.clone(),
            version: version.version.clone(),
            installed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| invalid(e.to_string()))?
                .as_millis()
                .try_into()
                .map_err(|_| invalid("installation timestamp overflow"))?,
            source_url: Some(download.url.clone()),
            platform_os: Some(self.options.platform.os.clone()),
            platform_arch: Some(self.options.platform.arch.clone()),
            command,
            unpack_subdir: download.unpack_subdir.clone(),
            extra: Extra::from_iter([
                ("kodeRsInstaller".into(), 1.into()),
                ("archiveSha256".into(), checksum.into()),
            ]),
        };
        // Resolve inside staging before touching the old installation, including
        // interpreter commands and arguments relative to unpackSubdir.
        let mut staged_options = self.options.clone();
        staged_options.install_root = temporary.path().to_owned();
        staged_options.configured_commands.clear();
        let mut staged_entry = self.entry.clone();
        staged_entry.id = "contents".into();
        staged_options.resolve_command(&staged_entry, Some(&manifest))?;
        let path = contents.join("installed.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| io("write install manifest", &path, e))?;
        file.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| invalid(e.to_string()))?)
            .map_err(|e| io("write install manifest", &path, e))?;
        file.sync_all()
            .map_err(|e| io("sync install manifest", &path, e))?;
        drop(file);
        check(cancel)?;
        progress(InstallProgress {
            phase: "Publishing",
            completed: 0,
            total: None,
        });
        // Publication is an acknowledged critical phase: once replacement starts,
        // finish/rollback even if cancellation arrives. Keep previous user files.
        let previous = if fs::symlink_metadata(&destination).is_ok() {
            directory(&destination)?;
            let backup = self.options.install_root.join(format!(
                ".kode-rs-previous-{}-{}",
                self.entry.id,
                RequestId::fresh()?.0
            ));
            fs::rename(&destination, &backup)
                .map_err(|e| io("retain previous LSP installation", &destination, e))?;
            Some(backup)
        } else {
            None
        };
        if let Err(error) = fs::rename(&contents, &destination) {
            if let Some(backup) = &previous
                && !destination.exists()
            {
                fs::rename(backup,&destination).map_err(|rollback|invalid(format!("publication failed: {error}; previous install retained at {} (rollback failed: {rollback})",backup.display())))?;
            }
            return Err(io("publish LSP installation", &destination, error));
        }
        progress(InstallProgress {
            phase: "Installed",
            completed: 1,
            total: Some(1),
        });
        Ok(InstallReceipt {
            manifest,
            directory: destination,
            previous,
        })
    }
}
/// Explicit uninstall of one selected server. Never remove the install root or
/// retained previous versions. A symlink is refused before traversal/removal.
pub fn uninstall(options: &DiscoveryOptions, id: &str, cancel: &CancellationToken) -> Result<bool> {
    validate_id(id)?;
    check(cancel)?;
    if !options.install_root.exists() {
        return Ok(false);
    }
    let _lock = InstallLock::acquire(&options.install_root)?;
    let destination = options.directory(id)?;
    match fs::symlink_metadata(&destination) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io("inspect server for removal", &destination, e)),
        Ok(_) => directory(&destination)?,
    }
    let removal = options
        .install_root
        .join(format!(".kode-rs-remove-{id}-{}", RequestId::fresh()?.0));
    check(cancel)?;
    fs::rename(&destination, &removal).map_err(|e| io("detach removed server", &destination, e))?;
    fs::remove_dir_all(&removal).map_err(|e| io("remove detached server", &removal, e))?;
    Ok(true)
}
