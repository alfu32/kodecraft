use super::{Import, Kind, Notice, invalid, pointer};
use crate::model::{default_collection, workspace::validate_collection};
use indexmap::IndexMap;
use kode_core::Result;
use serde_json::{Value, json};
mod auth;
mod operation;
mod schema;
use crate::resolve::primitive;

struct Converter<'a> {
    document: &'a Value,
    notices: Vec<Notice>,
    incomplete: bool,
    work: usize,
}
impl Converter<'_> {
    fn notice(&mut self, path: &str, code: &'static str, message: &'static str, incomplete: bool) {
        self.incomplete |= incomplete;
        if self.notices.len() < 4096 {
            self.notices.push(Notice {
                pointer: path.into(),
                code,
                message,
            });
        }
    }
    /// Resolve only local JSON pointers, including escaped component names.
    /// No import can read another file or start a network request.
    fn resolve(&mut self, value: &Value, path: &str) -> Option<Value> {
        let mut value = value.clone();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
                return Some(value);
            };
            if !reference.starts_with("#/") || reference.contains('%') {
                self.notice(path, "external-reference", "Only local, unencoded JSON pointer references are converted; original reference retained", true);
                return None;
            }
            if !seen.insert(reference.to_string()) {
                self.notice(
                    path,
                    "reference-cycle",
                    "Reference cycle cannot supply an editable request value",
                    true,
                );
                return None;
            }
            let Some(target) = self.document.pointer(&reference[1..]) else {
                self.notice(
                    path,
                    "missing-reference",
                    "Local reference target does not exist",
                    true,
                );
                return None;
            };
            if value.as_object().is_some_and(|object| object.len() > 1) {
                self.notice(
                    path,
                    "reference-siblings",
                    "Reference siblings require review; conversion uses the referenced value",
                    true,
                );
            }
            value = target.clone();
        }
        self.notice(
            path,
            "reference-limit",
            "Reference chain exceeds 64 steps",
            true,
        );
        None
    }
    fn inventory(&mut self) {
        let mut pending = vec![(self.document, String::new())];
        while let Some((value, path)) = pending.pop() {
            match value {
                Value::Object(object) => {
                    for (key, child) in object {
                        let child_path = pointer(&path, key);
                        match key.as_str() {
                        "$ref" if child.as_str().is_some_and(|reference| !reference.starts_with("#/")) =>
                            self.notice(&child_path, "external-reference", "External references are retained without filesystem or network access", false),
                        "callbacks" | "webhooks" | "links" =>
                            self.notice(&child_path, "non-request-operation", "Callbacks, webhooks and response links are retained as source metadata, not runnable requests", false),
                        "oneOf" | "anyOf" | "allOf" | "not" | "if" | "then" | "else" | "unevaluatedProperties" | "$dynamicRef" =>
                            self.notice(&child_path, "schema-construct", "Schema composition or dynamic validation is retained; it is not evaluated by the request sample generator", false),
                        _ => {},
                    }
                        pending.push((child, child_path));
                    }
                }
                Value::Array(array) => pending.extend(
                    array
                        .iter()
                        .enumerate()
                        .map(|(index, child)| (child, pointer(&path, &index.to_string()))),
                ),
                _ => {}
            }
        }
    }
}
pub(super) fn convert(document: Value) -> Result<Import> {
    let version = primitive(document.get("openapi"))
        .or_else(|| primitive(document.get("swagger")))
        .ok_or_else(|| invalid("/", "expected an OpenAPI or Swagger version"))?;
    if !(version.starts_with("3.") || version == "2.0" || version == "2") {
        return Err(invalid(
            "/",
            "supported versions are Swagger 2.0 and OpenAPI 3.x",
        ));
    }
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("/paths", "expected an object"))?;
    let mut converter = Converter {
        document: &document,
        notices: vec![],
        incomplete: false,
        work: 0,
    };
    converter.inventory();
    let mut groups: IndexMap<String, Vec<Value>> = IndexMap::new();
    let mut operation_count = 0;
    let mut output_size = super::parse::encoded_size(&document)?;
    for (path, item) in paths {
        if path.starts_with("x-") {
            continue;
        }
        let item_pointer = pointer("/paths", path);
        if !path.starts_with('/') {
            return Err(invalid(&item_pointer, "path must start with /"));
        }
        converter.incomplete = false;
        let Some(item) = converter.resolve(item, &item_pointer) else {
            continue;
        };
        let Some(object) = item.as_object() else {
            return Err(invalid(&item_pointer, "path item must be an object"));
        };
        let path_incomplete = converter.incomplete;
        for (method, operation) in object {
            let method = method.to_ascii_lowercase();
            if ![
                "get", "post", "put", "patch", "delete", "head", "options", "trace",
            ]
            .contains(&method.as_str())
            {
                continue;
            }
            let operation_pointer = pointer(&item_pointer, &method);
            if !operation.is_object() {
                return Err(invalid(&operation_pointer, "operation must be an object"));
            }
            operation_count += 1;
            if operation_count > 100_000 {
                return Err(invalid("/paths", "too many operations"));
            }
            converter.incomplete = path_incomplete;
            let tag = operation
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|tags| tags.first())
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    path.trim_matches('/')
                        .split('/')
                        .next()
                        .filter(|name| !name.is_empty())
                        .unwrap_or("default")
                });
            let request =
                converter.operation(path, &method, operation, &item, &operation_pointer)?;
            output_size += super::parse::encoded_size(&request)?;
            if output_size > super::MAX_DOCUMENT_BYTES {
                return Err(invalid(
                    "/paths",
                    "converted requests and source metadata exceed 16 MiB",
                ));
            }
            groups.entry(tag.to_string()).or_default().push(request);
        }
    }
    let title = document
        .pointer("/info/title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Imported OpenAPI");
    let single_default = groups.len() == 1 && groups.contains_key("default");
    let mut collection = default_collection(title);
    collection["item"] = Value::Array(groups.into_iter().map(|(name, requests)| json!({"name": if single_default { title } else { &name }, "item": requests})).collect());
    let notices = converter.notices;
    collection["x-kode-openapi"] = json!({"document": document, "notices": notices.iter().map(|n| json!({"pointer":n.pointer,"code":n.code,"message":n.message})).collect::<Vec<_>>()});
    validate_collection(&collection)?;
    Ok(Import {
        kind: Kind::OpenApi,
        collection,
        notices,
    })
}
