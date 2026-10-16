use super::{Extra, yes};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnvironmentValue {
    pub key: String,
    pub value: Value,
    pub enabled: bool,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: Option<Value>,
    pub secret: bool,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for EnvironmentValue {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: Value::String(String::new()),
            enabled: yes(),
            kind: "default".into(),
            description: None,
            secret: false,
            extra: Extra::new(),
        }
    }
}
impl EnvironmentValue {
    pub fn text(&self) -> String {
        match &self.value {
            Value::String(value) => value.clone(),
            value => value.to_string(),
        }
    }
}
impl std::fmt::Debug for EnvironmentValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentValue")
            .field("enabled", &self.enabled)
            .field("secret", &self.secret)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub values: Vec<EnvironmentValue>,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: "New environment".into(),
            values: vec![],
            extra: Extra::new(),
        }
    }
}
impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("entries", &self.values.len())
            .finish_non_exhaustive()
    }
}
/// Source-compatible line codec: trim lines/keys/values, skip # comments and
/// invalid/empty keys, escape only = and backslash. Structured edits preserve
/// flags/descriptions and can represent values outside this text syntax.
pub fn parse(text: &str) -> Vec<EnvironmentValue> {
    text.split(['\n', '\r'])
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let mut escaped = false;
            let mut separator = None;
            for (index, ch) in line.char_indices() {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '=' {
                    separator = Some(index);
                    break;
                }
            }
            let index = separator?;
            let key = unescape(line[..index].trim());
            if key.trim().is_empty() {
                return None;
            }
            Some(EnvironmentValue {
                key,
                value: Value::String(unescape(line[index + 1..].trim())),
                ..Default::default()
            })
        })
        .collect()
}
pub fn format(values: &[EnvironmentValue]) -> String {
    values
        .iter()
        .map(|value| format!("{}={}", escape(&value.key), escape(&value.text())))
        .collect::<Vec<_>>()
        .join("\n")
}
fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if matches!(ch, '=' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            if !matches!(ch, '=' | '\\') {
                out.push('\\');
            }
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}
