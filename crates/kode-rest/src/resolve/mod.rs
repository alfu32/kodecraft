//! Pure request planning is separated from body-file I/O and network execution.
mod body;
mod preview;
pub use preview::RequestPreview;
pub mod request;
pub mod types;
pub mod variables;
use crate::model::NodePath;
use kode_core::{DomainError, Result};
pub use request::RequestResolver;
use serde_json::Value;
pub use types::*;
pub use variables::{Resolution, Variable, VariableResolver};
pub(crate) fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST request",
        message: message.into(),
    }
}
pub(crate) fn primitive(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}
pub(crate) fn boolean(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => value == "true",
        _ => false,
    }
}
pub(crate) fn ancestors<'a>(collection: &'a Value, path: &NodePath) -> Result<Vec<&'a Value>> {
    if path.0.len() > 64 || !collection.is_object() {
        return Err(invalid("Invalid collection or item path"));
    }
    let mut nodes = vec![collection];
    let mut current = collection;
    for &index in &path.0 {
        current = current
            .get("item")
            .and_then(Value::as_array)
            .and_then(|items| items.get(index))
            .filter(|item| item.is_object())
            .ok_or_else(|| invalid("Request or folder path does not exist"))?;
        nodes.push(current);
    }
    Ok(nodes)
}
