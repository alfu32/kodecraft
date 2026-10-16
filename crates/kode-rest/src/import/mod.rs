//! Bounded, explicit conversion into the lossless Postman model. File operations
//! are blocking worker operations; parsing/conversion never follows external refs.
mod files;
mod openapi;
mod parse;
mod yaml;
use crate::model::{NodePath, Workspace, workspace::validate_collection};
pub use files::{export_file, read_file};
use kode_core::{DomainError, Result};
pub use parse::{Format, MAX_DOCUMENT_BYTES, parse};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Postman,
    OpenApi,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// JSON pointer into the original document (not a resolved filesystem path).
    pub pointer: String,
    pub code: &'static str,
    pub message: &'static str,
}
pub struct Import {
    pub kind: Kind,
    pub collection: Value,
    pub notices: Vec<Notice>,
}
impl std::fmt::Debug for Import {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Import")
            .field("kind", &self.kind)
            .field("notices", &self.notices)
            .finish_non_exhaustive()
    }
}
impl Import {
    /// Detect shape after parsing. An invalid document never changes a workspace.
    pub fn from_document(document: Value) -> Result<Self> {
        parse::validate_document(&document)?;
        if document.get("openapi").is_some() || document.get("swagger").is_some() {
            let converted = openapi::convert(document)?;
            parse::validate_document(&converted.collection)?;
            Ok(converted)
        } else {
            validate_collection(&document)?;
            Ok(Self {
                kind: Kind::Postman,
                collection: document,
                notices: vec![],
            })
        }
    }
    /// Root Postman import replaces the collection as in the source application.
    /// Nested Postman import retains the whole collection as one folder so its
    /// auth, variables, scripts, extensions and unknown fields keep their scope.
    /// OpenAPI import appends tag folders, retaining the original document once
    /// on a containing folder whenever its source metadata must be retained.
    pub fn apply(&self, workspace: &mut Workspace, parent: &NodePath) -> Result<Vec<NodePath>> {
        validate_collection(&self.collection)?;
        parse::validate_document(&self.collection)?;
        let mut candidate = Workspace::from_state(workspace.project_state())?;
        let paths = self.apply_to_candidate(&mut candidate, parent)?;
        parse::validate_document(candidate.collection())?;
        workspace.restore(candidate.project_state())?;
        Ok(paths)
    }
    fn apply_to_candidate(
        &self,
        workspace: &mut Workspace,
        parent: &NodePath,
    ) -> Result<Vec<NodePath>> {
        if self.kind == Kind::Postman && parent.is_root() {
            workspace.replace_collection(self.collection.clone())?;
            return Ok(vec![NodePath::default()]);
        }
        if self.kind == Kind::OpenApi {
            let mut folders = self.collection["item"]
                .as_array()
                .expect("converted collection")
                .clone();
            if folders.iter().any(|folder| !folder.is_object())
                || !self.collection["x-kode-openapi"].is_object()
            {
                return Err(invalid("/item", "invalid converted OpenAPI collection"));
            }
            // Preserve source once without adding an extra visible tree level.
            // Empty specifications still get a container for lossless metadata.
            if folders.is_empty() {
                folders.push(json!({"name": self.collection["info"]["name"], "item": []}));
            }
            folders[0]["x-kode-openapi"] = self.collection["x-kode-openapi"].clone();
            return workspace.append_items(parent, folders);
        }
        let mut folder = self.collection.clone();
        let name = folder["info"]["name"]
            .as_str()
            .unwrap_or("Imported collection")
            .to_string();
        // An existing root `name` is itself arbitrary user data. Prefer it to
        // synthesizing a display name, preserving every original field.
        folder
            .as_object_mut()
            .expect("validated collection")
            .entry("name")
            .or_insert(Value::String(name));
        workspace.append_items(parent, vec![folder])
    }
}
fn invalid(pointer: &str, message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST import",
        message: format!("{pointer}: {message}"),
    }
}
fn pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}
