use super::{MAX_ARCHIVE_FILES, MAX_EXTRACTED_BYTES, check, io};
use crate::catalog::{Platform, ServerDownload, invalid, relative_path, validate_command};
use kode_core::Result;
use kode_platform::jobs::CancellationToken;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
const MAX_FILE: u64 = 512 * 1024 * 1024;
struct Extract<'a> {
    root: &'a Path,
    cancel: &'a CancellationToken,
    bytes: u64,
    files: usize,
    progress: &'a mut dyn FnMut(u64),
}
impl Extract<'_> {
    fn entry(
        &mut self,
        name: &str,
        directory: bool,
        mode: Option<u32>,
        size: Option<u64>,
        input: &mut dyn Read,
    ) -> Result<()> {
        check(self.cancel)?;
        self.files += 1;
        if self.files > MAX_ARCHIVE_FILES {
            return Err(invalid("package exceeds 100,000 entries"));
        }
        let relative = relative_path(name)?;
        if relative == Path::new("installed.json") {
            return Err(invalid("package contains reserved install manifest"));
        }
        let target = self.root.join(relative);
        if directory {
            fs::create_dir_all(&target)
                .map_err(|e| io("create extracted directory", &target, e))?;
            return Ok(());
        }
        if size.is_some_and(|size| size > MAX_FILE || size > MAX_EXTRACTED_BYTES - self.bytes) {
            return Err(invalid("package extraction size limit exceeded"));
        }
        fs::create_dir_all(target.parent().expect("joined package path"))
            .map_err(|e| io("create extracted parent", &target, e))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| io("create extracted file", &target, e))?;
        let mut buffer = [0; 64 * 1024];
        let mut written = 0u64;
        loop {
            check(self.cancel)?;
            let count = input
                .read(&mut buffer)
                .map_err(|e| io("read archive entry", &target, e))?;
            if count == 0 {
                break;
            }
            written = written.saturating_add(count as u64);
            self.bytes = self.bytes.saturating_add(count as u64);
            if written > MAX_FILE || self.bytes > MAX_EXTRACTED_BYTES {
                return Err(invalid("package extraction size limit exceeded"));
            }
            file.write_all(&buffer[..count])
                .map_err(|e| io("write extracted file", &target, e))?;
            (self.progress)(self.bytes);
        }
        if size.is_some_and(|size| size != written) {
            return Err(invalid("truncated archive entry"));
        }
        file.sync_all()
            .map_err(|e| io("sync extracted file", &target, e))?;
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))
                .map_err(|e| io("set package permissions", &target, e))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        Ok(())
    }
}
pub(super) struct Extraction {
    pub bytes: u64,
    pub files: usize,
}
pub(super) fn extract(
    package: &Path,
    root: &Path,
    download: &ServerDownload,
    command: Option<&[String]>,
    cancel: &CancellationToken,
    budget: &mut Extraction,
    progress: &mut dyn FnMut(u64),
) -> Result<()> {
    let url = reqwest::Url::parse(&download.url).map_err(|e| invalid(e.to_string()))?;
    let name = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("download URL has no package filename"))?;
    let lower = name.to_ascii_lowercase();
    let mut extract = Extract {
        root,
        cancel,
        bytes: budget.bytes,
        files: budget.files,
        progress,
    };
    if lower.ends_with(".zip") {
        let file = File::open(package).map_err(|e| io("open ZIP package", package, e))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| invalid(e.to_string()))?;
        if archive.len() > MAX_ARCHIVE_FILES {
            return Err(invalid("package entry limit exceeded"));
        }
        for index in 0..archive.len() {
            check(cancel)?;
            let mut entry = archive
                .by_index(index)
                .map_err(|e| invalid(e.to_string()))?;
            if entry.is_symlink() {
                return Err(invalid("package symlinks are unsupported"));
            }
            if entry.enclosed_name().is_none() {
                return Err(invalid("ZIP path escapes the install directory"));
            }
            let name = entry
                .name()
                .map_err(|e| invalid(e.to_string()))?
                .into_owned();
            let directory = entry.is_dir();
            let mode = entry.unix_mode();
            let size = entry.size();
            extract.entry(&name, directory, mode, Some(size), &mut entry)?;
        }
    } else if lower.ends_with(".tar") || lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        let mut tar_file = if lower.ends_with(".tar") {
            File::open(package).map_err(|e| io("open TAR package", package, e))?
        } else {
            let mut unpacked = tempfile::tempfile_in(package.parent().expect("staged package"))
                .map_err(|e| io("stage unpacked TAR", package, e))?;
            let mut decoder = flate2::read::GzDecoder::new(
                File::open(package).map_err(|e| io("open compressed TAR", package, e))?,
            );
            let mut buffer = [0; 64 * 1024];
            let mut bytes = 0u64;
            loop {
                check(cancel)?;
                let count = decoder
                    .read(&mut buffer)
                    .map_err(|e| invalid(e.to_string()))?;
                if count == 0 {
                    break;
                }
                bytes = bytes.saturating_add(count as u64);
                if bytes > MAX_EXTRACTED_BYTES {
                    return Err(invalid("unpacked TAR exceeds 1 GiB"));
                }
                unpacked
                    .write_all(&buffer[..count])
                    .map_err(|e| io("stage unpacked TAR", package, e))?;
            }
            unpacked
                .seek(SeekFrom::Start(0))
                .map_err(|e| io("rewind TAR", package, e))?;
            unpacked
        };
        // Preflight raw headers before tar-rs materializes GNU/PAX metadata.
        // This bounds extended-name memory without reimplementing TAR parsing.
        {
            let mut archive = tar::Archive::new(&mut tar_file);
            let entries = archive
                .entries_with_seek()
                .map_err(|e| invalid(e.to_string()))?
                .raw(true);
            for (index, entry) in entries.enumerate() {
                check(cancel)?;
                if index >= MAX_ARCHIVE_FILES {
                    return Err(invalid("TAR entry limit exceeded"));
                }
                let entry = entry.map_err(|e| invalid(e.to_string()))?;
                let kind = entry.header().entry_type();
                let metadata = kind.is_gnu_longname()
                    || kind.is_gnu_longlink()
                    || kind.is_pax_local_extensions()
                    || kind.is_pax_global_extensions();
                if metadata && entry.size() > 64 * 1024 {
                    return Err(invalid("TAR extended metadata exceeds 64 KiB"));
                }
                if !metadata && !kind.is_file() && !kind.is_dir() {
                    return Err(invalid(
                        "TAR links, devices and sparse entries are unsupported",
                    ));
                }
                if entry.size() > MAX_FILE {
                    return Err(invalid("TAR file exceeds 512 MiB"));
                }
            }
        }
        tar_file
            .seek(SeekFrom::Start(0))
            .map_err(|e| io("rewind validated TAR", package, e))?;
        let mut archive = tar::Archive::new(tar_file);
        for entry in archive.entries().map_err(|e| invalid(e.to_string()))? {
            check(cancel)?;
            let mut entry = entry.map_err(|e| invalid(e.to_string()))?;
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions() {
                continue;
            }
            if !kind.is_file() && !kind.is_dir() {
                return Err(invalid("unsupported TAR entry"));
            }
            let name = String::from_utf8(entry.path_bytes().into_owned())
                .map_err(|e| invalid(e.to_string()))?;
            let size = entry.size();
            let mode = entry.header().mode().map_err(|e| invalid(e.to_string()))?;
            extract.entry(&name, kind.is_dir(), Some(mode), Some(size), &mut entry)?;
        }
    } else if lower.ends_with(".gz") {
        let name = command
            .and_then(|c| c.first())
            .filter(|name| !Path::new(name).is_absolute())
            .map_or_else(|| name[..name.len() - 3].to_owned(), Clone::clone);
        let mut decoder = flate2::read::GzDecoder::new(
            File::open(package).map_err(|e| io("open GZIP binary", package, e))?,
        );
        extract.entry(&name, false, None, None, &mut decoder)?;
    } else {
        let mut file = File::open(package).map_err(|e| io("open raw package", package, e))?;
        extract.entry(name, false, None, None, &mut file)?;
    }
    budget.bytes = extract.bytes;
    budget.files = extract.files;
    Ok(())
}
fn executable(path: &Path) -> Result<()> {
    if !fs::metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Err(invalid("package command is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).map_err(|e| io("stat installed command", path, e))?;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode((metadata.permissions().mode() & 0o777) | 0o111),
        )
        .map_err(|e| io("mark installed command executable", path, e))?;
    }
    Ok(())
}
pub(super) fn installed_command(
    root: &Path,
    subdir: Option<&str>,
    command: Option<Vec<String>>,
    platform: &Platform,
) -> Result<Option<Vec<String>>> {
    let Some(mut command) = command else {
        return Err(invalid(
            "No package command configured; set preferredCommand or download.command",
        ));
    };
    validate_command(&command)?;
    let base = if let Some(subdir) = subdir {
        root.join(relative_path(subdir)?)
    } else {
        root.to_owned()
    };
    if !base.is_dir() {
        return Err(invalid("package unpackSubdir does not exist"));
    }
    let first = Path::new(&command[0]);
    // Explicit absolute commands and interpreters can live outside the package.
    if first.is_absolute() {
        return Ok(Some(command));
    }
    let relative = relative_path(&command[0])?;
    let mut candidates = vec![base.join(&relative)];
    if platform.os == "windows" && relative.extension().is_none() {
        candidates.extend(
            ["exe", "cmd", "bat"]
                .iter()
                .map(|extension| base.join(&relative).with_extension(extension)),
        );
    }
    if relative.components().count() == 1 {
        for folder in ["bin", "server/bin", "libexec"] {
            candidates.push(base.join(folder).join(&relative));
            if platform.os == "windows" && relative.extension().is_none() {
                candidates.extend(
                    ["exe", "cmd", "bat"].iter().map(|extension| {
                        base.join(folder).join(&relative).with_extension(extension)
                    }),
                );
            }
        }
    }
    if let Some(found) = candidates.into_iter().find(|path| path.is_file()) {
        executable(&found)?;
        command[0] = found
            .strip_prefix(&base)
            .expect("package candidate below base")
            .to_str()
            .ok_or_else(|| invalid("non UTF-8 installed command"))?
            .replace('\\', "/");
    } else if relative.components().count() > 1
        || !command[1..]
            .iter()
            .any(|arg| relative_path(arg).is_ok_and(|path| base.join(path).is_file()))
    {
        return Err(invalid(format!("package command {} not found", command[0])));
    }
    Ok(Some(command))
}
