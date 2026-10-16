use super::*;
fn auth(kind: &str, entries: &[(&str, &str)]) -> Value {
    let mut value = json!({"type":kind});
    value[kind] = Value::Array(
        entries
            .iter()
            .map(|(key, value)| json!({"key":key,"value":value,"type":"string"}))
            .collect(),
    );
    value
}
impl Converter<'_> {
    pub(super) fn authentication(&mut self, operation: &Value, path: &str) -> Option<Value> {
        let Some(security) = operation
            .get("security")
            .or_else(|| self.document.get("security"))
        else {
            // An unsecured OpenAPI operation must not acquire destination-folder
            // authentication merely because it was imported there.
            return Some(json!({"type":"noauth"}));
        };
        let Some(requirements) = security.as_array() else {
            self.notice(
                path,
                "invalid-security",
                "Security requirements must be an array",
                true,
            );
            return Some(json!({"type":"openapi-unsupported"}));
        };
        if requirements.is_empty() {
            return Some(json!({"type":"noauth"}));
        }
        if requirements.len() > 1 {
            self.notice(path, "security-alternatives", "First authentication alternative selected; other alternatives retained in source metadata", false);
        }
        let Some(requirement) = requirements[0].as_object() else {
            self.notice(
                path,
                "invalid-security",
                "Security requirement must be an object",
                true,
            );
            return Some(json!({"type":"openapi-unsupported"}));
        };
        if requirement.is_empty() {
            return Some(json!({"type":"noauth"}));
        }
        if requirement.len() != 1 {
            self.notice(
                path,
                "combined-security",
                "Combined security schemes cannot be represented by one Postman auth object",
                true,
            );
            return Some(json!({"type":"openapi-unsupported"}));
        }
        let name = requirement.keys().next().expect("one requirement");
        let schema = self
            .document
            .pointer("/components/securitySchemes")
            .or_else(|| self.document.get("securityDefinitions"))
            .and_then(|schemes| schemes.get(name));
        let Some(schema) = schema else {
            self.notice(
                path,
                "missing-security-scheme",
                "Security scheme definition is missing",
                true,
            );
            return Some(json!({"type":"openapi-unsupported"}));
        };
        let Some(schema) = self.resolve(schema, path) else {
            return Some(json!({"type":"openapi-unsupported"}));
        };
        let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
        let result = match kind {
            "basic" => auth(
                "basic",
                &[("username", "{{username}}"), ("password", "{{password}}")],
            ),
            "http" => match schema
                .get("scheme")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "basic" => auth(
                    "basic",
                    &[("username", "{{username}}"), ("password", "{{password}}")],
                ),
                "bearer" => auth("bearer", &[("token", "{{bearerToken}}")]),
                _ => return Some(self.unsupported_auth(path)),
            },
            "apiKey" => {
                let location = schema.get("in").and_then(Value::as_str).unwrap_or("");
                let name = schema
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty());
                if !["header", "query"].contains(&location) || name.is_none() {
                    return Some(self.unsupported_auth(path));
                }
                auth(
                    "apikey",
                    &[
                        ("key", name.expect("checked name")),
                        ("value", "{{apiKey}}"),
                        ("in", location),
                    ],
                )
            }
            _ => return Some(self.unsupported_auth(path)),
        };
        self.notice(path, "auth-placeholder", "Authentication mapped to unresolved credential variables; supply values before sending", false);
        Some(result)
    }
    fn unsupported_auth(&mut self, path: &str) -> Value {
        self.notice(path, "unsupported-security-scheme", "OAuth2, OpenID Connect, mutual TLS, cookie API keys or this HTTP auth scheme require explicit setup", true);
        json!({"type":"openapi-unsupported"})
    }
}
