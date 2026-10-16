//! Direct Java launch avoids Python/batch wrappers and keeps install templates immutable.
use crate::{
    catalog::{Platform, invalid, relative_path},
    discovery::ResolvedCommand,
};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
const DATA: &str = "{kodeWorkspaceData}";
pub fn installed_command(root: &Path, platform: &Platform) -> Result<Vec<String>> {
    let config = match (platform.os.as_str(), platform.arch.as_str()) {
        ("linux", "arm64") => "config_linux_arm",
        ("linux", _) => "config_linux",
        ("macos", "arm64") => "config_mac_arm",
        ("macos", _) => "config_mac",
        ("windows", _) => "config_win",
        _ => return Err(invalid("JDT LS package has no configuration for this OS")),
    };
    if !root.join(config).join("config.ini").is_file() {
        return Err(invalid(
            "JDT LS package is missing its platform configuration",
        ));
    }
    let plugins = root.join("plugins");
    let mut jars = fs::read_dir(&plugins)
        .map_err(|error| io("list JDT LS plugins", &plugins, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| io("read JDT LS plugin", &plugins, error))?;
    jars.retain(|path| {
        path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("org.eclipse.equinox.launcher_") && name.ends_with(".jar")
                })
    });
    if jars.len() != 1 {
        return Err(invalid(
            "JDT LS package must contain exactly one Equinox launcher JAR",
        ));
    }
    let jar = jars
        .pop()
        .expect("one launcher")
        .strip_prefix(root)
        .expect("package plugin")
        .to_str()
        .ok_or_else(|| invalid("JDT LS launcher path is not UTF-8"))?
        .replace('\\', "/");
    Ok(vec![
        "java".into(),
        "-Declipse.application=org.eclipse.jdt.ls.core.id1".into(),
        "-Dosgi.bundles.defaultStartLevel=4".into(),
        "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
        "-Djdk.xml.maxGeneralEntitySizeLimit=0".into(),
        "-Djdk.xml.totalEntitySizeLimit=0".into(),
        "-Xms100m".into(),
        "-Xmx1G".into(),
        "--add-modules=ALL-SYSTEM".into(),
        "--add-opens=java.base/java.util=ALL-UNNAMED".into(),
        "--add-opens=java.base/java.lang=ALL-UNNAMED".into(),
        "-jar".into(),
        jar,
        "-configuration".into(),
        config.into(),
        "-data".into(),
        DATA.into(),
    ])
}
fn io(operation: &'static str, path: &Path, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
fn directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io("create JDT LS workspace directory", path, error)),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io("inspect JDT LS workspace directory", path, error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(
            "JDT LS workspace cache must be a regular directory",
        ));
    }
    Ok(())
}
fn copy_config(source: &Path, target: &Path, cancel: &CancellationToken) -> Result<()> {
    let mut work = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut files = 0;
    let mut bytes = 0u64;
    while let Some((source, target)) = work.pop() {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        directory(&target)?;
        for entry in fs::read_dir(&source)
            .map_err(|error| io("read JDT LS configuration", &source, error))?
        {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let entry =
                entry.map_err(|error| io("read JDT LS configuration entry", &source, error))?;
            let path = entry.path();
            let target = target.join(entry.file_name());
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| io("inspect JDT LS configuration", &path, error))?;
            files += 1;
            bytes = bytes.saturating_add(metadata.len());
            if files > 10000 || bytes > 32 * 1024 * 1024 {
                return Err(invalid("JDT LS configuration exceeds copy limits"));
            }
            if metadata.is_dir() {
                work.push((path, target));
            } else if metadata.is_file() {
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                {
                    Ok(mut output) => {
                        let mut input = fs::File::open(&path)
                            .map_err(|error| io("read JDT LS configuration", &path, error))?;
                        std::io::copy(&mut input, &mut output)
                            .map_err(|error| io("copy JDT LS configuration", &target, error))?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&target).map_err(|error| {
                            io("inspect existing JDT LS configuration", &target, error)
                        })?;
                        if !metadata.is_file() || metadata.file_type().is_symlink() {
                            return Err(invalid(
                                "JDT LS configuration target is not a regular file",
                            ));
                        }
                    }
                    Err(error) => return Err(io("create JDT LS configuration", &target, error)),
                }
            } else {
                return Err(invalid(
                    "JDT LS configuration contains a link or special file",
                ));
            }
        }
    }
    Ok(())
}
pub fn workspace_command(
    mut command: ResolvedCommand,
    install_root: &Path,
    id: &str,
    workspace: &Path,
    revision: &str,
    cancel: &CancellationToken,
) -> Result<ResolvedCommand> {
    let base = command
        .cwd
        .as_ref()
        .ok_or_else(|| invalid("JDT LS command needs its installed directory"))?;
    let position = command
        .args
        .iter()
        .position(|arg| arg == "-configuration")
        .ok_or_else(|| invalid("JDT LS launcher has no configuration argument"))?
        + 1;
    let configuration = command
        .args
        .get(position)
        .ok_or_else(|| invalid("JDT LS launcher has no configuration path"))?;
    let source = base.join(relative_path(configuration)?);
    let text = workspace
        .to_str()
        .ok_or_else(|| invalid("JDT LS workspace path is not UTF-8"))?;
    let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
    let mut cache = PathBuf::from(install_root);
    for component in [".workspaces", &digest, id] {
        cache.push(component);
        directory(&cache)?;
    }
    // OSGi configuration contains bundle versions. An upgraded server must get
    // its new template, while project data survives an upgrade.
    let mut identity = Sha256::new();
    identity.update(text.as_bytes());
    identity.update([0]);
    identity.update(revision.as_bytes());
    let config_digest = format!("{:x}", identity.finalize());
    let mut config = PathBuf::from(install_root);
    for component in [".configurations", &config_digest, id] {
        config.push(component);
        directory(&config)?;
    }
    copy_config(&source, &config, cancel)?;
    command.args[position] = config
        .to_str()
        .ok_or_else(|| invalid("JDT LS cache path is not UTF-8"))?
        .into();
    let data = cache.join("data");
    directory(&data)?;
    for arg in &mut command.args {
        if arg == DATA {
            *arg = data
                .to_str()
                .ok_or_else(|| invalid("JDT LS data path is not UTF-8"))?
                .into();
        }
    }
    Ok(command)
}
