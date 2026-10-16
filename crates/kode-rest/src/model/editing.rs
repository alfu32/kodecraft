//! Lossless field/row operations used by request forms and captured editor ropes.
//! These operations never resolve variables, read body files or submit requests.
use super::{NodePath, Workspace};
use kode_core::{DomainError, Result};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    Name,
    Description,
    Method,
    Url,
    RawBody,
    GraphqlQuery,
    GraphqlVariables,
    File,
    Timeout,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rows {
    Variable,
    Query,
    Path,
    UrlEncoded,
    FormData,
    Auth,
}
#[derive(Clone, PartialEq, Eq)]
pub struct EntryValue {
    pub key: String,
    pub value: String,
    pub description: String,
    pub kind: String,
    pub enabled: bool,
}
impl Default for EntryValue {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            description: String::new(),
            kind: "string".into(),
            enabled: true,
        }
    }
}
impl std::fmt::Debug for EntryValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestEntryValue(..)")
    }
}
pub fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(value) => value.to_string(),
    }
}
pub fn description(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(object)) => text(object.get("content")),
        other => text(other),
    }
}
pub fn disabled(value: &Value) -> bool {
    value
        .get("disabled")
        .is_some_and(|value| value == true || value == "true")
}
pub fn is_request(node: &Value, path: &NodePath) -> bool {
    !path.is_root()
        && node
            .get("request")
            .is_some_and(|request| request.is_object() || request.is_string())
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST form",
        message: message.into(),
    }
}
fn object(value: Option<&Value>) -> Value {
    value
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}
fn request(node: &Value) -> Result<Value> {
    match node.get("request") {
        Some(value) if value.is_object() => Ok(value.clone()),
        Some(Value::String(url)) => Ok(json!({"method":"GET","url":url})),
        _ => Err(invalid("Selected node is not a request")),
    }
}
fn url_object(request: &Value) -> Value {
    match request.get("url") {
        Some(value) if value.is_object() => value.clone(),
        other => json!({"raw":text(other)}),
    }
}
pub fn raw_url(url: &Value) -> String {
    if let Some(raw) = url
        .as_str()
        .or_else(|| url.get("raw").and_then(Value::as_str))
    {
        return raw.into();
    }
    fn joined(value: Option<&Value>, separator: &str) -> String {
        match value {
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| text(Some(value)))
                .collect::<Vec<_>>()
                .join(separator),
            other => text(other),
        }
    }
    let scheme = url
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("http");
    let host = joined(url.get("host"), ".");
    let path = joined(url.get("path"), "/");
    let port = text(url.get("port"));
    let port = if port.is_empty() {
        String::new()
    } else {
        format!(":{port}")
    };
    format!("{scheme}://{host}{port}/{}", path.trim_start_matches('/'))
}
fn query_part(raw: &str) -> Option<&str> {
    raw.split('#')
        .next()?
        .split_once('?')
        .map(|(_, query)| query)
}
fn set_text(slot: &mut Value, next: &str) {
    if text(Some(slot)) != next {
        *slot = Value::String(next.into());
    }
}
fn set_description(slot: &mut Value, next: &str) {
    if description(Some(slot)) == next {
        return;
    }
    if let Some(object) = slot.as_object_mut() {
        object.insert("content".into(), next.into());
    } else {
        *slot = next.into();
    }
}
fn description_field(parent: &mut Value, next: &str) {
    if description(parent.get("description")) == next {
        return;
    }
    let mut value = parent.get("description").cloned().unwrap_or(Value::Null);
    set_description(&mut value, next);
    parent["description"] = value;
}
pub fn field(model: &mut Workspace, path: &NodePath, field: Field, next: &str) -> Result<bool> {
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    if field == Field::Name {
        return model.rename(path, next);
    }
    if field == Field::Description {
        if path.is_root() {
            let mut info = object(node.get("info"));
            description_field(&mut info, next);
            return model.set_node_field(path, "info", Some(info));
        }
        if description(node.get("description")) == next {
            return Ok(false);
        }
        let mut value = node.get("description").cloned().unwrap_or(Value::Null);
        set_description(&mut value, next);
        return model.set_node_field(path, "description", Some(value));
    }
    if field == Field::Timeout {
        let mut settings = object(node.get("protocolProfileBehavior"));
        let digits: String = next.chars().filter(char::is_ascii_digit).collect();
        settings["requestTimeout"] = Value::String(if digits.is_empty() {
            "30000".into()
        } else {
            digits
        });
        return model.set_node_field(path, "protocolProfileBehavior", Some(settings));
    }
    let mut value = request(node)?;
    match field {
        Field::Method => set_text(
            &mut value["method"],
            if next.trim().is_empty() { "GET" } else { next },
        ),
        Field::Url => {
            if value["url"].is_object() {
                let old = text(value["url"].get("raw"));
                if query_part(&old) != query_part(next) {
                    // An explicit edit of the raw query replaces its structured
                    // representation. Host/path-only edits retain all row metadata.
                    value["url"]
                        .as_object_mut()
                        .expect("URL object")
                        .remove("query");
                }
                value["url"]["raw"] = next.into();
            } else {
                value["url"] = next.into();
            }
        }
        Field::RawBody | Field::GraphqlQuery | Field::GraphqlVariables | Field::File => {
            let mut body = object(value.get("body"));
            match field {
                Field::RawBody => {
                    if body.get("mode").is_none() {
                        body["mode"] = "raw".into();
                    }
                    set_text(&mut body["raw"], next);
                }
                Field::GraphqlQuery | Field::GraphqlVariables => {
                    if body.get("mode").is_none() {
                        body["mode"] = "graphql".into();
                    }
                    let mut graphql = object(body.get("graphql"));
                    if field == Field::GraphqlQuery {
                        set_text(&mut graphql["query"], next);
                    } else {
                        graphql["variables"] =
                            serde_json::from_str(next).unwrap_or_else(|_| next.into());
                    }
                    body["graphql"] = graphql;
                }
                Field::File => {
                    if body.get("mode").is_none() {
                        body["mode"] = "file".into();
                    }
                    let mut file = object(body.get("file"));
                    set_text(&mut file["src"], next);
                    body["file"] = file;
                }
                _ => unreachable!(),
            }
            value["body"] = body;
        }
        _ => unreachable!(),
    }
    model.set_node_field(path, "request", Some(value))
}
pub fn body_mode(model: &mut Workspace, path: &NodePath, mode: &str) -> Result<bool> {
    if !["none", "raw", "urlencoded", "formdata", "file", "graphql"].contains(&mode) {
        return Err(invalid("Unsupported body mode"));
    }
    let mut value = request(
        model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?,
    )?;
    let mut body = object(value.get("body"));
    body["mode"] = mode.into();
    value["body"] = body;
    model.set_node_field(path, "request", Some(value))
}
pub fn setting(model: &mut Workspace, path: &NodePath, key: &str) -> Result<bool> {
    if !["disableRedirects", "disableSslVerification"].contains(&key) {
        return Err(invalid("Unsupported request setting"));
    }
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    let mut settings = object(node.get("protocolProfileBehavior"));
    settings[key] = Value::Bool(
        !settings
            .get(key)
            .is_some_and(|value| value == true || value == "true"),
    );
    model.set_node_field(path, "protocolProfileBehavior", Some(settings))
}
pub fn auth(model: &mut Workspace, path: &NodePath, kind: Option<&str>) -> Result<bool> {
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    let mut value = if is_request(node, path) {
        request(node)?
    } else {
        json!({"auth":node.get("auth")})
    };
    if let Some(kind) = kind {
        if !["noauth", "bearer", "basic", "apikey"].contains(&kind) {
            return Err(invalid("Unsupported authentication choice"));
        }
        let mut definition = object(value.get("auth"));
        definition["type"] = kind.into();
        if kind != "noauth" && definition.get(kind).is_none() {
            let fields: &[(&str, &str)] = match kind {
                "bearer" => &[("token", "{{bearerToken}}")],
                "basic" => &[("username", "{{username}}"), ("password", "{{password}}")],
                "apikey" => &[
                    ("key", "X-API-Key"),
                    ("value", "{{apiKey}}"),
                    ("in", "header"),
                ],
                _ => &[],
            };
            definition[kind] = fields
                .iter()
                .map(|(key, value)| json!({"key":key,"value":value,"type":"string"}))
                .collect();
        }
        value["auth"] = definition;
    } else {
        value
            .as_object_mut()
            .expect("request/node object")
            .remove("auth");
    }
    if is_request(node, path) {
        model.set_node_field(path, "request", Some(value))
    } else {
        model.set_node_field(path, "auth", value.get("auth").cloned())
    }
}
fn decode_query(raw: &str) -> Result<Vec<Value>> {
    fn decode(value: &str) -> Result<String> {
        let plus = value.replace('+', " ");
        percent_encoding::percent_decode_str(&plus)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|_| invalid("Raw query contains non-UTF-8 bytes; edit the URL directly"))
    }
    let Some(query) = query_part(raw).filter(|query| !query.is_empty()) else {
        return Ok(Vec::new());
    };
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part
                .split_once('=')
                .map_or((part, None), |(key, value)| (key, Some(value)));
            Ok(json!({"key":decode(key)?,"value":value.map(decode).transpose()?}))
        })
        .collect()
}
pub fn rows(node: &Value, path: &NodePath, kind: Rows) -> Result<Vec<Value>> {
    let request = match node.get("request") {
        Some(value) if value.is_object() => std::borrow::Cow::Borrowed(value),
        Some(Value::String(url)) => std::borrow::Cow::Owned(json!({"method":"GET","url":url})),
        _ => std::borrow::Cow::Owned(json!({})),
    };
    let values = match kind {
        Rows::Variable => node.get("variable"),
        Rows::Query => {
            if let Some(values) = request
                .get("url")
                .and_then(|url| url.get("query"))
                .and_then(Value::as_array)
            {
                return Ok(values.clone());
            }
            let url = request.get("url");
            return decode_query(&url.map(raw_url).unwrap_or_default());
        }
        Rows::Path => request.get("url").and_then(|url| url.get("variable")),
        Rows::FormData => request.get("body").and_then(|body| body.get("formdata")),
        Rows::UrlEncoded => request.get("body").and_then(|body| body.get("urlencoded")),
        Rows::Auth => {
            let parent = if is_request(node, path) {
                &request
            } else {
                node
            };
            let auth = parent.get("auth");
            auth.and_then(|auth| auth.get(auth.get("type")?.as_str()?))
        }
    };
    Ok(values
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}
fn set_rows(
    model: &mut Workspace,
    path: &NodePath,
    kind: Rows,
    values: Vec<Value>,
) -> Result<bool> {
    if kind == Rows::Variable {
        return model.set_node_field(path, "variable", Some(values.into()));
    }
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    let request_node = is_request(node, path);
    let mut value = if kind == Rows::Auth && !request_node {
        json!({"auth":node.get("auth")})
    } else {
        request(node)?
    };
    match kind {
        Rows::Query | Rows::Path => {
            let mut url = url_object(&value);
            if kind == Rows::Query {
                let raw = raw_url(&url);
                let fragment = raw
                    .split_once('#')
                    .map(|(_, fragment)| fragment.to_string());
                let base = raw
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("");
                let mut query = url::form_urlencoded::Serializer::new(String::new());
                for entry in &values {
                    if disabled(entry) {
                        continue;
                    }
                    let key = text(entry.get("key"));
                    if key.is_empty() {
                        continue;
                    }
                    if entry.get("value") == Some(&Value::Null) {
                        query.append_key_only(&key);
                    } else {
                        query.append_pair(&key, &text(entry.get("value")));
                    }
                }
                let query = query.finish();
                url["raw"] = format!(
                    "{base}{}{}",
                    if query.is_empty() {
                        String::new()
                    } else {
                        format!("?{query}")
                    },
                    fragment.map_or(String::new(), |fragment| format!("#{fragment}"))
                )
                .into();
                url["query"] = values.into();
            } else {
                url["variable"] = values.into();
            }
            value["url"] = url;
        }
        Rows::FormData | Rows::UrlEncoded => {
            let mut body = object(value.get("body"));
            let key = if kind == Rows::FormData {
                "formdata"
            } else {
                "urlencoded"
            };
            if body.get("mode").is_none() {
                body["mode"] = key.into();
            }
            body[key] = values.into();
            value["body"] = body;
        }
        Rows::Auth => {
            let mut auth = object(value.get("auth"));
            let key = auth
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("Choose a local authentication type first"))?
                .to_string();
            auth[key] = values.into();
            value["auth"] = auth;
        }
        Rows::Variable => unreachable!(),
    }
    if kind == Rows::Auth && !request_node {
        model.set_node_field(path, "auth", value.get("auth").cloned())
    } else {
        model.set_node_field(path, "request", Some(value))
    }
}
pub fn entry_value(entry: &Value, kind: Rows) -> EntryValue {
    EntryValue {
        key: text(entry.get("key")),
        value: if kind == Rows::FormData && entry.get("type") == Some(&json!("file")) {
            text(entry.get("src").or_else(|| entry.get("value")))
        } else {
            text(entry.get("value"))
        },
        description: if kind == Rows::FormData {
            text(entry.get("type"))
        } else {
            description(entry.get("description"))
        },
        kind: if entry.get("type").is_none() {
            "string".into()
        } else {
            text(entry.get("type"))
        },
        enabled: !disabled(entry),
    }
}
pub fn add_row(model: &mut Workspace, path: &NodePath, kind: Rows) -> Result<usize> {
    let mut values = rows(
        model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?,
        path,
        kind,
    )?;
    if kind == Rows::Auth {
        return Err(invalid(
            "Authentication fields are defined by the selected type",
        ));
    }
    let key = match kind {
        Rows::Variable => "newVariable",
        Rows::Query => "param",
        Rows::Path => "path",
        Rows::FormData => "field",
        _ => "parameter",
    };
    let index = values.len();
    let mut value = json!({"key":key,"value":""});
    if kind == Rows::Variable {
        value["type"] = "string".into();
    } else if kind == Rows::FormData {
        value["type"] = "text".into();
    }
    values.push(value);
    set_rows(model, path, kind, values)?;
    Ok(index)
}
pub fn edit_row(
    model: &mut Workspace,
    path: &NodePath,
    kind: Rows,
    index: usize,
    next: &EntryValue,
) -> Result<bool> {
    let mut values = rows(
        model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?,
        path,
        kind,
    )?;
    let old = values
        .get_mut(index)
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("Selected row is no longer editable"))?;
    if kind != Rows::Auth {
        set_text(&mut old["key"], &next.key);
    }
    if kind == Rows::FormData && next.description.eq_ignore_ascii_case("file") {
        // An imported path array remains an array when left unchanged. An
        // explicit file-path edit writes the source actually used by the resolver.
        let source = old.get("src").or_else(|| old.get("value"));
        if text(source) != next.value {
            old["src"] = if next.value.trim_start().starts_with('[') {
                let paths: Vec<String> = serde_json::from_str(&next.value)
                    .map_err(|_| invalid("File sources must be a path or a JSON array of paths"))?;
                serde_json::to_value(paths).map_err(|_| invalid("Cannot store file sources"))?
            } else {
                next.value.clone().into()
            };
        }
        old["type"] = "file".into();
    } else {
        set_text(&mut old["value"], &next.value);
        if kind == Rows::FormData {
            if !next.description.is_empty() && !next.description.eq_ignore_ascii_case("text") {
                return Err(invalid("Multipart entry type must be text or file"));
            }
            old["type"] = "text".into();
        } else {
            description_field(old, &next.description);
        }
    }
    if kind == Rows::Variable {
        set_text(&mut old["type"], &next.kind);
        old["disabled"] = Value::Bool(!next.enabled);
    }
    set_rows(model, path, kind, values)
}
pub fn toggle_row(
    model: &mut Workspace,
    path: &NodePath,
    kind: Rows,
    index: usize,
) -> Result<bool> {
    let mut values = rows(
        model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?,
        path,
        kind,
    )?;
    let entry = values
        .get_mut(index)
        .filter(|value| value.is_object())
        .ok_or_else(|| invalid("Selected row is no longer editable"))?;
    entry["disabled"] = Value::Bool(!disabled(entry));
    set_rows(model, path, kind, values)
}
pub fn remove_row(
    model: &mut Workspace,
    path: &NodePath,
    kind: Rows,
    index: usize,
) -> Result<bool> {
    let mut values = rows(
        model
            .node(path)
            .ok_or_else(|| invalid("REST item no longer exists"))?,
        path,
        kind,
    )?;
    if index >= values.len() {
        return Err(invalid("Selected row no longer exists"));
    }
    values.remove(index);
    set_rows(model, path, kind, values)
}
pub fn headers_text(node: &Value, path: &NodePath) -> String {
    let parent = if is_request(node, path) {
        node.get("request").unwrap_or(&Value::Null)
    } else {
        node
    };
    let field = if is_request(node, path) {
        "header"
    } else {
        "x-kode-headers"
    };
    parent
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            entry
                .get("key")
                .and_then(Value::as_str)
                .is_some_and(|key| !key.is_empty())
        })
        .map(|entry| {
            format!(
                "{}{}: {}",
                if disabled(entry) { "# " } else { "" },
                text(entry.get("key")),
                text(entry.get("value"))
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn headers(model: &mut Workspace, path: &NodePath, next: &str) -> Result<bool> {
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    let request_node = is_request(node, path);
    let key = if request_node {
        "header"
    } else {
        "x-kode-headers"
    };
    let mut value = if request_node {
        request(node)?
    } else {
        {
            let mut value = json!({});
            value[key] = node.get(key).cloned().unwrap_or(Value::Null);
            value
        }
    };
    let previous = value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut available: HashMap<String, VecDeque<usize>> = HashMap::new();
    let mut known = Vec::new();
    let mut opaque = Vec::new();
    for (index, entry) in previous.into_iter().enumerate() {
        if let Some(key) = entry
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        {
            available
                .entry(key.to_ascii_lowercase())
                .or_default()
                .push_back(known.len());
            known.push((key.to_ascii_lowercase(), entry));
        } else {
            opaque.push((index, entry));
        }
    }
    let mut parsed = Vec::new();
    for (index, line) in next.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let disabled = line.starts_with('#');
        let line = line.strip_prefix('#').unwrap_or(line).trim();
        let Some((name, header_value)) = line.split_once(':') else {
            if disabled {
                continue;
            }
            return Err(invalid(&format!(
                "Header line {} needs Name: Value",
                index + 1
            )));
        };
        let name = name.trim();
        let header_value = header_value.trim();
        if ::http::HeaderName::from_bytes(name.as_bytes()).is_err()
            || ::http::HeaderValue::from_str(header_value).is_err()
        {
            return Err(invalid(&format!(
                "Invalid header syntax at line {}",
                index + 1
            )));
        }
        parsed.push((name, header_value, disabled));
    }
    let names: HashSet<_> = parsed
        .iter()
        .map(|(name, _, _)| name.to_ascii_lowercase())
        .collect();
    let mut used = HashSet::new();
    let mut entries = Vec::new();
    for (index, (name, header_value, disabled)) in parsed.into_iter().enumerate() {
        let matched = available
            .get_mut(&name.to_ascii_lowercase())
            .and_then(|indices| {
                while let Some(index) = indices.pop_front() {
                    if !used.contains(&index) {
                        return Some(index);
                    }
                }
                None
            })
            .or_else(|| {
                known
                    .get(index)
                    .filter(|(name, _)| !used.contains(&index) && !names.contains(name))
                    .map(|_| index)
            });
        let mut entry = matched
            .map(|index| {
                used.insert(index);
                known[index].1.clone()
            })
            .unwrap_or_else(|| json!({}));
        set_text(&mut entry["key"], name);
        set_text(&mut entry["value"], header_value);
        if disabled || entry.get("disabled").is_some() {
            entry["disabled"] = disabled.into();
        }
        entries.push(entry);
    }
    for (index, entry) in opaque {
        entries.insert(index.min(entries.len()), entry);
    }
    value[key] = entries.into();
    if request_node {
        model.set_node_field(path, "request", Some(value))
    } else {
        model.set_node_field(path, key, value.get(key).cloned())
    }
}
/// Publish already parsed local headers without reinterpreting the editor rope
/// on the application thread. Other request fields retain their current values.
pub fn replace_headers(model: &mut Workspace, path: &NodePath, headers: Value) -> Result<bool> {
    if !headers.is_array() {
        return Err(invalid("Prepared headers must be an array"));
    }
    let node = model
        .node(path)
        .ok_or_else(|| invalid("REST item no longer exists"))?;
    if is_request(node, path) {
        let mut value = request(node)?;
        value["header"] = headers;
        model.set_node_field(path, "request", Some(value))
    } else {
        model.set_node_field(path, "x-kode-headers", Some(headers))
    }
}
/// Called only by the explicit import-review acknowledgement action.
pub fn acknowledge_import(model: &mut Workspace, path: &NodePath) -> Result<bool> {
    model.set_node_field(path, "x-kode-import-incomplete", Some(Value::Bool(false)))
}
