//! Explicit optional runtime installation; construction never probes Java or networks.
mod archive;
mod http;
mod storage;
use kode_core::{DomainError, Result};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessSpec, run_capture},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;
pub const LATEST_RELEASE: &str = "https://api.github.com/repos/alfu32/kodecraft/releases/latest";
pub fn storage_root(database_home: &Path) -> PathBuf {
    database_home.join("java-runtime")
}
pub const DOWNLOAD_LIMIT: u64 = 128 * 1024 * 1024;
pub const EXTRACT_LIMIT: u64 = 512 * 1024 * 1024;
pub const FILE_LIMIT: usize = 10_000;
pub fn native_target() -> Option<&'static str> {
    if cfg!(all(
        target_os = "linux",
        target_arch = "x86_64",
        target_env = "gnu"
    )) {
        Some("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_os = "linux",
        target_arch = "aarch64",
        target_env = "gnu"
    )) {
        Some("aarch64-unknown-linux-gnu")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    )) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(
        target_os = "windows",
        target_arch = "aarch64",
        target_env = "msvc"
    )) {
        Some("aarch64-pc-windows-msvc")
    } else {
        None
    }
}
#[derive(Clone)]
pub struct Options {
    pub root: PathBuf,
    pub release_url: String,
    pub target: String,
}
impl Options {
    pub fn from_environment(root: PathBuf) -> Result<Self> {
        Ok(Self{root,release_url:std::env::var("KODE_RUNTIME_RELEASE_URL").unwrap_or_else(|_|LATEST_RELEASE.into()),target:native_target().ok_or_else(||invalid("No Java runtime artifact target for this platform; configure local Java 21+"))?.into()})
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: &'static str,
    pub completed: u64,
    pub total: Option<u64>,
}
impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: "Preparing",
            completed: 0,
            total: None,
        }
    }
}
fn progress(
    sender: &watch::Sender<Progress>,
    phase: &'static str,
    completed: u64,
    total: Option<u64>,
) {
    sender.send_replace(Progress {
        phase,
        completed,
        total,
    });
}
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub schema_version: u32,
    pub target: String,
    pub release: String,
    pub sha256: String,
    pub java_major: u32,
    pub generation: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    target: String,
    java_major: u32,
    java_version: String,
    modules: Vec<String>,
    files: BTreeMap<String, FileEntry>,
}
#[derive(Serialize, Deserialize)]
struct FileEntry {
    sha256: String,
    size: u64,
    executable: bool,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}
