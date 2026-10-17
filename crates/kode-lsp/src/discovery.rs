//! Worker-only catalog/manifest/filesystem discovery. Drawing consumes statuses.
use crate::catalog::{
    Catalog, InstallManifest, MAX_MANIFEST_BYTES, Platform, ServerEntry, invalid, relative_path,
    validate_command, validate_id,
};
use kode_core::{DomainError, Result};
use kode_platform::{
    files::open_regular_file,
    paths::ResourcePaths,
    process::{ProcessFactory, ProcessSpec, SystemProcessFactory},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InstallState {
    Installed,
    Missing,
    UpdateAvailable,
    Manual,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}
impl ResolvedCommand {
    pub fn process_spec(&self) -> ProcessSpec {
        ProcessSpec {
            program: self.args[0].clone().into(),
            args: self.args[1..].to_vec(),
            cwd: self.cwd.clone(),
            env: vec![],
        }
    }
}
#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub entry: ServerEntry,
    pub install_state: InstallState,
    pub installed_version: Option<String>,
    pub install_path: PathBuf,
    pub command: Option<ResolvedCommand>,
    pub message: Option<String>,
    /// Supplied by P02's actual process registry. Discovery never invents running state.
    pub running: bool,
}
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    pub platform: Platform,
    pub install_root: PathBuf,
    pub search_path: Vec<PathBuf>,
    pub configured_commands: BTreeMap<String, Vec<String>>,
}
impl DiscoveryOptions {
    pub fn from_environment(paths: &ResourcePaths, lsp_home: Option<OsString>) -> Self {
        let install_root = lsp_home
            .map(PathBuf::from)
            .or_else(|| paths.rust_home.as_ref().map(|root| root.join("lsp")))
            .or_else(|| paths.legacy_home.as_ref().map(|root| root.join("lsp")))
            .unwrap_or_else(|| paths.config_dir.join("lsp"));
        let mut search_path = std::env::var_os("PATH").map_or(vec![], |path| {
            std::env::split_paths(&path)
                .filter(|path| !path.as_os_str().is_empty())
                .collect::<Vec<_>>()
        });
        if let Some(java_home) = std::env::var_os("JAVA_HOME") {
            search_path.insert(0, PathBuf::from(java_home).join("bin"));
        }
        Self {
            platform: Platform::detect(),
            install_root,
            search_path,
            configured_commands: BTreeMap::new(),
        }
    }
    pub fn directory(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.install_root.join(id))
    }
    pub fn read_manifest(&self, id: &str) -> Result<Option<InstallManifest>> {
        let directory = self.directory(id)?;
        if let Ok(metadata) = fs::symlink_metadata(&directory)
            && (metadata.file_type().is_symlink() || !metadata.is_dir())
        {
            return Err(invalid("server install path must be a regular directory"));
        }
        let path = directory.join("installed.json");
        let mut bytes = vec![];
        let file = match open_regular_file(&path) {
            Ok(file) => file,
            Err(DomainError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        file.take(MAX_MANIFEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid(e.to_string()))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("install manifest exceeds 128 KiB"));
        }
        let manifest: InstallManifest =
            serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
        if manifest.id != id {
            return Err(invalid("installed manifest server ID mismatch"));
        }
        if let Some(command) = &manifest.command {
            validate_command(command)?;
        }
        if let Some(subdir) = &manifest.unpack_subdir {
            relative_path(subdir)?;
        }
        Ok(Some(manifest))
    }
    fn executable(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if self.platform.os != "windows" && metadata.permissions().mode() & 0o111 == 0 {
                return false;
            }
        }
        true
    }
    /// Resolve the external runtime without reading or creating server configuration.
    pub fn runtime_program(
        &self,
        requirement: &crate::package::RuntimeRequirement,
    ) -> Result<PathBuf> {
        let name = requirement.kind.program();
        for directory in &self.search_path {
            let path = directory.join(if self.platform.os == "windows" {
                format!("{name}.exe")
            } else {
                name.into()
            });
            if self.executable(&path) {
                return Ok(path);
            }
        }
        Err(invalid(format!(
            "{} {} or newer is required; {} was not found on PATH. Install the runtime or configure a local server executable.",
            requirement.kind.label(),
            requirement.minimum,
            name
        )))
    }
    /// Start/restart preparation runs on a worker. Explicit user commands retain precedence.
    pub fn resolve_for_launch(
        &self,
        entry: &ServerEntry,
        manifest: Option<&InstallManifest>,
        workspace: &Path,
        cancel: &kode_platform::jobs::CancellationToken,
    ) -> Result<ResolvedCommand> {
        self.resolve_for_launch_with_process_factory(
            entry,
            manifest,
            workspace,
            cancel,
            &SystemProcessFactory,
        )
    }
    /// Native qualification may prefix an emulator while retaining production preparation.
    pub fn resolve_for_launch_with_process_factory(
        &self,
        entry: &ServerEntry,
        manifest: Option<&InstallManifest>,
        workspace: &Path,
        cancel: &kode_platform::jobs::CancellationToken,
        factory: &dyn ProcessFactory,
    ) -> Result<ResolvedCommand> {
        let command = self.resolve_command(entry, manifest)?;
        if !self.configured_commands.contains_key(&entry.id)
            && let Some(manifest) = manifest
            && manifest
                .extra
                .get(crate::package::COMMAND_KEY)
                .is_none_or(|generated| generated == &serde_json::json!(manifest.command))
            && let Some(plan) = crate::package::plan(&manifest.extra)?
        {
            if let Some(requirement) = plan.runtime {
                crate::runtime::verify_with_factory(
                    Path::new(&command.args[0]),
                    &requirement,
                    cancel,
                    factory,
                )?;
            }
            if plan.launcher == Some(crate::package::Launcher::Gnuelf) {
                crate::gnuelf::verify(&command, cancel, factory)?;
                return crate::gnuelf::prepare(command);
            }
            if plan.launcher == Some(crate::package::Launcher::Jdtls) {
                return crate::jdtls::workspace_command(
                    command,
                    &self.install_root,
                    &entry.id,
                    workspace,
                    manifest
                        .extra
                        .get("archiveSha256")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&manifest.version),
                    cancel,
                );
            }
        }
        Ok(command)
    }
    pub fn resolve_command(
        &self,
        entry: &ServerEntry,
        manifest: Option<&InstallManifest>,
    ) -> Result<ResolvedCommand> {
        let command = self
            .configured_commands
            .get(&entry.id)
            .or_else(|| manifest.and_then(|m| m.command.as_ref()))
            .or(entry.preferred_command.as_ref())
            .ok_or_else(|| invalid(format!("No command configured for {}", entry.name)))?;
        validate_command(command)?;
        let install_dir = self.directory(&entry.id)?;
        let mut bases = vec![];
        if let Some(subdir) = manifest.and_then(|m| m.unpack_subdir.as_deref()) {
            bases.push(install_dir.join(relative_path(subdir)?));
        }
        bases.push(install_dir.clone());
        bases.push(self.install_root.clone());
        bases.extend(self.search_path.iter().cloned());
        let executable = Path::new(&command[0]);
        let candidates = if executable.is_absolute() {
            vec![executable.to_owned()]
        } else {
            bases.iter().map(|base| base.join(executable)).collect()
        };
        let mut found = None;
        for candidate in candidates {
            let extensions = if self.platform.os == "windows" && candidate.extension().is_none() {
                vec!["", "exe", "cmd", "bat"]
            } else {
                vec![""]
            };
            for extension in extensions {
                let path = if extension.is_empty() {
                    candidate.clone()
                } else {
                    candidate.with_extension(extension)
                };
                if self.executable(&path) {
                    found = Some(path);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let executable = found.ok_or_else(|| {
            invalid(format!(
                "Executable {} not found; configure {}",
                command[0],
                install_dir.join("installed.json").display()
            ))
        })?;
        // Keep argv[0]'s filename for rustup and other symlink multiplexers.
        let executable = std::path::absolute(&executable).map_err(|e| invalid(e.to_string()))?;
        let mut args = vec![
            executable
                .to_str()
                .ok_or_else(|| invalid("server executable path is not UTF-8"))?
                .to_owned(),
        ];
        args.extend_from_slice(&command[1..]);
        let cwd = if install_dir.is_dir() {
            Some(
                if let Some(subdir) = manifest.and_then(|m| m.unpack_subdir.as_deref()) {
                    install_dir.join(relative_path(subdir)?)
                } else {
                    install_dir
                },
            )
        } else if self.install_root.is_dir() {
            Some(self.install_root.clone())
        } else {
            None
        };
        Ok(ResolvedCommand { args, cwd })
    }
    pub fn status(&self, entry: &ServerEntry, running: bool) -> ServerStatus {
        let install_path = self.install_root.join(&entry.id);
        let result: Result<_> = (|| {
            validate_id(&entry.id)?;
            let manifest = self.read_manifest(&entry.id)?;
            let resolved = self.resolve_command(entry, manifest.as_ref());
            let installed_version = manifest.as_ref().map(|m| m.version.clone());
            let latest = entry.versions.first().map(|version| &version.version);
            let state = if let Some(installed) = &installed_version {
                if latest.is_some_and(|latest| latest != installed) {
                    InstallState::UpdateAvailable
                } else {
                    InstallState::Installed
                }
            } else if install_path.is_dir() {
                InstallState::Manual
            } else if resolved.is_ok() {
                InstallState::Installed
            } else if entry.download(&self.platform).is_none() && !entry.versions.is_empty() {
                InstallState::Manual
            } else {
                InstallState::Missing
            };
            let (command, message) = match resolved {
                Ok(command) => (
                    Some(command),
                    if manifest.is_none() {
                        Some("Existing local executable; no download needed".into())
                    } else {
                        None
                    },
                ),
                Err(error) => (None, Some(error.to_string())),
            };
            let state = if manifest.is_some() && command.is_none() {
                InstallState::Error
            } else {
                state
            };
            Ok((state, installed_version, command, message))
        })();
        match result {
            Ok((install_state, installed_version, command, message)) => ServerStatus {
                entry: entry.clone(),
                install_state,
                installed_version,
                install_path,
                command,
                message,
                running,
            },
            Err(error) => ServerStatus {
                entry: entry.clone(),
                install_state: InstallState::Error,
                installed_version: None,
                install_path,
                command: None,
                message: Some(error.to_string()),
                running,
            },
        }
    }
    pub fn statuses(&self, catalog: &Catalog, running: impl Fn(&str) -> bool) -> Vec<ServerStatus> {
        catalog
            .servers
            .iter()
            .map(|entry| self.status(entry, running(&entry.id)))
            .collect()
    }
}
