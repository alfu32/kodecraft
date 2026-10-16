//! Keep YAML numeric spelling until conversion to arbitrary-precision JSON.
//! deserialize_any in the selected parser uses f64 outside the i64/u64 range;
//! source spans let us avoid rounding decimal examples and extension data.
use super::invalid;
use kode_core::Result;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use serde_saphyr::Spanned;
use std::{fmt, str::FromStr};
pub(super) enum Node {
    Scalar(Value),
    Array(Vec<Spanned<Node>>),
    Object(MapLike),
}
type MapLike = Vec<(String, Spanned<Node>)>;
impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct NodeVisitor;
        impl<'de> Visitor<'de> for NodeVisitor {
            type Value = Node;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON-compatible YAML value")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(Value::Bool(value)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(Value::Null))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(Value::Number(
                    Number::from_f64(value).ok_or_else(|| E::custom("non-finite number"))?,
                )))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Node, E> {
                Ok(Node::Scalar(value.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Node, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Node::Array(values))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Node, A::Error> {
                let mut values = Vec::new();
                let mut keys = std::collections::HashSet::new();
                while let Some((key, value)) = map.next_entry::<String, Spanned<Node>>()? {
                    if !keys.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate JSON map key"));
                    }
                    values.push((key, value));
                }
                Ok(Node::Object(values))
            }
        }
        deserializer.deserialize_any(NodeVisitor)
    }
}
pub(super) fn into_json(node: Spanned<Node>, input: &str) -> Result<Value> {
    Ok(match node.value {
        Node::Scalar(Value::Number(fallback)) => {
            let span = node.defined.span();
            let raw = span
                .byte_offset()
                .zip(span.byte_len())
                .and_then(|(offset, length)| {
                    input.get(offset as usize..offset.saturating_add(length) as usize)
                })
                .ok_or_else(|| invalid("/", "cannot retain YAML number source spelling"))?;
            let normalized = raw.trim().trim_start_matches('+').replace('_', "");
            match Number::from_str(&normalized) {
                Ok(number) => Value::Number(number),
                // Other YAML numeric syntaxes (e.g. 0xFF) must have an exact
                // integer result. Never fall back to a rounded float.
                Err(_) if fallback.is_i64() || fallback.is_u64() => Value::Number(fallback),
                Err(_) => {
                    return Err(invalid(
                        "/",
                        "YAML number cannot be represented exactly; use decimal JSON syntax",
                    ));
                }
            }
        }
        Node::Scalar(value) => value,
        Node::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| into_json(value, input))
                .collect::<Result<_>>()?,
        ),
        Node::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| Ok((key, into_json(value, input)?)))
                .collect::<Result<Map<_, _>>>()?,
        ),
    })
}
