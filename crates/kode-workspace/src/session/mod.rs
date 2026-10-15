//! Versioned user session data. Unknown feature fields remain owned JSON.
pub mod legacy;
pub mod persistence;
pub mod restore;

use kode_core::{
    DocumentId, DocumentOrigin, DomainError, Result,
    document::{Document, encoding::FileFormat},
    editor::{history::HistoryStateV1, selection::CaretState},
    services::DiskIdentity,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Component, Path, PathBuf};

pub const SESSION_VERSION: u32 = 1;
fn version() -> u32 {
    SESSION_VERSION
}
fn empty_history() -> HistoryStateV1 {
    Document::new(
        DocumentId(0),
        DocumentOrigin::Virtual {
            owner: "session".into(),
            key: "empty".into(),
            label: String::new(),
        },
        None,
        "",
    )
    .export_history(&CaretState::default())
}
fn database() -> Value {
    json!({"dataSources":[],"sessions":[]})
}
fn rest_api() -> Value {
    json!({"version":1,"collection":{"info":{"name":"REST API","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[]},"ui":{"selectedPath":null,"responsePanelHeight":12,"prettyPrintResponses":true,"environmentPanelHeight":8},"environments":[],"activeEnvironmentId":null})
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionV1 {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub recent_files: Vec<RecentFileV1>,
    #[serde(default)]
    pub open_editors: Vec<EditorStateV1>,
    #[serde(default)]
    pub source_roots: Vec<PathBuf>,
    #[serde(default)]
    pub scan_exclusions: Vec<PathBuf>,
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
    #[serde(default = "database")]
    pub database: Value,
    #[serde(default = "rest_api")]
    pub rest_api: Value,
    #[serde(default)]
    pub active_origin: Option<DocumentOrigin>,
    #[serde(default)]
    pub imported_from: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl Default for ProjectSessionV1 {
    fn default() -> Self {
        Self {
            version: SESSION_VERSION,
            recent_files: vec![],
            open_editors: vec![],
            source_roots: vec![],
            scan_exclusions: vec![],
            ignore_patterns: vec![],
            database: database(),
            rest_api: rest_api(),
            active_origin: None,
            imported_from: None,
            extra: Map::new(),
        }
    }
}
impl ProjectSessionV1 {
    pub fn validate_version(&self) -> Result<()> {
        if self.version != SESSION_VERSION {
            return Err(DomainError::Unsupported(format!(
                "Rust session version {}; supported version is {}",
                self.version, SESSION_VERSION
            )));
        }
        Ok(())
    }
    pub fn project_config(&self) -> crate::project::ProjectConfig {
        crate::project::ProjectConfig {
            source_roots: self.source_roots.clone(),
            exclusions: self.scan_exclusions.clone(),
            ignore_patterns: self.ignore_patterns.clone(),
        }
    }
    pub fn reconcile_move(&mut self, root: &Path, from: &Path, to: &Path) -> Result<()> {
        let retarget = |origin: &mut DocumentOrigin| -> Result<()> {
            if let DocumentOrigin::File(path) = origin {
                let absolute = resolve_path(root, path)?;
                if let Some(moved) = crate::tree::moved_path(&absolute, from, to) {
                    *path = relative_path(root, &moved);
                }
            }
            Ok(())
        };
        for state in self.open_editors.iter_mut().chain(
            self.recent_files
                .iter_mut()
                .filter_map(|recent| recent.editor.as_mut()),
        ) {
            retarget(&mut state.origin)?;
            if let Some(identity) = &mut state.disk_identity
                && let Some(path) = crate::tree::moved_path(&identity.canonical_path, from, to)
            {
                identity.canonical_path = path;
            }
        }
        for recent in &mut self.recent_files {
            let absolute = resolve_path(root, &recent.path)?;
            if let Some(moved) = crate::tree::moved_path(&absolute, from, to) {
                recent.path = relative_path(root, &moved);
            }
        }
        if let Some(origin) = &mut self.active_origin {
            retarget(origin)?;
        }
        Ok(())
    }
    /// Source behavior sorts case-insensitively by path and caps recents at 20.
    pub fn record_recent(&mut self, recent: RecentFileV1) {
        self.recent_files.retain(|entry| entry.path != recent.path);
        self.recent_files.push(recent);
        self.recent_files
            .sort_by_key(|entry| entry.path.to_string_lossy().to_lowercase());
        self.recent_files.truncate(20);
    }
    pub fn remove_recent(&mut self, root: &Path, path: &Path) -> Result<()> {
        let absolute = resolve_path(root, path)?;
        self.recent_files
            .retain(|entry| resolve_path(root, &entry.path).ok().as_ref() != Some(&absolute));
        // Recovery of a live dirty document is owned by the shell before close.
        self.open_editors.retain(|entry| match &entry.origin {
            DocumentOrigin::File(path) => resolve_path(root, path).ok().as_ref() != Some(&absolute),
            _ => true,
        });
        Ok(())
    }
    pub fn most_recent_editor(&self) -> Option<&EditorStateV1> {
        self.active_origin
            .as_ref()
            .and_then(|origin| {
                self.open_editors
                    .iter()
                    .find(|entry| &entry.origin == origin)
            })
            .or_else(|| {
                self.recent_files
                    .iter()
                    .filter_map(|entry| {
                        entry
                            .editor
                            .as_ref()
                            .map(|editor| (entry.last_opened_epoch_millis, editor))
                    })
                    .max_by_key(|(time, _)| *time)
                    .map(|(_, editor)| editor)
            })
            .or_else(|| self.open_editors.first())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorStateV1 {
    pub origin: DocumentOrigin,
    #[serde(default = "empty_history")]
    pub history: HistoryStateV1,
    #[serde(default)]
    pub format: FileFormat,
    #[serde(default)]
    pub scroll_top: usize,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub grammar_language: Option<String>,
    #[serde(default)]
    pub grammar_available: bool,
    #[serde(default)]
    pub read_only: bool,
    /// One-time legacy baseline reconciliation; native capture clears it while
    /// retaining original legacy fields for recovery.
    #[serde(default)]
    pub legacy_format: bool,
    #[serde(default)]
    pub last_modified_millis: Option<i64>,
    #[serde(default)]
    pub disk_identity: Option<DiskIdentity>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
impl EditorStateV1 {
    /// Call from a worker using an owned cheap Document clone; history/text
    /// flattening and JSON encoding do not belong to painting/input handling.
    pub fn capture(
        root: &Path,
        document: &Document,
        caret: &CaretState,
        scroll_top: usize,
        disk_identity: Option<DiskIdentity>,
    ) -> Self {
        let mut origin = document.origin().clone();
        if let DocumentOrigin::File(path) = &mut origin {
            *path = relative_path(root, path);
        }
        Self {
            origin,
            history: document.export_history(caret),
            format: document.format(),
            scroll_top,
            mime: None,
            language: document.snapshot().language,
            grammar_language: None,
            grammar_available: false,
            read_only: document.is_read_only(),
            legacy_format: false,
            last_modified_millis: disk_identity
                .as_ref()
                .and_then(|identity| identity.modified_nanos)
                .and_then(|nanos| i64::try_from(nanos / 1_000_000).ok()),
            disk_identity,
            extra: Map::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFileV1 {
    pub path: PathBuf,
    #[serde(default)]
    pub last_opened_epoch_millis: i64,
    #[serde(default)]
    pub last_modified_millis: Option<i64>,
    #[serde(default)]
    pub editor: Option<EditorStateV1>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default = "code_viewer")]
    pub viewer_type: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
fn code_viewer() -> String {
    "CODE".into()
}
/// Session file origins may be outside the project (explicit CLI files). This
/// function does lexical normalization only; tree actions retain stricter scope.
pub fn resolve_path(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(DomainError::Unsupported("empty session path".into()));
    }
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if path.as_os_str().is_empty() {
        return Err(DomainError::Unsupported("empty session path".into()));
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    Ok(out)
}
pub fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).into()
}
