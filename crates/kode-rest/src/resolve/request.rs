use super::*;
use crate::model::Environment;
use base64::{Engine, engine::general_purpose::STANDARD};
use indexmap::{IndexMap, IndexSet};
use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

pub struct RequestResolver<'a> {
    collection: &'a Value,
    path: NodePath,
    root: PathBuf,
    variables: VariableResolver,
}
struct UrlPlan {
    raw: String,
    query: Option<Vec<(String, String)>>,
}
pub(super) struct Context<'a> {
    pub variables: &'a VariableResolver,
    pub unresolved: IndexSet<String>,
    pub cycles: IndexSet<String>,
    resolved_bytes: usize,
}
impl Context<'_> {
    pub fn value(&mut self, value: &str) -> Result<String> {
        let result = self.variables.resolve(value)?;
        if result.value.len() > MAX_RESOLVED_BYTES.saturating_sub(self.resolved_bytes) {
            return Err(invalid(
                "Expanded request values exceed the 64 MiB planning limit",
            ));
        }
        self.resolved_bytes += result.value.len();
        self.unresolved.extend(result.unresolved);
        self.cycles.extend(result.cycles);
        Ok(result.value)
    }
    pub fn check(&self) -> Result<()> {
        if self.unresolved.is_empty() && self.cycles.is_empty() {
            return Ok(());
        }
        let mut message = String::new();
        if !self.unresolved.is_empty() {
            message.push_str("Unresolved variable(s): ");
            message.push_str(
                &self
                    .unresolved
                    .iter()
                    .map(|key| format!("{{{{{key}}}}}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.cycles.is_empty() {
            if !message.is_empty() {
                message.push_str("; ");
            }
            message.push_str("Variable substitution cycle: ");
            message.push_str(
                &self
                    .cycles
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        Err(invalid(&message))
    }
}
impl<'a> RequestResolver<'a> {
    pub fn new(
        collection: &'a Value,
        path: NodePath,
        workspace_root: &Path,
        environment: Option<&Environment>,
    ) -> Result<Self> {
        let variables = VariableResolver::new(collection, &path, environment)?;
        Ok(Self {
            collection,
            path,
            root: workspace_root.into(),
            variables,
        })
    }
    pub fn effective_variables(&self) -> impl Iterator<Item = &Variable> {
        self.variables.effective_variables()
    }
    pub fn resolve(&self, text: &str) -> Result<Resolution> {
        self.variables.resolve(text)
    }
    pub fn effective_headers(&self) -> Result<Vec<HeaderValue>> {
        let nodes = ancestors(self.collection, &self.path)?;
        let mut headers = vec![];
        for (index, node) in nodes.iter().enumerate() {
            let entries = if node.get("request").is_some_and(Value::is_object) {
                node["request"].get("header")
            } else {
                node.get("x-kode-headers")
                    .filter(|value| value.is_array())
                    .or_else(|| node.get("header").filter(|value| value.is_array()))
            };
            let source = if index == 0 {
                "Collection".into()
            } else {
                primitive(node.get("name")).unwrap_or_else(|| {
                    if node.get("request").is_some() {
                        "Request"
                    } else {
                        "Folder"
                    }
                    .into()
                })
            };
            for entry in entries.and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = primitive(entry.get("key"))
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                else {
                    continue;
                };
                headers.push(HeaderValue {
                    name,
                    value: primitive(entry.get("value")).unwrap_or_default(),
                    source: source.clone(),
                    editable: index == nodes.len() - 1,
                    disabled: boolean(entry.get("disabled")),
                });
            }
        }
        Ok(headers)
    }
    pub fn effective_auth(&self) -> Result<Option<Auth>> {
        let mut auth = None;
        for (index, node) in ancestors(self.collection, &self.path)?.iter().enumerate() {
            let source = if index == 0 {
                "Collection".into()
            } else {
                primitive(node.get("name")).unwrap_or_else(|| "Folder".into())
            };
            for definition in [
                node.get("auth"),
                node.get("request").and_then(|request| request.get("auth")),
            ]
            .into_iter()
            .flatten()
            .filter(|auth| auth.is_object())
            {
                auth = Some(Auth {
                    kind: primitive(definition.get("type")).unwrap_or_else(|| "noauth".into()),
                    definition: definition.clone(),
                    source: source.clone(),
                });
            }
        }
        Ok(auth)
    }
    pub fn plan(&self) -> Result<ResolvedRequest> {
        let nodes = ancestors(self.collection, &self.path)?;
        let item = nodes.last().expect("collection ancestor");
        if item
            .get("x-kode-import-incomplete")
            .and_then(Value::as_bool)
            == Some(true)
        {
            return Err(invalid(
                "OpenAPI conversion is incomplete; review import notices and correct the request before clearing x-kode-import-incomplete",
            ));
        }
        let request = match item.get("request") {
            Some(request) if request.is_object() => Cow::Borrowed(request),
            Some(Value::String(url)) => Cow::Owned(serde_json::json!({"method":"GET","url":url})),
            _ => return Err(invalid("Selected node is not a request")),
        };
        let mut context = Context {
            variables: &self.variables,
            unresolved: IndexSet::new(),
            cycles: IndexSet::new(),
            resolved_bytes: 0,
        };
        let method = primitive(request.get("method"))
            .filter(|method| !method.trim().is_empty())
            .unwrap_or_else(|| "GET".into())
            .trim()
            .to_ascii_uppercase();
        http::Method::from_bytes(method.as_bytes())
            .map_err(|_| invalid("Request method must be an HTTP token"))?;
        let UrlPlan {
            raw: raw_url,
            query,
        } = resolve_url(&request, &mut context)?;
        let sse = raw_url
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("sse://"));
        let raw_url = if sse {
            format!("http://{}", &raw_url[6..])
        } else {
            raw_url
        };
        let mut headers = IndexMap::new();
        for header in self
            .effective_headers()?
            .into_iter()
            .filter(|header| !header.disabled)
        {
            let name = context.value(&header.name)?;
            let value = context.value(&header.value)?;
            headers.insert(name.to_ascii_lowercase(), Header { name, value });
        }
        if sse && !headers.contains_key("accept") {
            headers.insert(
                "accept".into(),
                Header {
                    name: "Accept".into(),
                    value: "text/event-stream".into(),
                },
            );
        }
        let auth_query = apply_auth(self.effective_auth()?.as_ref(), &mut context, &mut headers)?;
        let body = super::body::plan(&request, &self.root, &mut context, &mut headers)?;
        context.check()?;
        if raw_url.chars().any(|ch| ch.is_ascii_control() || ch == ' ') {
            return Err(invalid(
                "URL contains an unescaped space or control character",
            ));
        }
        let mut url = url::Url::parse(&raw_url).map_err(|_| invalid("Invalid request URL"))?;
        if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") || url.host_str().is_none() {
            return Err(invalid(
                "Request URL must use http, https, ws or wss with a host",
            ));
        }
        if let Some(query) = query {
            url.set_query(None);
            if !query.is_empty() {
                url.query_pairs_mut()
                    .extend_pairs(query.iter().map(|(key, value)| (key, value)));
            }
        }
        if let Some((key, value)) = auth_query {
            url.query_pairs_mut().append_pair(&key, &value);
        }
        let protocol = if matches!(url.scheme(), "ws" | "wss") {
            Protocol::WebSocket
        } else if sse
            || headers.get("accept").is_some_and(|header| {
                header
                    .value
                    .to_ascii_lowercase()
                    .contains("text/event-stream")
            })
        {
            Protocol::Sse
        } else {
            Protocol::Http
        };
        for header in headers.values() {
            validate_header(header)?;
        }
        let settings = item
            .get("protocolProfileBehavior")
            .filter(|value| value.is_object())
            .or_else(|| self.collection.get("protocolProfileBehavior"));
        let timeout_ms = primitive(settings.and_then(|settings| settings.get("requestTimeout")))
            .and_then(|value| value.parse::<i64>().ok())
            .map_or(30_000, |value| value.max(1) as u64);
        Ok(ResolvedRequest {
            path: self.path.clone(),
            method,
            url: url.into(),
            headers: headers.into_values().collect(),
            body,
            timeout_ms,
            follow_redirects: !boolean(
                settings.and_then(|settings| settings.get("disableRedirects")),
            ),
            ssl_validation: !boolean(
                settings.and_then(|settings| settings.get("disableSslVerification")),
            ),
            protocol,
        })
    }
}
pub(super) fn validate_header(header: &Header) -> Result<()> {
    http::header::HeaderName::from_bytes(header.name.as_bytes())
        .map_err(|_| invalid("Header name must be an HTTP token"))?;
    http::HeaderValue::from_bytes(header.value.as_bytes())
        .map_err(|_| invalid("Header value contains a prohibited control character"))?;
    Ok(())
}
fn auth_value(auth: &Auth, key: &str, context: &mut Context<'_>) -> Result<String> {
    let value = auth
        .definition
        .get(&auth.kind)
        .or_else(|| auth.definition.get(auth.kind.to_ascii_lowercase()))
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| primitive(entry.get("key")).as_deref() == Some(key))
        })
        .and_then(|entry| primitive(entry.get("value")))
        .or_else(|| primitive(auth.definition.get(key)))
        .unwrap_or_default();
    context.value(&value)
}
fn apply_auth(
    auth: Option<&Auth>,
    context: &mut Context<'_>,
    headers: &mut IndexMap<String, Header>,
) -> Result<Option<(String, String)>> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    match auth.kind.to_ascii_lowercase().as_str() {
        "noauth" => {}
        "bearer" => {
            headers.shift_remove("authorization");
            headers.insert(
                "authorization".into(),
                Header {
                    name: "Authorization".into(),
                    value: format!("Bearer {}", auth_value(auth, "token", context)?),
                },
            );
        }
        "basic" => {
            let user = auth_value(auth, "username", context)?;
            let password = auth_value(auth, "password", context)?;
            headers.shift_remove("authorization");
            headers.insert(
                "authorization".into(),
                Header {
                    name: "Authorization".into(),
                    value: format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
                },
            );
        }
        "apikey" => {
            let key = auth_value(auth, "key", context)?;
            let value = auth_value(auth, "value", context)?;
            if auth_value(auth, "in", context)?.eq_ignore_ascii_case("query") {
                return Ok(Some((key, value)));
            }
            headers.shift_remove(&key.to_ascii_lowercase());
            headers.insert(key.to_ascii_lowercase(), Header { name: key, value });
        }
        _ => {
            return Err(invalid(
                "Authentication type is not supported for execution",
            ));
        }
    }
    Ok(None)
}
fn resolve_url(request: &Value, context: &mut Context<'_>) -> Result<UrlPlan> {
    let value = request.get("url").ok_or_else(|| invalid("URL is empty"))?;
    let raw = if value.is_object() {
        primitive(value.get("raw")).unwrap_or_else(|| structured_url(value))
    } else {
        primitive(Some(value)).unwrap_or_default()
    };
    if raw.trim().is_empty() {
        return Err(invalid("URL is empty"));
    }
    let mut resolved = context.value(&raw)?;
    if let Some(variables) = value.get("variable").and_then(Value::as_array) {
        let path_start = resolved
            .find("://")
            .and_then(|start| {
                resolved[start + 3..]
                    .find(['/', '?', '#'])
                    .map(|offset| start + 3 + offset)
            })
            .filter(|offset| resolved.as_bytes()[*offset] == b'/');
        if let Some(path_start) = path_start {
            let end = resolved[path_start..]
                .find(['?', '#'])
                .map_or(resolved.len(), |offset| path_start + offset);
            let mut path = resolved[path_start..end].to_string();
            let mut entries: Vec<_> = variables
                .iter()
                .filter(|entry| !boolean(entry.get("disabled")))
                .filter_map(|entry| primitive(entry.get("key")).map(|key| (key, entry)))
                .collect();
            entries.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
            for (key, entry) in entries {
                let value = context.value(&primitive(entry.get("value")).unwrap_or_default())?;
                path = path_variable(&path, &key, &value)?;
            }
            resolved.replace_range(path_start..end, &path);
        }
    }
    let query = if let Some(entries) = value.get("query").and_then(Value::as_array) {
        let mut pairs = vec![];
        for entry in entries
            .iter()
            .filter(|entry| !boolean(entry.get("disabled")))
        {
            let key = context.value(&primitive(entry.get("key")).unwrap_or_default())?;
            let value = context.value(&primitive(entry.get("value")).unwrap_or_default())?;
            if !key.trim().is_empty() {
                pairs.push((key, value));
            }
        }
        Some(pairs)
    } else {
        None
    };
    Ok(UrlPlan {
        raw: resolved,
        query,
    })
}
fn structured_url(url: &Value) -> String {
    fn join(value: Option<&Value>, separator: &str) -> String {
        match value {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(|value| primitive(Some(value)))
                .collect::<Vec<_>>()
                .join(separator),
            value => primitive(value).unwrap_or_default(),
        }
    }
    let protocol = primitive(url.get("protocol")).unwrap_or_else(|| "http".into());
    let host = join(url.get("host"), ".");
    let path = join(url.get("path"), "/");
    let port = primitive(url.get("port"))
        .filter(|port| !port.is_empty())
        .map_or_else(String::new, |port| format!(":{port}"));
    format!("{protocol}://{host}{port}/{}", path.trim_start_matches('/'))
}
fn path_variable(path: &str, key: &str, value: &str) -> Result<String> {
    if key.is_empty() {
        return Ok(path.into());
    }
    let token = format!(":{key}");
    let mut encoded = String::with_capacity(value.len());
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    let mut out = String::new();
    fn append(out: &mut String, text: &str) -> Result<()> {
        if text.len() > MAX_RESOLVED_BYTES.saturating_sub(out.len()) {
            return Err(invalid(
                "Expanded URL path exceeds the 64 MiB planning limit",
            ));
        }
        out.push_str(text);
        Ok(())
    }
    let mut at = 0;
    while let Some(offset) = path[at..].find(&token) {
        let start = at + offset;
        let end = start + token.len();
        append(&mut out, &path[at..start])?;
        if path[end..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
        {
            append(&mut out, &token)?;
        } else {
            append(&mut out, &encoded)?;
        }
        at = end;
    }
    append(&mut out, &path[at..])?;
    Ok(out)
}