/// This job runs on the application's blocking scheduler, with network/probe
/// futures driven by its existing Tokio handle. No private runtime is created.
pub struct Installer {
    options: Options,
    http: http::Http,
    runtime: tokio::runtime::Handle,
    factory: Arc<dyn ProcessFactory>,
}
impl Installer {
    pub fn new(
        options: Options,
        runtime: tokio::runtime::Handle,
        factory: Arc<dyn ProcessFactory>,
    ) -> Result<Self> {
        if Some(options.target.as_str()) != native_target() {
            return Err(invalid(
                "Runtime installation target does not match this host",
            ));
        }
        let http = http::Http::new(runtime.clone(), &options.release_url)?;
        Ok(Self {
            options,
            http,
            runtime,
            factory,
        })
    }
    pub fn run(
        self,
        cancel: &CancellationToken,
        sender: &watch::Sender<Progress>,
    ) -> Result<Receipt> {
        let _cancel_on_drop = cancel.clone().drop_guard();
        let result = self.install(cancel, sender);
        if result.is_err() {
            progress(
                sender,
                if cancel.is_cancelled() {
                    "Cancelled"
                } else {
                    "Failed"
                },
                0,
                None,
            );
        }
        result
    }
    fn install(
        &self,
        cancel: &CancellationToken,
        sender: &watch::Sender<Progress>,
    ) -> Result<Receipt> {
        check(cancel)?;
        let stage = storage::Stage::new(&self.options.root)?;
        progress(sender, "Finding latest release", 0, None);
        let bytes = self
            .http
            .small(&self.options.release_url, 2 * 1024 * 1024, cancel)?;
        let release: Release = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("Invalid runtime release metadata"))?;
        if release.draft
            || release.prerelease
            || release.tag_name.is_empty()
            || release.tag_name.len() > 256
            || release.tag_name.chars().any(char::is_control)
            || release.assets.len() > 512
        {
            return Err(invalid(
                "Runtime release metadata is not a supported published release",
            ));
        }
        let name = format!("kode-jdbc-runtime-{}.zip", self.options.target);
        let select = |name: &str| -> Result<&Asset> {
            let mut matches = release.assets.iter().filter(|a| a.name == name);
            let asset=matches.next().ok_or_else(||invalid("Latest release has no matching runtime/checksum asset for this platform; configure local Java or retry after a runtime release"))?;
            if matches.next().is_some() {
                return Err(invalid("Ambiguous runtime release assets"));
            }
            Ok(asset)
        };
        let asset = select(&name)?;
        let checksum_asset = select(&format!("{name}.sha256"))?;
        if asset.size == 0 || asset.size > DOWNLOAD_LIMIT || checksum_asset.size > 1024 {
            return Err(invalid("Runtime release asset exceeds its download limit"));
        }
        let checksum = self
            .http
            .small(&checksum_asset.browser_download_url, 1024, cancel)?;
        let checksum = std::str::from_utf8(&checksum)
            .map_err(|_| invalid("Invalid runtime SHA-256 checksum"))?;
        let mut parts = checksum.split_whitespace();
        let expected = parts
            .next()
            .filter(|v| digest(v))
            .ok_or_else(|| invalid("Invalid runtime SHA-256 checksum"))?
            .to_ascii_lowercase();
        if parts.next() != Some(name.as_str()) || parts.next().is_some() {
            return Err(invalid(
                "Runtime checksum filename does not match the release asset",
            ));
        }
        progress(sender, "Downloading", 0, Some(asset.size));
        let package = stage.path().join("runtime.zip");
        self.http.download(
            &asset.browser_download_url,
            &package,
            asset.size,
            cancel,
            sender,
        )?;
        progress(sender, "Verifying checksum", 0, None);
        if archive::hash(&package, cancel)? != expected {
            return Err(invalid(
                "Java runtime SHA-256 verification failed; no installed runtime was changed",
            ));
        }
        let image = stage.path().join("image");
        progress(sender, "Extracting", 0, None);
        let manifest = archive::extract(&package, &image, &self.options.target, cancel, sender)?;
        progress(sender, "Checking Java", 0, None);
        let java = image.join("runtime/bin").join(java_name());
        let major = self
            .runtime
            .block_on(probe(&java, self.factory.clone(), cancel))?;
        if major != manifest.java_major {
            return Err(invalid(
                "Downloaded runtime Java version does not match its manifest",
            ));
        }
        check(cancel)?;
        progress(sender, "Publishing", 0, None);
        let receipt = stage.publish(
            &image,
            &self.options.target,
            release.tag_name,
            expected,
            major,
            cancel,
        )?;
        progress(sender, "Ready", asset.size, Some(asset.size));
        Ok(receipt)
    }
}
pub fn current_java(root: &Path, target: &str) -> Result<Option<PathBuf>> {
    storage::current(root, target)
}
fn java_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}
fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn check(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else {
        Ok(())
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "Java runtime installation",
        message: message.into(),
    }
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
async fn probe(
    java: &Path,
    factory: Arc<dyn ProcessFactory>,
    cancel: &CancellationToken,
) -> Result<u32> {
    let child = CancellationToken::new();
    let _guard = child.clone().drop_guard();
    let mut spec = ProcessSpec::new(java);
    spec.args.push("-version".into());
    let future = run_capture(spec, vec![], child.clone(), factory.as_ref());
    tokio::pin!(future);
    let output = tokio::select! {biased;
        _=cancel.cancelled()=>{child.cancel();let _=future.await;return Err(DomainError::Cancelled)},
        _=tokio::time::sleep(Duration::from_secs(15))=>{child.cancel();let _=future.await;return Err(invalid("Downloaded Java version probe timed out"))},
        output=&mut future=>output.map_err(|_|invalid("Downloaded Java cannot execute on this platform"))?,
    };
    let major = super::bridge::runtime::java_major(&output.stderr)
        .or_else(|| super::bridge::runtime::java_major(&String::from_utf8_lossy(&output.stdout)));
    if !output.status.success() || output.cancelled || output.truncated {
        return Err(invalid("Downloaded Java failed its version check"));
    }
    major
        .filter(|major| *major >= 21)
        .ok_or_else(|| invalid("Downloaded Java must be version 21 or newer"))
}
