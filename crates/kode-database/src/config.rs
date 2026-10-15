use crate::{
    model::{DataSourceDefinition, Extra},
    security,
};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, RequestId, Result, Revision,
    WorkspaceGeneration,
    document::encoding::FileFormat,
    intelligence::DocumentSnapshot,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::{files::NativeDocumentIo, paths::ResourcePaths};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
pub(crate) fn failure(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "database configuration",
        message: message.into(),
    }
}
pub(crate) fn io(path: &Path, operation: &'static str, source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation,
        path: path.into(),
        source,
    }
}
#[derive(Debug, Clone)]
pub struct DatabasePaths {
    pub root: PathBuf,
    pub legacy_root: Option<PathBuf>,
    pub plugin_root: PathBuf,
    pub legacy_plugin_root: Option<PathBuf>,
}
impl DatabasePaths {
    /// Path discovery only. Import and all filesystem reads are explicit jobs.
    pub fn from_environment(resources: &ResourcePaths) -> Self {
        let env = |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let legacy_config = env("KODE_CONFIG_HOME")
            .or_else(|| env("XDG_CONFIG_HOME").map(|path| path.join("kode")))
            .or_else(|| {
                if cfg!(windows) {
                    env("APPDATA").map(|path| path.join("Kode"))
                } else {
                    None
                }
            })
            .or_else(|| {
                env("HOME")
                    .or_else(|| env("USERPROFILE"))
                    .map(|path| path.join(".config/kode"))
            });
        Self {
            root: env("KODE_RS_DATABASE_HOME")
                .unwrap_or_else(|| resources.config_dir.join("database")),
            plugin_root: env("KODE_RS_JDBC_HOME")
                .or_else(|| env("KODE_PLUGINS_HOME").map(|path| path.join("jdbc")))
                .unwrap_or_else(|| resources.config_dir.join("plugins/jdbc")),
            legacy_root: legacy_config.as_ref().map(|path| path.join("database")),
            legacy_plugin_root: env("KODE_PLUGINS_HOME")
                .or_else(|| legacy_config.map(|path| path.join("plugins")))
                .map(|path| path.join("jdbc")),
        }
    }
}
pub(crate) fn reject_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure(
                    "database configuration paths cannot traverse symlinks",
                ));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(io(ancestor, "inspect database configuration", error));
            }
            _ => {}
        }
    }
    Ok(())
}
pub(crate) fn ensure_root(root: &Path) -> Result<()> {
    reject_links(root)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(root)
        .map_err(|source| io(root, "create database configuration directory", source))?;
    Ok(())
}
pub(crate) struct ConfigLock {
    _file: fs::File,
}
impl ConfigLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        ensure_root(root)?;
        let path = root.join(".database-config.lock");
        reject_links(&path)?;
        if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
            return Err(failure("database lock target must be a regular file"));
        }
        let mut options = fs::OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|source| io(&path, "open database configuration lock", source))?;
        if !file
            .metadata()
            .map_err(|source| io(&path, "inspect database lock", source))?
            .is_file()
        {
            return Err(failure("database lock target is not regular"));
        }
        file.try_lock().map_err(|error| match error {
            fs::TryLockError::WouldBlock => failure(
                "database configuration is locked by another writer; retry after it finishes",
            ),
            fs::TryLockError::Error(source) => io(&path, "lock database configuration", source),
        })?;
        Ok(Self { _file: file })
    }
}
#[derive(Debug, Clone)]
pub(crate) struct JsonFile {
    pub(crate) path: PathBuf,
}
impl JsonFile {
    pub(crate) fn new(root: &Path, name: &str) -> Self {
        Self {
            path: root.join(name),
        }
    }
    pub(crate) fn read(
        &self,
        maximum: u64,
        private: bool,
    ) -> Result<Option<(Vec<u8>, DiskIdentity)>> {
        reject_links(&self.path)?;
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(&self.path, "stat database configuration", source)),
        };
        if !metadata.is_file() || metadata.len() > maximum {
            return Err(failure(
                "database configuration must be a bounded regular file",
            ));
        }
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(failure("credential file permissions must be owner-only"));
            }
        }
        #[cfg(not(unix))]
        let _ = private;
        let tag = JobTag {
            workspace: WorkspaceGeneration(0),
            request: RequestId::fresh()?,
            document: None,
        };
        let loaded = NativeDocumentIo.load(&DocumentOrigin::File(self.path.clone()), tag)?;
        if loaded.bytes.len() as u64 > maximum {
            return Err(failure("database configuration exceeds its read limit"));
        }
        Ok(Some((
            loaded.bytes,
            loaded
                .disk_identity
                .ok_or_else(|| failure("configuration load has no disk identity"))?,
        )))
    }
    pub(crate) fn write(&self, bytes: Vec<u8>, expected: Option<&DiskIdentity>) -> Result<()> {
        let root = self
            .path
            .parent()
            .ok_or_else(|| failure("configuration target has no parent"))?;
        ensure_root(root)?;
        reject_links(&self.path)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| failure("configuration serialization must be UTF-8"))?;
        let snapshot = DocumentSnapshot {
            id: DocumentId::fresh()?,
            revision: Revision(0),
            origin: DocumentOrigin::File(self.path.clone()),
            language: None,
            format: FileFormat::default(),
            text: text.into(),
        };
        NativeDocumentIo.save(&self.path, &snapshot, expected)?;
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataSourceStore {
    #[serde(default = "version")]
    version: u32,
    #[serde(default)]
    data_sources: Vec<DataSourceDefinition>,
    #[serde(default)]
    imported_from: Option<String>,
    #[serde(flatten, serialize_with = "security::serialize_extra")]
    extra: Extra,
}
fn version() -> u32 {
    1
}
impl Default for DataSourceStore {
    fn default() -> Self {
        Self {
            version: 1,
            data_sources: vec![],
            imported_from: None,
            extra: Extra::new(),
        }
    }
}
#[derive(Debug, Clone)]
pub struct DataSourceRepository {
    pub(crate) root: PathBuf,
    file: JsonFile,
}
pub mod recents;
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub already_present: bool,
    pub data_sources: usize,
    pub credentials: usize,
}
impl DataSourceRepository {
    pub fn new(root: PathBuf) -> Self {
        Self {
            file: JsonFile::new(&root, "datasources-v1.json"),
            root,
        }
    }
    fn read(&self) -> Result<(DataSourceStore, Option<DiskIdentity>)> {
        let Some((bytes, identity)) = self.file.read(4 * 1024 * 1024, false)? else {
            return Ok((DataSourceStore::default(), None));
        };
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| failure("malformed datasource store; original file retained"))?;
        security::validate_persistent_json(&value).map_err(failure)?;
        let store: DataSourceStore = serde_json::from_value(value)
            .map_err(|_| failure("invalid datasource definition; original file retained"))?;
        if store.version != 1 || store.data_sources.len() > 4096 {
            return Err(failure("unsupported datasource version or entry count"));
        }
        Ok((store, Some(identity)))
    }
    pub fn load(&self) -> Result<Vec<DataSourceDefinition>> {
        self.read().map(|(store, _)| store.data_sources)
    }
    fn publish(&self, mut store: DataSourceStore, expected: Option<&DiskIdentity>) -> Result<()> {
        if store.data_sources.len() > 4096 {
            return Err(failure("datasource count exceeds 4,096"));
        }
        store
            .data_sources
            .sort_by_key(|definition| definition.name.to_lowercase());
        let bytes = serde_json::to_vec_pretty(&store).map_err(|_| {
            failure("datasource export contains invalid or credential-bearing data")
        })?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(failure("datasource store exceeds 4 MiB"));
        }
        self.file.write(bytes, expected)
    }
    pub fn save(&self, definitions: Vec<DataSourceDefinition>) -> Result<()> {
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut store, identity) = self.read()?;
        store.data_sources = definitions;
        self.publish(store, identity.as_ref())
    }
    pub fn upsert(&self, definition: DataSourceDefinition) -> Result<Vec<DataSourceDefinition>> {
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut store, identity) = self.read()?;
        store.data_sources.retain(|old| old.id != definition.id);
        store.data_sources.push(definition);
        let result = store.data_sources.clone();
        self.publish(store, identity.as_ref())?;
        Ok(result)
    }
    pub fn remove(&self, id: &str) -> Result<Vec<DataSourceDefinition>> {
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut store, identity) = self.read()?;
        store.data_sources.retain(|definition| definition.id != id);
        let result = store.data_sources.clone();
        self.publish(store, identity.as_ref())?;
        Ok(result)
    }
    /// Explicit first import; existing Rust data wins and legacy files stay intact.
    pub fn import_legacy(
        &self,
        legacy_root: &Path,
        credentials: &crate::credentials::CredentialStore,
    ) -> Result<ImportReport> {
        if self.root == legacy_root
            || self
                .root
                .canonicalize()
                .ok()
                .zip(legacy_root.canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
        {
            return Err(failure(
                "Rust database configuration must be separate from legacy configuration",
            ));
        }
        if credentials.root != self.root {
            return Err(failure(
                "datasource and credential stores must share the Rust configuration root",
            ));
        }
        let _lock = ConfigLock::acquire(&self.root)?;
        let (existing, identity) = self.read()?;
        if identity.is_some() {
            return Ok(ImportReport {
                already_present: true,
                data_sources: existing.data_sources.len(),
                credentials: 0,
            });
        }
        let legacy = JsonFile::new(legacy_root, "datasources.json");
        let Some((bytes, _)) = legacy.read(4 * 1024 * 1024, false)? else {
            return Ok(ImportReport::default());
        };
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| failure("malformed legacy datasources; original file retained"))?;
        let secret_file = JsonFile::new(legacy_root, "credentials.properties");
        let mut private_values = if let Some((bytes, _)) = secret_file.read(1024 * 1024, false)? {
            crate::credentials::parse_java_properties(&bytes)?
        } else {
            Default::default()
        };
        if let Some(definitions) = value
            .get_mut("dataSources")
            .and_then(serde_json::Value::as_array_mut)
        {
            use sha2::{Digest, Sha256};
            for definition in definitions {
                let id = definition
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| failure("legacy datasource ID is missing"))?
                    .to_owned();
                let mut moved = Vec::new();
                if let Some(properties) = definition
                    .get_mut("properties")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    for key in properties
                        .keys()
                        .filter(|key| security::sensitive_key(key))
                        .cloned()
                        .collect::<Vec<_>>()
                    {
                        let secret = properties
                            .remove(&key)
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .ok_or_else(|| failure("legacy secret property must be text"))?;
                        let reference = format!(
                            "imported.property.{:x}",
                            Sha256::new()
                                .chain_update((id.len() as u64).to_be_bytes())
                                .chain_update(id.as_bytes())
                                .chain_update(key.as_bytes())
                                .finalize()
                        );
                        if private_values
                            .get(&reference)
                            .is_some_and(|old| old != &secret)
                        {
                            return Err(failure("legacy credential reference collision"));
                        }
                        private_values.insert(reference.clone(), secret);
                        moved.push((key, reference));
                    }
                }
                if !moved.is_empty() {
                    let object = definition
                        .as_object_mut()
                        .ok_or_else(|| failure("legacy datasource must be an object"))?;
                    let refs = object
                        .entry("credentialProperties")
                        .or_insert_with(|| serde_json::json!({}))
                        .as_object_mut()
                        .ok_or_else(|| failure("invalid credential property references"))?;
                    for (key, reference) in moved {
                        if refs
                            .insert(key, serde_json::Value::String(reference))
                            .is_some()
                        {
                            return Err(failure("duplicate legacy credential property reference"));
                        }
                    }
                }
            }
        }
        security::validate_persistent_json(&value).map_err(failure)?;
        let mut store: DataSourceStore = serde_json::from_value(value).map_err(|_| {
            failure("invalid legacy datasource or credential-bearing URL; original retained")
        })?;
        if store.version != 1 || store.data_sources.len() > 4096 {
            return Err(failure("unsupported legacy datasource version or count"));
        }
        store.imported_from = Some(legacy_root.to_string_lossy().into_owned());
        let count = store.data_sources.len();
        let exported = serde_json::to_vec_pretty(&store)
            .map_err(|_| failure("unsafe legacy datasource export"))?;
        if exported.len() > 4 * 1024 * 1024 {
            return Err(failure("legacy datasource export exceeds 4 MiB"));
        }
        // Private values publish first. A later failed public write can leave
        // only private orphan refs, never a definition with lost auth values.
        let imported = credentials.import_values(private_values)?;
        self.publish(store, None)?;
        Ok(ImportReport {
            already_present: false,
            data_sources: count,
            credentials: imported,
        })
    }
}
