//! Owned, unfinished request text is separate from the Postman collection.
use super::{buffers, form::BufferKind};
use kode_core::{DomainError, Result};
use kode_platform::jobs::CancellationToken;
use kode_rest::model::{NodePath, ProjectState, Workspace, editing};
use serde_json::{Map, Value};
use std::{collections::HashMap, sync::Arc};
pub const KEY: &str = "x-kode-rs-request-buffers";
pub type Drafts = HashMap<(NodePath, String), Arc<str>>;
pub fn kind(name: &str) -> Option<BufferKind> {
    [
        BufferKind::Headers,
        BufferKind::RawBody,
        BufferKind::GraphqlQuery,
        BufferKind::GraphqlVariables,
    ]
    .into_iter()
    .find(|kind| kind.name() == name)
}
pub fn read(state: &ProjectState) -> Result<Drafts> {
    let mut drafts = Drafts::new();
    if let Some(value) = state.extra.get(KEY) {
        let invalid = || DomainError::Parse {
            format: "REST request buffers",
            message: "Invalid saved request buffers; original REST state retained".into(),
        };
        for (path, value) in value.as_object().ok_or_else(invalid)? {
            let path = NodePath::decode(Some(path))?;
            for (name, text) in value.as_object().ok_or_else(invalid)? {
                drafts.insert(
                    (path.clone(), name.clone()),
                    Arc::from(text.as_str().ok_or_else(invalid)?),
                );
            }
        }
    }
    Ok(drafts)
}
pub fn write(drafts: Drafts) -> Value {
    let mut paths = Map::new();
    for ((path, name), text) in drafts {
        paths
            .entry(path.encode())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("created object")
            .insert(name, Value::String(text.to_string()));
    }
    Value::Object(paths)
}
pub fn apply_text(
    model: &mut Workspace,
    path: &NodePath,
    kind: BufferKind,
    text: &str,
) -> Result<bool> {
    if kind == BufferKind::Headers {
        editing::headers(model, path, text)
    } else if let Some(field) = kind.field() {
        editing::field(model, path, field, text)
    } else {
        Ok(false)
    }
}
pub fn applies(path: &NodePath, kind: BufferKind, selection: &NodePath) -> bool {
    path.starts_with(selection) || (kind == BufferKind::Headers && selection.starts_with(path))
}
/// Strict worker merge for execution/preview. Current ropes supersede saved text.
pub fn apply(
    model: &mut Workspace,
    drafts: &Drafts,
    overrides: &[buffers::Override],
    selection: &NodePath,
    cancel: &CancellationToken,
) -> Result<()> {
    for ((path, name), text) in drafts {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let Some(kind) = kind(name) else {
            continue;
        };
        if applies(path, kind, selection)
            && model.node(path).is_some()
            && !overrides
                .iter()
                .any(|capture| capture.path == *path && capture.kind == kind)
        {
            apply_text(model, path, kind, text)?;
        }
    }
    for capture in overrides {
        if applies(&capture.path, capture.kind, selection) {
            buffers::apply_override(model, capture, cancel)?;
        }
    }
    Ok(())
}
