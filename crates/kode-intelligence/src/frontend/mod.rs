//! Native parser frontends emit unresolved file facts; resolution is separate.
pub mod c;
mod expressions;
pub mod generic;
pub mod kotlin;
pub mod legacy;
pub mod registry;

fn empty_delta(
    path: &str,
    language: &str,
    text: &str,
    version: u64,
) -> crate::model::FileSemanticDelta {
    use crate::{ids::SemanticIds, model::*};
    FileSemanticDelta {
        file: FileRecord {
            id: SemanticIds::file(path),
            path: path.into(),
            language_id: language.into(),
            content_hash: SemanticIds::content_hash(text),
            parse_version: version,
            semantic_version: version,
            dependency_generation: 0,
        },
        scopes: vec![],
        symbols: vec![],
        occurrences: vec![],
        relations: vec![],
        unresolved_types: vec![],
        imports: vec![],
        type_hints: vec![],
        expression_types: vec![],
        lexical_tokens: vec![],
        syntax_expressions: vec![],
        exported_surface_hash: String::new(),
    }
}

/// Pick the occurrence denoting a receiver's value, excluding call arguments and
/// index expressions. The resolver can inspect the enclosing expression later.
fn receiver_node(mut node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    for _ in 0..256 {
        if matches!(
            node.kind(),
            "identifier"
                | "simple_identifier"
                | "interpolated_identifier"
                | "field_identifier"
                | "type_identifier"
                | "this_expression"
                | "super_expression"
        ) {
            return Some(node);
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        node = match node.kind() {
            "navigation_expression" | "field_expression" => node
                .child_by_field_name("field")
                .or_else(|| children.last().copied())?,
            "call_expression" => node
                .child_by_field_name("function")
                .or_else(|| children.first().copied())?,
            "navigation_suffix" => *children.first()?,
            "parenthesized_expression" => *children.first()?,
            "pointer_expression" | "subscript_expression" => {
                node.child_by_field_name("argument")?
            }
            "cast_expression" => node.child_by_field_name("value")?,
            _ => return None,
        };
    }
    None
}

use kode_core::{ByteOffset, Result, TextRange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageCompletionContext {
    pub member_access: bool,
    pub receiver_range: Option<TextRange>,
    pub prefix: String,
}
/// Source-compatible Kotlin receiver scan, operating only on scalar boundaries.
pub fn kotlin_completion_context(
    text: &str,
    offset: ByteOffset,
) -> Result<LanguageCompletionContext> {
    TextRange::new(offset, offset)?.validate(text)?;
    let before = &text[..offset.0];
    let prefix_start = before
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '_')
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let prefix = before[prefix_start..].to_owned();
    let dot = before[..prefix_start].trim_end();
    if !dot.ends_with('.') {
        return Ok(LanguageCompletionContext {
            member_access: false,
            receiver_range: None,
            prefix,
        });
    }
    let receiver = dot[..dot.len() - 1]
        .trim_end()
        .trim_end_matches('?')
        .trim_end();
    let end = receiver.len();
    let (mut parens, mut brackets, mut start) = (0usize, 0usize, 0usize);
    for (index, ch) in receiver.char_indices().rev() {
        let stop = match ch {
            ')' => {
                parens += 1;
                false
            }
            '(' => {
                if parens > 0 {
                    parens -= 1;
                    false
                } else {
                    true
                }
            }
            ']' => {
                brackets += 1;
                false
            }
            '[' => {
                if brackets > 0 {
                    brackets -= 1;
                    false
                } else {
                    true
                }
            }
            '\n' | '\r' | ';' | ',' | '=' | '{' | '}' | '+' | '-' | '*' | '/' | '%' | '&' | '|'
            | '!' => parens == 0 && brackets == 0,
            _ => false,
        };
        if stop {
            start = index + ch.len_utf8();
            break;
        }
    }
    start += receiver[start..].len() - receiver[start..].trim_start().len();
    let range = (start < end)
        .then(|| TextRange::new(ByteOffset(start), ByteOffset(end)))
        .transpose()?;
    Ok(LanguageCompletionContext {
        member_access: range.is_some(),
        receiver_range: range,
        prefix,
    })
}
