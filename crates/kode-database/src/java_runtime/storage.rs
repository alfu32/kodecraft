use super::{Receipt, check, digest, invalid, io};
use kode_core::{RequestId, Result};
use kode_platform::jobs::CancellationToken;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub(super) struct Stage {
    root: PathBuf,
    temp: tempfile::TempDir,
    lock: PathBuf,
    nonce: Vec<u8>,
}
impl Drop for Stage {
    fn drop(&mut self) {
        if fs::read(&self.lock).ok().as_deref() == Some(&self.nonce) {
            let _ = fs::remove_file(&self.lock);
        }
    }
}
impl Stage {
    pub fn new(root: &Path) -> Result<Self> {
        crate::config::reject_links(root)?;
        fs::create_dir_all(root).map_err(|e| io("create managed Java storage", root, e))?;
        let lock = root.join(".install.lock");
        let nonce = format!("{}:{}", std::process::id(), RequestId::fresh()?.0).into_bytes();
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&lock).map_err(|e| {
            io(
                "lock Java storage (another install or a retained .install.lock)",
                &lock,
                e,
            )
        })?;
        if let Err(error) = file.write_all(&nonce).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&lock);
            return Err(io("write Java install lock", &lock, error));
        }
        let temp = match tempfile::Builder::new()
            .prefix(".java-install-")
            .tempdir_in(root)
        {
            Ok(temp) => temp,
            Err(error) => {
                let _ = fs::remove_file(&lock);
                return Err(io("create Java staging directory", root, error));
            }
        };
        Ok(Self {
            root: root.into(),
            temp,
            lock,
            nonce,
        })
    }
    pub fn path(&self) -> &Path {
        self.temp.path()
    }
    pub fn publish(
        self,
        image: &Path,
        target: &str,
        release: String,
        sha256: String,
        java_major: u32,
        cancel: &CancellationToken,
    ) -> Result<Receipt> {
        check(cancel)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("Cannot identify the installed runtime generation"))?
            .as_nanos();
        let generation = format!("{sha256}-{stamp}-{}", RequestId::fresh()?.0);
        let receipt = Receipt {
            schema_version: 1,
            target: target.into(),
            release,
            sha256,
            java_major,
            generation,
        };
        let bytes = serde_json::to_vec_pretty(&receipt)
            .map_err(|_| invalid("Cannot encode installed runtime receipt"))?;
        fs::write(image.join("installed.json"), &bytes)
            .map_err(|e| io("write staged runtime receipt", image, e))?;
        let versions = self.root.join("versions");
        crate::config::reject_links(&versions)?;
        fs::create_dir_all(&versions)
            .map_err(|e| io("create runtime version storage", &versions, e))?;
        let installed = versions.join(&receipt.generation);
        fs::rename(image, &installed)
            .map_err(|e| io("publish runtime generation", &installed, e))?;
        let publish = (|| {
            check(cancel)?;
            let mut pointer = tempfile::NamedTempFile::new_in(&self.root)
                .map_err(|e| io("create runtime pointer", &self.root, e))?;
            pointer
                .write_all(&bytes)
                .and_then(|_| pointer.as_file().sync_all())
                .map_err(|e| io("write runtime pointer", &self.root, e))?;
            pointer
                .persist(self.root.join("current.json"))
                .map_err(|e| io("publish runtime pointer", &self.root, e.error))?;
            Ok(())
        })();
        if let Err(error) = publish {
            let _ = fs::remove_dir_all(&installed);
            return Err(error);
        }
        Ok(receipt)
    }
}
pub(super) fn current(root: &Path, target: &str) -> Result<Option<PathBuf>> {
    let pointer = root.join("current.json");
    crate::config::reject_links(&pointer)?;
    let meta = match fs::metadata(&pointer) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io("inspect managed Java pointer", &pointer, e)),
    };
    if !meta.is_file() || meta.len() > 4096 {
        return Err(invalid(
            "Invalid managed Java pointer; install the runtime again",
        ));
    }
    let bytes = fs::read(&pointer).map_err(|e| io("read managed Java pointer", &pointer, e))?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("Invalid managed Java pointer; install the runtime again"))?;
    if receipt.schema_version != 1
        || receipt.target != target
        || receipt.java_major < 21
        || !digest(&receipt.sha256)
        || !receipt
            .generation
            .starts_with(&format!("{}-", receipt.sha256))
        || receipt.generation.len() > 192
        || receipt
            .generation
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'-')
    {
        return Err(invalid(
            "Managed runtime pointer does not match this platform",
        ));
    }
    let directory = root.join("versions").join(&receipt.generation);
    crate::config::reject_links(&directory)?;
    let installed = directory.join("installed.json");
    crate::config::reject_links(&installed)?;
    let installed_meta =
        fs::metadata(&installed).map_err(|e| io("inspect runtime receipt", &installed, e))?;
    if !installed_meta.is_file()
        || installed_meta.len() > 4096
        || fs::read(&installed).map_err(|e| io("read runtime receipt", &installed, e))? != bytes
    {
        return Err(invalid(
            "Managed runtime receipt does not match its active pointer",
        ));
    }
    let java = directory.join("runtime/bin").join(super::java_name());
    crate::config::reject_links(&java)?;
    if !java.is_file() {
        return Err(invalid(
            "Managed Java executable is missing; install the runtime again",
        ));
    }
    Ok(Some(java))
}
