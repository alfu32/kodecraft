use super::request::Context;
use super::*;
use indexmap::IndexMap;
use kode_platform::jobs::CancellationToken;
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) fn plan(
    request: &Value,
    root: &Path,
    context: &mut Context<'_>,
    headers: &mut IndexMap<String, Header>,
) -> Result<BodyPlan> {
    let Some(body) = request.get("body").filter(|body| body.is_object()) else {
        return Ok(BodyPlan::Empty);
    };
    if boolean(body.get("disabled")) {
        return Ok(BodyPlan::Empty);
    }
    let Some(mode) = primitive(body.get("mode")) else {
        return Ok(BodyPlan::Empty);
    };
    let (plan, content_type) = match mode.to_ascii_lowercase().as_str() {
        "raw" => {
            let language = primitive(
                body.get("options")
                    .and_then(|options| options.get("raw"))
                    .and_then(|raw| raw.get("language")),
            )
            .unwrap_or_default()
            .to_ascii_lowercase();
            let content = match body.get("raw") {
                Some(Value::String(value)) => value.clone(),
                None | Some(Value::Null) => String::new(),
                _ => return Err(invalid("Raw body must be text")),
            };
            let value = context.value(&content)?;
            let content_type = match language.as_str() {
                "json" => "application/json",
                "xml" => "application/xml",
                "html" => "text/html",
                _ => "text/plain",
            };
            (
                BodyPlan::Bytes(Arc::from(value.into_bytes())),
                Some(content_type.to_string()),
            )
        }
        "urlencoded" => {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for field in body
                .get("urlencoded")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|field| !boolean(field.get("disabled")))
            {
                let key = context.value(&primitive(field.get("key")).unwrap_or_default())?;
                let value = context.value(&primitive(field.get("value")).unwrap_or_default())?;
                if !key.trim().is_empty() {
                    serializer.append_pair(&key, &value);
                }
            }
            (
                BodyPlan::Bytes(Arc::from(serializer.finish().into_bytes())),
                Some("application/x-www-form-urlencoded".into()),
            )
        }
        "file" => {
            let source = primitive(body.get("file").and_then(|file| file.get("src")))
                .ok_or_else(|| invalid("Body file source is missing"))?;
            let source = context.value(&source)?;
            if source.is_empty() || source.contains('\0') {
                return Err(invalid("Body file source is empty or invalid"));
            }
            (BodyPlan::File(normalize(root.join(source))), None)
        }
        "formdata" => {
            let boundary = multipart_boundary(headers)?;
            let mut parts = vec![];
            for field in body
                .get("formdata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|field| !boolean(field.get("disabled")))
            {
                let name = context.value(&primitive(field.get("key")).unwrap_or_default())?;
                if name.trim().is_empty() {
                    continue;
                }
                validate_part_name(&name)?;
                let content_type = primitive(field.get("contentType"));
                if let Some(content_type) = &content_type {
                    super::request::validate_header(&Header {
                        name: "Content-Type".into(),
                        value: content_type.clone(),
                    })?;
                }
                match primitive(field.get("type"))
                    .unwrap_or_else(|| "text".into())
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "text" => {
                        let value =
                            context.value(&primitive(field.get("value")).unwrap_or_default())?;
                        parts.push(MultipartPart {
                            name,
                            content: PartContent::Text(value),
                            content_type,
                        });
                    }
                    "file" => {
                        let sources = match field.get("src") {
                            Some(Value::Array(values)) => values
                                .iter()
                                .map(|value| {
                                    value.as_str().map(str::to_string).ok_or_else(|| {
                                        invalid("Multipart file sources must be paths")
                                    })
                                })
                                .collect::<Result<Vec<_>>>()?,
                            Some(Value::String(source)) => vec![source.clone()],
                            None | Some(Value::Null) => {
                                vec![primitive(field.get("value")).unwrap_or_default()]
                            }
                            _ => {
                                return Err(invalid(
                                    "Multipart file source must be a path or path array",
                                ));
                            }
                        };
                        if sources.is_empty() {
                            return Err(invalid("Multipart file source is missing"));
                        }
                        for source in sources {
                            let source = context.value(&source)?;
                            if source.is_empty() || source.contains('\0') {
                                return Err(invalid("Multipart file source is empty or invalid"));
                            }
                            parts.push(MultipartPart {
                                name: name.clone(),
                                content: PartContent::File(normalize(root.join(source))),
                                content_type: content_type.clone(),
                            });
                        }
                    }
                    _ => return Err(invalid("Unsupported multipart field type")),
                }
            }
            (
                BodyPlan::Multipart {
                    boundary: boundary.clone(),
                    parts,
                },
                Some(format!("multipart/form-data; boundary={boundary}")),
            )
        }
        "graphql" => {
            let graph = body
                .get("graphql")
                .ok_or_else(|| invalid("GraphQL body is missing"))?;
            let query = context.value(&primitive(graph.get("query")).unwrap_or_default())?;
            let variables = match graph.get("variables") {
                None => serde_json::json!({}),
                Some(Value::String(text)) if text.trim().is_empty() => serde_json::json!({}),
                Some(Value::String(text)) => serde_json::from_str(text)
                    .map_err(|_| invalid("GraphQL variables must be valid JSON"))?,
                Some(value) => value.clone(),
            };
            if !variables.is_object() && !variables.is_null() {
                return Err(invalid("GraphQL variables must be a JSON object or null"));
            }
            let variables = resolve_json(&variables, context, 0)?;
            let value = serde_json::json!({"query":query,"variables":variables});
            let bytes =
                serde_json::to_vec(&value).map_err(|_| invalid("Cannot encode GraphQL body"))?;
            (
                BodyPlan::Bytes(Arc::from(bytes)),
                Some("application/json".into()),
            )
        }
        _ => return Err(invalid("Request body mode is not supported")),
    };
    if let Some(value) = content_type {
        headers.entry("content-type".into()).or_insert(Header {
            name: "Content-Type".into(),
            value,
        });
    }
    Ok(plan)
}
fn resolve_json(value: &Value, context: &mut Context<'_>, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err(invalid("GraphQL variables exceed 64 levels"));
    }
    Ok(match value {
        Value::String(value) => Value::String(context.value(value)?),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| resolve_json(value, context, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| Ok((key.clone(), resolve_json(value, context, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
        value => value.clone(),
    })
}
fn multipart_boundary(headers: &mut IndexMap<String, Header>) -> Result<String> {
    let generated = || format!("kode-{}", uuid::Uuid::new_v4());
    let Some(header) = headers.get_mut("content-type") else {
        return Ok(generated());
    };
    let media_type: mime::Mime = header
        .value
        .parse()
        .map_err(|_| invalid("Invalid multipart Content-Type"))?;
    if !media_type
        .essence_str()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        return Ok(generated());
    }
    if let Some(boundary) = media_type.get_param(mime::BOUNDARY) {
        let boundary = boundary.as_str();
        if boundary.is_empty()
            || boundary.len() > 70
            || boundary.ends_with(' ')
            || !boundary
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"'()+_,-./:=? ".contains(&byte))
        {
            return Err(invalid("Multipart boundary is invalid"));
        }
        return Ok(boundary.to_string());
    }
    let boundary = generated();
    header.value.push_str(&format!("; boundary={boundary}"));
    Ok(boundary)
}
fn normalize(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_some_and(|name| name != "..") {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push("..");
                }
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
fn validate_part_name(name: &str) -> Result<()> {
    if name.chars().any(|ch| ch.is_ascii_control()) {
        return Err(invalid("Multipart names cannot contain control characters"));
    }
    Ok(())
}
fn quoted(value: &str) -> Result<String> {
    validate_part_name(value)?;
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn push(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<()> {
    if output.len().saturating_add(bytes.len()) > limit {
        return Err(invalid("Request body exceeds the caller's buffering limit"));
    }
    output.extend_from_slice(bytes);
    Ok(())
}
fn file(output: &mut Vec<u8>, path: &Path, limit: usize, cancel: &CancellationToken) -> Result<()> {
    let mut source = kode_platform::files::open_regular_file(path)
        .map_err(|_| invalid("Request body file cannot be opened as a regular file"))?;
    let before = source
        .metadata()
        .map_err(|_| invalid("Cannot inspect request body file"))?;
    if before.len() > limit.saturating_sub(output.len()) as u64 {
        return Err(invalid("Request file exceeds the caller's buffering limit"));
    }
    let mut buffer = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let read = source
            .read(&mut buffer)
            .map_err(|_| invalid("Cannot read request body file"))?;
        if read == 0 {
            break;
        }
        push(output, &buffer[..read], limit)?;
    }
    let after = source
        .metadata()
        .map_err(|_| invalid("Cannot inspect request body file after reading"))?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(invalid(
            "Request body file changed while being read; retry explicitly",
        ));
    }
    Ok(())
}
impl BodyPlan {
    /// Explicit blocking-worker operation. Call before networking; pure planning
    /// never reads files. The caller selects its buffering/preview budget.
    pub fn materialize(&self, limit: usize, cancel: &CancellationToken) -> Result<Vec<u8>> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let mut output = vec![];
        match self {
            Self::Empty => {}
            Self::Bytes(bytes) => push(&mut output, bytes, limit)?,
            Self::File(path) => file(&mut output, path, limit, cancel)?,
            Self::Multipart { boundary, parts } => {
                for part in parts {
                    if cancel.is_cancelled() {
                        return Err(DomainError::Cancelled);
                    }
                    push(
                        &mut output,
                        format!(
                            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"",
                            quoted(&part.name)?
                        )
                        .as_bytes(),
                        limit,
                    )?;
                    match &part.content {
                        PartContent::Text(_) => {}
                        PartContent::File(path) => {
                            let name = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .ok_or_else(|| invalid("Multipart filename must be valid UTF-8"))?;
                            push(
                                &mut output,
                                format!("; filename=\"{}\"", quoted(name)?).as_bytes(),
                                limit,
                            )?;
                        }
                    }
                    push(&mut output, b"\r\n", limit)?;
                    if let Some(content_type) = &part.content_type {
                        push(
                            &mut output,
                            format!("Content-Type: {content_type}\r\n").as_bytes(),
                            limit,
                        )?;
                    }
                    push(&mut output, b"\r\n", limit)?;
                    let body_start = output.len();
                    match &part.content {
                        PartContent::Text(text) => push(&mut output, text.as_bytes(), limit)?,
                        PartContent::File(path) => file(&mut output, path, limit, cancel)?,
                    }
                    let payload = &output[body_start..];
                    let delimiter = format!("--{boundary}");
                    let line_delimiter = format!("\r\n{delimiter}");
                    if payload.starts_with(delimiter.as_bytes())
                        || payload
                            .windows(line_delimiter.len())
                            .any(|window| window == line_delimiter.as_bytes())
                    {
                        return Err(invalid("Multipart boundary occurs inside a part"));
                    }
                    push(&mut output, b"\r\n", limit)?;
                }
                push(&mut output, format!("--{boundary}--\r\n").as_bytes(), limit)?;
            }
        }
        Ok(output)
    }
}
impl ResolvedRequest {
    pub fn materialize(self, limit: usize, cancel: &CancellationToken) -> Result<PreparedRequest> {
        let body = Arc::from(self.body.materialize(limit, cancel)?);
        Ok(PreparedRequest {
            request: self,
            body,
        })
    }
}
