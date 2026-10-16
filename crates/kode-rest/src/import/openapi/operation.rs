use super::*;
use kode_core::Result;
fn array(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
fn text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}
fn language(media: &str) -> &str {
    let subtype = media
        .split(';')
        .next()
        .unwrap_or(media)
        .split('/')
        .nth(1)
        .unwrap_or("text");
    if subtype == "json" || subtype.ends_with("+json") {
        "json"
    } else if subtype == "xml" || subtype.ends_with("+xml") {
        "xml"
    } else if ["javascript", "x-javascript"].contains(&subtype) {
        "javascript"
    } else if subtype == "html" {
        "html"
    } else {
        "text"
    }
}
impl Converter<'_> {
    fn base_url(&mut self, operation: &Value, item: &Value, path: &str) -> String {
        if let Some(servers) = operation
            .get("servers")
            .or_else(|| item.get("servers"))
            .or_else(|| self.document.get("servers"))
        {
            if let Some(servers) = servers.as_array().filter(|servers| !servers.is_empty()) {
                if servers.len() > 1 {
                    self.notice(
                        path,
                        "server-choice",
                        "First server selected; alternative servers retained in source metadata",
                        false,
                    );
                }
                let server = &servers[0];
                if let Some(url) = server.get("url").and_then(Value::as_str) {
                    let mut url = url.to_string();
                    if let Some(variables) = server.get("variables").and_then(Value::as_object) {
                        for (key, variable) in variables {
                            if let Some(default) = primitive(variable.get("default")) {
                                url = url.replace(&format!("{{{key}}}"), &default);
                            } else {
                                self.notice(
                                    path,
                                    "server-variable",
                                    "Server variable is missing its default",
                                    true,
                                );
                            }
                        }
                    }
                    if url.contains('{')
                        || !(url.starts_with("https://") || url.starts_with("http://"))
                    {
                        self.notice(path, "relative-server", "Server URL is relative or has unresolved variables; supply an absolute URL", true);
                    }
                    return url;
                }
            }
            self.notice(
                path,
                "invalid-servers",
                "Servers must contain an object with a URL",
                true,
            );
        }
        let scheme = self
            .document
            .get("schemes")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .unwrap_or("https");
        let host = self
            .document
            .get("host")
            .and_then(Value::as_str)
            .unwrap_or("localhost");
        let base = self
            .document
            .get("basePath")
            .and_then(Value::as_str)
            .unwrap_or("");
        format!("{scheme}://{host}{base}")
    }
    pub(super) fn operation(
        &mut self,
        path: &str,
        method: &str,
        operation: &Value,
        item: &Value,
        source: &str,
    ) -> Result<Value> {
        let base = self.base_url(operation, item, source);
        let mut parameters: IndexMap<(String, String), Value> = IndexMap::new();
        for (owner, location) in [
            (
                item,
                source.rsplit_once('/').map_or(source, |(parent, _)| parent),
            ),
            (operation, source),
        ] {
            if owner.get("parameters").is_some_and(|v| !v.is_array()) {
                return Err(invalid(
                    &pointer(location, "parameters"),
                    "parameters must be an array",
                ));
            }
            for (index, parameter) in array(owner, "parameters").iter().enumerate() {
                let at = pointer(&pointer(location, "parameters"), &index.to_string());
                let Some(parameter) = self.resolve(parameter, &at) else {
                    continue;
                };
                let name = parameter
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid(&at, "parameter requires a name"))?;
                let kind = parameter
                    .get("in")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid(&at, "parameter requires an in location"))?;
                parameters.insert((kind.into(), name.into()), parameter);
            }
        }
        let mut query = vec![];
        let mut headers = vec![];
        let mut variables = vec![];
        for ((kind, name), parameter) in &parameters {
            if kind == "body" || kind == "formData" {
                continue;
            }
            let value = self
                .example(parameter, source)
                .or_else(|| {
                    parameter
                        .get("schema")
                        .map(|schema| self.sample(schema, source, 0))
                        .or_else(|| parameter.get("default").cloned())
                })
                .unwrap_or_else(|| Value::String(String::new()));
            let expected_style = if kind == "query" || kind == "cookie" {
                "form"
            } else {
                "simple"
            };
            if parameter
                .get("style")
                .and_then(Value::as_str)
                .is_some_and(|style| style != expected_style)
                || parameter.get("content").is_some()
                || parameter.get("allowReserved").and_then(Value::as_bool) == Some(true)
            {
                self.notice(
                    source,
                    "parameter-serialization",
                    "Parameter style/content/allowReserved needs explicit serialization review",
                    true,
                );
            }
            let values = if let Some(values) = value.as_array() {
                if values.iter().any(|v| v.is_object() || v.is_array()) {
                    self.notice(
                        source,
                        "parameter-serialization",
                        "Nested parameter values require explicit serialization",
                        true,
                    );
                }
                let explode = parameter
                    .get("explode")
                    .and_then(Value::as_bool)
                    .unwrap_or(kind == "query");
                let collection_format = parameter.get("collectionFormat").and_then(Value::as_str);
                if kind == "query"
                    && (collection_format == Some("multi")
                        || (collection_format.is_none() && explode))
                {
                    values.iter().map(text).collect::<Vec<_>>()
                } else {
                    let separator = match collection_format {
                        Some("ssv") => " ",
                        Some("tsv") => "\t",
                        Some("pipes") => "|",
                        _ => ",",
                    };
                    vec![values.iter().map(text).collect::<Vec<_>>().join(separator)]
                }
            } else {
                if value.is_object() {
                    self.notice(
                        source,
                        "parameter-serialization",
                        "Object parameter values require explicit serialization",
                        true,
                    );
                }
                vec![text(&value)]
            };
            for value in values {
                let row = json!({"key": name, "value": value});
                match kind.as_str() {
                    "path" => variables.push(row), "query" => query.push(row), "header" => headers.push(row),
                    _ => self.notice(source, "parameter-location", "Cookie or unknown parameter location is retained but not automatically converted", true),
                }
            }
        }
        let body = self.body(operation, &parameters, source, &mut headers);
        let auth = self.authentication(operation, source);
        static PATH_TEMPLATE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(r"\{([^{}]+)\}").expect("constant path pattern")
        });
        let path = PATH_TEMPLATE.replace_all(path, ":$1");
        let raw = format!(
            "{}/{}",
            base.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut url = json!({"raw": raw});
        if !query.is_empty() {
            url["query"] = Value::Array(query);
        }
        if !variables.is_empty() {
            url["variable"] = Value::Array(variables);
        }
        let mut request =
            json!({"method": method.to_ascii_uppercase(), "header": headers, "url": url});
        if let Some(body) = body {
            request["body"] = body;
        }
        if let Some(auth) = auth {
            request["auth"] = auth;
        }
        let name = operation
            .get("summary")
            .and_then(Value::as_str)
            .or_else(|| operation.get("operationId").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase()));
        let mut result = json!({"id": uuid::Uuid::new_v4().to_string(), "name": name, "request": request, "response": [], "x-kode-openapi-operation": source});
        if self.incomplete {
            result["x-kode-import-incomplete"] = Value::Bool(true);
        }
        Ok(result)
    }
    fn body(
        &mut self,
        operation: &Value,
        parameters: &IndexMap<(String, String), Value>,
        path: &str,
        headers: &mut Vec<Value>,
    ) -> Option<Value> {
        if let Some(body) = operation.get("requestBody") {
            let body = self.resolve(body, &pointer(path, "requestBody"))?;
            let Some(content) = body.get("content").and_then(Value::as_object) else {
                self.notice(
                    path,
                    "request-body",
                    "Request body requires a content object",
                    true,
                );
                return None;
            };
            let Some((media_type, media)) = content.iter().next() else {
                self.notice(path, "request-body", "Request body has no media type", true);
                return None;
            };
            if content.len() > 1 {
                self.notice(path, "media-choice", "First request body media type selected; alternatives retained in source metadata", false);
            }
            if media.get("encoding").is_some()
                || media_type.starts_with("multipart/")
                || media_type == "application/x-www-form-urlencoded"
            {
                self.notice(path, "body-encoding", "Form or multipart media schema needs explicit body encoding; generated raw sample requires review", true);
            }
            let at = pointer(
                &pointer(&pointer(path, "requestBody"), "content"),
                media_type,
            );
            let value = self
                .example(media, &at)
                .or_else(|| {
                    media
                        .get("schema")
                        .map(|schema| self.sample(schema, &pointer(&at, "schema"), 0))
                })
                .unwrap_or_else(|| Value::String(String::new()));
            return Some(Self::raw_body(value, media_type, headers));
        }
        let media = operation
            .get("consumes")
            .or_else(|| self.document.get("consumes"))
            .and_then(Value::as_array)
            .and_then(|types| types.first())
            .and_then(Value::as_str)
            .unwrap_or("application/json");
        if let Some(parameter) = parameters
            .iter()
            .find(|((kind, _), _)| kind == "body")
            .map(|(_, value)| value)
        {
            let value = self
                .example(parameter, path)
                .or_else(|| {
                    parameter
                        .get("schema")
                        .map(|schema| self.sample(schema, path, 0))
                })
                .unwrap_or_else(|| json!({}));
            return Some(Self::raw_body(value, media, headers));
        }
        let form: Vec<_> = parameters
            .iter()
            .filter(|((kind, _), _)| kind == "formData")
            .collect();
        if form.is_empty() {
            return None;
        }
        let multipart = media.starts_with("multipart/");
        let mut rows = vec![];
        for ((_, name), parameter) in form {
            if parameter.get("type").and_then(Value::as_str) == Some("file") {
                self.notice(
                    path,
                    "file-placeholder",
                    "Imported file field requires choosing a local file before sending",
                    true,
                );
                rows.push(json!({"key":name,"type":"file","src":""}));
            } else {
                let value = self
                    .example(parameter, path)
                    .or_else(|| parameter.get("default").cloned())
                    .unwrap_or_else(|| Value::String(String::new()));
                rows.push(json!({"key":name,"type":"text","value":text(&value)}));
            }
        }
        if multipart {
            Some(json!({"mode":"formdata","formdata":rows}))
        } else {
            if media != "application/x-www-form-urlencoded" {
                self.notice(
                    path,
                    "form-media",
                    "Swagger form fields need form-urlencoded or multipart media",
                    true,
                );
            }
            Some(json!({"mode":"urlencoded","urlencoded":rows}))
        }
    }
    fn raw_body(value: Value, media: &str, headers: &mut Vec<Value>) -> Value {
        let grammar = language(media);
        // String JSON examples must remain JSON strings, not unquoted payloads.
        let raw = if grammar == "json" {
            value.to_string()
        } else {
            text(&value)
        };
        headers.push(json!({"key":"Content-Type","value":media}));
        json!({"mode":"raw","raw":raw,"options":{"raw":{"language":grammar}}})
    }
}
