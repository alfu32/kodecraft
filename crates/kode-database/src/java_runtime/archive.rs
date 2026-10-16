use super::{
    EXTRACT_LIMIT, FILE_LIMIT, FileEntry, Manifest, Progress, check, digest, invalid, io, progress,
};
use kode_core::Result;
use kode_platform::jobs::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use tokio::sync::watch;
fn relative(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', ':'])
        || value.chars().any(char::is_control)
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid("Runtime archive contains an unsafe path"));
    }
    Ok(path.into())
}
pub(super) fn hash(path: &Path, cancel: &CancellationToken) -> Result<String> {
    let mut input = File::open(path).map_err(|e| io("open runtime package", path, e))?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        check(cancel)?;
        let n = input
            .read(&mut bytes)
            .map_err(|e| io("hash runtime package", path, e))?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn file(
    entry: &mut dyn Read,
    path: &Path,
    spec: &FileEntry,
    cancel: &CancellationToken,
) -> Result<()> {
    if !digest(&spec.sha256) || spec.size > EXTRACT_LIMIT {
        return Err(invalid("Invalid runtime file manifest"));
    }
    fs::create_dir_all(path.parent().expect("joined runtime file"))
        .map_err(|e| io("create runtime directory", path, e))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|e| io("create runtime file", path, e))?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        check(cancel)?;
        let n = entry
            .read(&mut bytes)
            .map_err(|_| invalid("Invalid or truncated runtime ZIP entry"))?;
        if n == 0 {
            break;
        }
        count = count.saturating_add(n as u64);
        if count > spec.size {
            return Err(invalid("Runtime file exceeds its declared size"));
        }
        hash.update(&bytes[..n]);
        output
            .write_all(&bytes[..n])
            .map_err(|e| io("extract runtime file", path, e))?;
    }
    if count != spec.size || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&spec.sha256) {
        return Err(invalid(
            "Runtime file hash/size does not match its manifest",
        ));
    }
    output
        .sync_all()
        .map_err(|e| io("sync runtime file", path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if spec.executable { 0o700 } else { 0o600 }),
        )
        .map_err(|e| io("set runtime executable permissions", path, e))?;
    }
    Ok(())
}
pub(super) fn extract(
    package: &Path,
    image: &Path,
    target: &str,
    cancel: &CancellationToken,
    sender: &watch::Sender<Progress>,
) -> Result<Manifest> {
    let input = File::open(package).map_err(|e| io("open runtime ZIP", package, e))?;
    let mut archive =
        zip::ZipArchive::new(input).map_err(|_| invalid("Invalid runtime ZIP package"))?;
    if archive.len() > FILE_LIMIT + 1 {
        return Err(invalid("Runtime archive exceeds its file limit"));
    }
    let manifest = {
        let entry = archive
            .by_name("runtime-manifest.json")
            .map_err(|_| invalid("Runtime ZIP is missing its manifest"))?;
        if entry.size() > 2 * 1024 * 1024 {
            return Err(invalid("Runtime manifest is too large"));
        }
        let mut bytes = vec![];
        entry
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("Cannot read runtime manifest"))?;
        serde_json::from_slice::<Manifest>(&bytes)
            .map_err(|_| invalid("Invalid runtime manifest"))?
    };
    let java = format!("bin/{}", super::java_name());
    if manifest.schema_version != 1
        || manifest.target != target
        || manifest.java_major < 21
        || manifest.java_version.len() > 256
        || manifest.files.len() > FILE_LIMIT
        || !manifest.files.get(&java).is_some_and(|f| f.executable)
        || !manifest.files.contains_key("release")
        || !manifest.files.contains_key("lib/modules")
        || !manifest.files.keys().any(|name| name.starts_with("legal/"))
        || ["java.base", "java.sql", "java.naming", "jdk.crypto.ec"]
            .iter()
            .any(|name| !manifest.modules.iter().any(|module| module == name))
    {
        return Err(invalid(
            "Runtime manifest lacks the required platform, Java, JDBC, TLS or license contents",
        ));
    }
    let total = manifest
        .files
        .values()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .filter(|n| *n <= EXTRACT_LIMIT)
        .ok_or_else(|| invalid("Runtime extraction exceeds 512 MiB"))?;
    for name in manifest.files.keys() {
        if name.ends_with('/') {
            return Err(invalid("Runtime file manifest contains a directory entry"));
        }
        relative(name)?;
    }
    fs::create_dir(image).map_err(|e| io("create staged runtime image", image, e))?;
    let mut seen = BTreeSet::new();
    let mut completed = 0;
    for index in 0..archive.len() {
        check(cancel)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|_| invalid("Invalid runtime ZIP entry"))?;
        let name = entry
            .name()
            .map_err(|_| invalid("Invalid UTF-8 runtime ZIP path"))?
            .into_owned();
        relative(name.trim_end_matches('/'))?;
        if !seen.insert(if cfg!(windows) {
            name.to_ascii_lowercase()
        } else {
            name.clone()
        }) {
            return Err(invalid("Duplicate runtime ZIP path"));
        }
        let mode = entry.unix_mode().unwrap_or(0) & 0o170000;
        if entry.is_symlink() || !matches!(mode, 0 | 0o100000 | 0o040000) {
            return Err(invalid(
                "Runtime ZIP must contain only regular files and directories",
            ));
        }
        if name == "runtime-manifest.json" {
            if entry.is_dir() {
                return Err(invalid("Invalid runtime manifest entry"));
            }
            continue;
        }
        if entry.is_dir() {
            if name != "runtime/" && !name.starts_with("runtime/") {
                return Err(invalid("Unexpected runtime archive directory"));
            }
            continue;
        }
        let name = name
            .strip_prefix("runtime/")
            .ok_or_else(|| invalid("Runtime ZIP contains unexpected application/driver files"))?;
        let spec = manifest
            .files
            .get(name)
            .ok_or_else(|| invalid("Runtime ZIP contains an undeclared file"))?;
        if entry.size() != spec.size {
            return Err(invalid("Runtime ZIP file size does not match its manifest"));
        }
        file(
            &mut entry,
            &image.join("runtime").join(relative(name)?),
            spec,
            cancel,
        )?;
        completed += spec.size;
        progress(sender, "Extracting", completed, Some(total));
    }
    if completed != total
        || manifest.files.keys().any(|name| {
            !seen.contains(&if cfg!(windows) {
                format!("runtime/{name}").to_ascii_lowercase()
            } else {
                format!("runtime/{name}")
            })
        })
    {
        return Err(invalid("Runtime ZIP is missing declared files"));
    }
    Ok(manifest)
}
