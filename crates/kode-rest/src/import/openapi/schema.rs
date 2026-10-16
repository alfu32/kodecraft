use super::*;
impl Converter<'_> {
    pub(super) fn example(&mut self, value: &Value, path: &str) -> Option<Value> {
        if let Some(example) = value.get("example") {
            return Some(example.clone());
        }
        if let Some(examples) = value.get("examples") {
            if let Some(examples) = examples.as_object() {
                if let Some((key, example)) = examples.iter().next() {
                    let example =
                        self.resolve(example, &pointer(&pointer(path, "examples"), key))?;
                    if let Some(value) = example.get("value") {
                        return Some(value.clone());
                    }
                    if example.get("externalValue").is_some() {
                        self.notice(
                            path,
                            "external-example",
                            "External example retained without fetching it",
                            false,
                        );
                    }
                }
            } else if let Some(examples) = examples.as_array() {
                return examples.first().cloned();
            }
        }
        None
    }
    pub(super) fn sample(&mut self, schema: &Value, path: &str, depth: usize) -> Value {
        self.work += 1;
        if depth > 24 || self.work > 10_000 {
            self.notice(
                path,
                "sample-limit",
                "Schema sample depth/work limit reached; supply an explicit body example",
                true,
            );
            return Value::Null;
        }
        let Some(schema) = self.resolve(schema, path) else {
            return Value::Null;
        };
        if let Some(example) = self.example(&schema, path) {
            return example;
        }
        if let Some(value) = schema.get("default").or_else(|| schema.get("const")) {
            return value.clone();
        }
        if let Some(value) = schema
            .get("enum")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
        {
            return value.clone();
        }
        if schema.is_boolean()
            || ["oneOf", "anyOf", "allOf", "not", "if", "$dynamicRef"]
                .iter()
                .any(|key| schema.get(key).is_some())
        {
            self.notice(
                path,
                "unsupported-schema-sample",
                "Schema cannot supply an unambiguous sample; supply an explicit body example",
                true,
            );
            return Value::Null;
        }
        let kind = schema.get("type");
        let kind = kind
            .and_then(Value::as_str)
            .or_else(|| {
                kind.and_then(Value::as_array).and_then(|types| {
                    types
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|kind| *kind != "null")
                })
            })
            .unwrap_or(if schema.get("properties").is_some() {
                "object"
            } else {
                "string"
            });
        match kind {
            "object" => {
                let properties = schema.get("properties").and_then(Value::as_object);
                let mut result = serde_json::Map::new();
                for (key, child) in properties.into_iter().flatten() {
                    result.insert(
                        key.clone(),
                        self.sample(
                            child,
                            &pointer(&pointer(path, "properties"), key),
                            depth + 1,
                        ),
                    );
                }
                Value::Object(result)
            }
            "array" => Value::Array(vec![]),
            "boolean" => Value::Bool(false),
            "integer" | "number" => json!(0),
            "null" => Value::Null,
            _ => Value::String(String::new()),
        }
    }
}
