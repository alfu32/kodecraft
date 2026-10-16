use super::*;
use crate::model::Environment;
use indexmap::{IndexMap, IndexSet};
use regex::Regex;
use std::sync::OnceLock;
#[derive(Clone, PartialEq, Eq)]
pub struct Variable {
    pub key: String,
    pub value: String,
    pub kind: String,
    pub source: String,
    pub description: Option<String>,
    pub secret: bool,
}
impl std::fmt::Debug for Variable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestVariable")
            .field("secret", &self.secret)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Resolution {
    pub value: String,
    pub unresolved: IndexSet<String>,
    pub cycles: IndexSet<String>,
}
impl std::fmt::Debug for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VariableResolution")
            .field("unresolved", &self.unresolved.len())
            .field("cycles", &self.cycles.len())
            .finish_non_exhaustive()
    }
}
pub struct VariableResolver {
    values: IndexMap<String, Variable>,
}
impl VariableResolver {
    pub fn new(
        collection: &Value,
        path: &NodePath,
        environment: Option<&Environment>,
    ) -> Result<Self> {
        let mut values = IndexMap::new();
        for (index, node) in ancestors(collection, path)?.into_iter().enumerate() {
            let source = if index == 0 {
                "Collection".into()
            } else {
                primitive(node.get("name")).unwrap_or_else(|| "Folder".into())
            };
            for entry in node
                .get("variable")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(key) = primitive(entry.get("key")).filter(|key| !key.trim().is_empty())
                else {
                    continue;
                };
                if boolean(entry.get("disabled")) {
                    continue;
                }
                let value = entry.get("value").map_or_else(String::new, |value| {
                    primitive(Some(value)).unwrap_or_else(|| value.to_string())
                });
                let kind = primitive(entry.get("type")).unwrap_or_else(|| "string".into());
                values.insert(
                    key.clone(),
                    Variable {
                        key,
                        value,
                        source: source.clone(),
                        secret: kind == "secret" || boolean(entry.get("secret")),
                        kind,
                        description: primitive(entry.get("description")),
                    },
                );
            }
        }
        if let Some(environment) = environment {
            for entry in environment.values.iter().filter(|entry| entry.enabled) {
                values.insert(
                    entry.key.clone(),
                    Variable {
                        key: entry.key.clone(),
                        value: entry.text(),
                        kind: entry.kind.clone(),
                        source: "Environment".into(),
                        description: primitive(entry.description.as_ref()),
                        secret: entry.secret || entry.kind == "secret",
                    },
                );
            }
        }
        Ok(Self { values })
    }
    pub fn effective_variables(&self) -> impl Iterator<Item = &Variable> {
        self.values.values()
    }
    pub fn resolve(&self, text: &str) -> Result<Resolution> {
        let mut resolution = Resolution {
            value: String::new(),
            unresolved: IndexSet::new(),
            cycles: IndexSet::new(),
        };
        let mut budget = 10_000;
        resolution.value = self.replace(text, &mut Vec::new(), &mut resolution, &mut budget)?;
        Ok(resolution)
    }
    fn replace(
        &self,
        text: &str,
        stack: &mut Vec<String>,
        result: &mut Resolution,
        budget: &mut usize,
    ) -> Result<String> {
        static PATTERN: OnceLock<Regex> = OnceLock::new();
        let pattern = PATTERN
            .get_or_init(|| Regex::new(r"\{\{([^{}]+)}}").expect("constant variable pattern"));
        if stack.len() > 64 || text.len() > 4 * 1024 * 1024 {
            return Err(invalid(
                "Variable expansion exceeds the 64-level or 4 MiB limit",
            ));
        }
        let mut output = String::new();
        let mut at = 0;
        for capture in pattern.captures_iter(text) {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| invalid("Variable expansion exceeds 10,000 substitutions"))?;
            let matched = capture.get(0).expect("variable match");
            output.push_str(&text[at..matched.start()]);
            let name = capture[1].trim();
            let value = if let Some(variable) = self.values.get(name) {
                if stack.iter().any(|parent| parent == name) {
                    result.cycles.insert(name.into());
                    format!("{{{{{name}}}}}")
                } else {
                    stack.push(name.into());
                    let value = self.replace(&variable.value, stack, result, budget)?;
                    stack.pop();
                    value
                }
            } else {
                result.unresolved.insert(name.into());
                format!("{{{{{name}}}}}")
            };
            if output.len().saturating_add(value.len()) > 4 * 1024 * 1024 {
                return Err(invalid("Resolved value exceeds 4 MiB"));
            }
            output.push_str(&value);
            at = matched.end();
        }
        if output.len().saturating_add(text.len() - at) > 4 * 1024 * 1024 {
            return Err(invalid("Resolved value exceeds 4 MiB"));
        }
        output.push_str(&text[at..]);
        Ok(output)
    }
}
