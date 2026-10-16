use kode_core::{DomainError, Result};
use kode_platform::{files::open_regular_file, paths::ResourcePaths};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};
pub const MAX_CATALOG_BYTES: usize = 1024 * 1024;
pub type Extra = Map<String, Value>;
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub languages: Vec<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub preferred_command: Option<Vec<String>>,
    #[serde(default)]
    pub versions: Vec<ServerVersion>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerVersion {
    pub version: String,
    #[serde(default)]
    pub downloads: Vec<ServerDownload>,
    #[serde(default)]
    pub install_instructions: Extra,
    #[serde(flatten)]
    pub extra: Extra,
}
fn any() -> String {
    "any".into()
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerDownload {
    #[serde(default = "any")]
    pub os: String,
    #[serde(default = "any")]
    pub arch: String,
    pub url: String,
    pub sha256: Option<String>,
    pub unpack_subdir: Option<String>,
    pub command: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallManifest {
    pub id: String,
    pub version: String,
    pub installed_at: u64,
    pub source_url: Option<String>,
    pub platform_os: Option<String>,
    pub platform_arch: Option<String>,
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub unpack_subdir: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}
impl Platform {
    pub fn new(os: &str, arch: &str) -> Self {
        let os = os.trim().to_ascii_lowercase();
        let arch = arch.trim().to_ascii_lowercase();
        let os = if os.contains("darwin") || os.starts_with("mac") {
            "macos".to_owned()
        } else if os.starts_with("win") {
            "windows".to_owned()
        } else if os.contains("linux") {
            "linux".to_owned()
        } else {
            os
        };
        Self {
            os: match os.as_str() {
                "darwin" | "mac" | "mac os x" | "osx" => "macos",
                "win" | "win32" | "win64" => "windows",
                "gnu/linux" => "linux",
                _ => &os,
            }
            .into(),
            arch: match arch.as_str() {
                "amd64" | "x64" | "x86-64" => "x86_64",
                "aarch64" => "arm64",
                "i386" | "i486" | "i586" | "i686" => "x86",
                _ => &arch,
            }
            .into(),
        }
    }
    pub fn detect() -> Self {
        Self::new(std::env::consts::OS, std::env::consts::ARCH)
    }
    pub fn matches(&self, os: &str, arch: &str) -> bool {
        let candidate = Self::new(os, arch);
        (candidate.os == "any" || candidate.os == self.os)
            && (candidate.arch == "any" || candidate.arch == self.arch)
    }
}
pub fn invalid(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "LSP catalog/install",
        message: message.into(),
    }
}
pub fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || matches!(id, "." | "..")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(invalid("invalid server ID"));
    }
    Ok(())
}
pub fn validate_command(command: &[String]) -> Result<()> {
    if command.is_empty()
        || command.len() > 128
        || command[0].trim().is_empty()
        || command
            .iter()
            .any(|arg| arg.len() > 32_768 || arg.contains('\0'))
    {
        return Err(invalid("invalid server command array"));
    }
    Ok(())
}
/// Portable relative archive path: reject traversal, drive/UNC prefixes and links
/// regardless of the OS currently performing extraction.
pub fn relative_path(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 4096
        || name.contains(['\\', ':', '\0'])
        || name.starts_with('/')
    {
        return Err(invalid("unsafe relative package path"));
    }
    let parts: Vec<_> = name
        .split('/')
        .filter(|part| *part != "." && !part.is_empty())
        .collect();
    if parts.contains(&"..") {
        return Err(invalid("unsafe relative package path"));
    }
    Ok(parts.iter().collect())
}
impl Catalog {
    pub fn compiled() -> Result<Self> {
        Self::parse(include_bytes!("../../../resources/lsp/servers.json"))
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err(invalid("catalog exceeds 1 MiB"));
        }
        let catalog: Self = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
        if catalog.servers.len() > 1024 {
            return Err(invalid("catalog exceeds 1024 servers"));
        }
        let mut ids = BTreeSet::new();
        for server in &catalog.servers {
            validate_id(&server.id)?;
            if !ids.insert(&server.id) {
                return Err(invalid("duplicate server ID"));
            }
            if server.name.is_empty()
                || server.name.len() > 1024
                || server.languages.len() > 128
                || server.versions.len() > 1024
            {
                return Err(invalid("invalid server catalog bounds"));
            }
            if let Some(command) = &server.preferred_command {
                validate_command(command)?;
            }
            for version in &server.versions {
                if version.version.is_empty()
                    || version.version.len() > 256
                    || version.downloads.len() > 256
                {
                    return Err(invalid("invalid server version"));
                }
                for download in &version.downloads {
                    crate::package::plan(&download.extra)?;
                    let url =
                        reqwest::Url::parse(&download.url).map_err(|e| invalid(e.to_string()))?;
                    if !matches!(url.scheme(), "http" | "https")
                        || url.host_str().is_none()
                        || !url.username().is_empty()
                        || url.password().is_some()
                    {
                        return Err(invalid("expected HTTP(S) package URL without credentials"));
                    }
                    if let Some(hash) = &download.sha256
                        && (hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
                    {
                        return Err(invalid("invalid SHA-256 checksum"));
                    }
                    if let Some(subdir) = &download.unpack_subdir {
                        relative_path(subdir)?;
                    }
                    if let Some(command) = &download.command {
                        validate_command(command)?;
                    }
                }
            }
        }
        Ok(catalog)
    }
    pub fn load(paths: &ResourcePaths, explicit: Option<&Path>) -> Result<(Self, Option<PathBuf>)> {
        if let Some(path) = explicit {
            let mut bytes = vec![];
            open_regular_file(path)?
                .take(MAX_CATALOG_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| invalid(e.to_string()))?;
            return Ok((Self::parse(&bytes)?, Some(path.to_owned())));
        }
        let rust_override = paths.explicit.is_some() || paths.rust_home.is_some();
        for root in [
            paths.explicit.as_ref(),
            paths.rust_home.as_ref(),
            if rust_override {
                None
            } else {
                paths.legacy_home.as_ref()
            },
            Some(&paths.executable_dir),
        ]
        .into_iter()
        .flatten()
        {
            let path = root.join("lsp/servers.json");
            match std::fs::symlink_metadata(&path) {
                Ok(_) => return Self::load(paths, Some(&path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(DomainError::Io {
                        operation: "inspect LSP catalog",
                        path,
                        source,
                    });
                }
            }
        }
        Ok((Self::compiled()?, None))
    }
    pub fn entry(&self, id: &str) -> Option<&ServerEntry> {
        self.servers.iter().find(|entry| entry.id == id)
    }
    pub fn for_language(&self, language: &str) -> impl Iterator<Item = &ServerEntry> {
        self.servers.iter().filter(move |entry| {
            entry
                .languages
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(language))
        })
    }
}
impl ServerEntry {
    /// Catalog order is preference order; never sort arbitrary version labels.
    pub fn download(&self, platform: &Platform) -> Option<(&ServerVersion, &ServerDownload)> {
        self.versions.iter().find_map(|version| {
            version
                .downloads
                .iter()
                .find(|download| platform.matches(&download.os, &download.arch))
                .map(|download| (version, download))
        })
    }
}
