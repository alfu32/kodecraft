use kode_core::{DomainError, Result};
use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct ResourcePaths {
    pub explicit: Option<PathBuf>,
    pub rust_home: Option<PathBuf>,
    pub legacy_home: Option<PathBuf>,
    pub executable_dir: PathBuf,
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceOrigin {
    File(PathBuf),
    Compiled,
}
#[derive(Debug, Clone)]
pub struct Resource {
    pub bytes: Vec<u8>,
    pub origin: ResourceOrigin,
}

impl ResourcePaths {
    pub fn from_environment(explicit: Option<PathBuf>) -> Result<Self> {
        let executable = env::current_exe().map_err(|source| DomainError::Io {
            operation: "resolve executable",
            path: PathBuf::new(),
            source,
        })?;
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        let base = if cfg!(target_os = "windows") {
            env::var_os("APPDATA").map(PathBuf::from)
        } else if cfg!(target_os = "macos") {
            home.as_ref()
                .map(|path| path.join("Library/Application Support"))
        } else {
            env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|path| path.join(".config")))
        }
        .ok_or_else(|| DomainError::MissingResource("user configuration directory".into()))?;
        Ok(Self {
            explicit,
            rust_home: env::var_os("KODE_RS_HOME").map(PathBuf::from),
            legacy_home: env::var_os("KODE_HOME").map(PathBuf::from),
            executable_dir: executable.parent().unwrap_or(Path::new("/")).to_path_buf(),
            config_dir: base.join("kode-rs"),
        })
    }
    pub fn resolve(&self, name: &str, defaults: &HashMap<&str, &[u8]>) -> Result<Resource> {
        let relative = Path::new(name);
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(DomainError::MissingResource(format!(
                "invalid resource path: {name}"
            )));
        }
        let rust_override = self.explicit.is_some() || self.rust_home.is_some();
        let roots = [
            self.explicit.as_ref(),
            self.rust_home.as_ref(),
            if rust_override {
                None
            } else {
                self.legacy_home.as_ref()
            },
            Some(&self.executable_dir),
        ];
        for root in roots.into_iter().flatten() {
            let path = root.join(relative);
            match std::fs::read(&path) {
                Ok(bytes) => {
                    return Ok(Resource {
                        bytes,
                        origin: ResourceOrigin::File(path),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(DomainError::Io {
                        operation: "read resource",
                        path,
                        source,
                    });
                }
            }
        }
        defaults
            .get(name)
            .map(|bytes| Resource {
                bytes: bytes.to_vec(),
                origin: ResourceOrigin::Compiled,
            })
            .ok_or_else(|| DomainError::MissingResource(name.into()))
    }
}

pub fn compiled_resources() -> HashMap<&'static str, &'static [u8]> {
    HashMap::from([
        (
            "mime-types.json",
            include_bytes!("../../../resources/mime-types.json").as_slice(),
        ),
        (
            "styles/app.css",
            include_bytes!("../../../resources/styles/app.css").as_slice(),
        ),
        (
            "keyword-patterns.txt",
            include_bytes!("../../../resources/keyword-patterns.txt").as_slice(),
        ),
        (
            "token-colors.txt",
            include_bytes!("../../../resources/token-colors.txt").as_slice(),
        ),
        (
            "codeintel/definitions.json",
            include_bytes!("../../../resources/codeintel/definitions.json").as_slice(),
        ),
        (
            "lsp/servers.json",
            include_bytes!("../../../resources/lsp/servers.json").as_slice(),
        ),
        (
            "database-jdbc-url-templates.json",
            include_bytes!("../../../resources/database-jdbc-url-templates.json").as_slice(),
        ),
    ])
}
