//! Preserve actual expression structure rather than guessing from token order.
use crate::{ids::*, model::*};
use kode_core::{ByteOffset, DomainError, Result};
use kode_platform::jobs::CancellationToken;
use kode_syntax::parser::ParsedDocument;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    time::{Duration, Instant},
};
use tree_sitter::Node;
pub(super) fn collect(
    parsed: &ParsedDocument,
    delta: &FileSemanticDelta,
    scopes: &HashMap<usize, ScopeId>,
    bindings: &HashMap<SourceRange, Vec<SymbolId>>,
    cancel: &CancellationToken,
) -> Result<Vec<ExpressionSyntaxRecord>> {
    let started = Instant::now();
    let mut result = vec![];
    let source = parsed.source();
    let occurrences: BTreeSet<_> = delta.occurrences.iter().map(|o| o.id).collect();
    let literals: BTreeMap<_, _> = delta
        .expression_types
        .iter()
        .filter_map(|e| match &e.r#type {
            TypeRef::Primitive { name } => Some(((e.range.start, e.range.end), name.as_str())),
            _ => None,
        })
        .collect();
    let root_scope = delta
        .scopes
        .first()
        .ok_or_else(|| DomainError::Unsupported("expression file has no scope".into()))?
        .id;
    let mut cursor = parsed.tree().walk();
    let mut visited = 0usize;
    loop {
        visited += 1;
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if visited > 1_000_000 || started.elapsed() > Duration::from_secs(2) {
            return Err(DomainError::Unsupported(
                "expression syntax work limit exceeded".into(),
            ));
        }
        let node = cursor.node();
        let span = range(node);
        let assigned = bindings.get(&span).cloned().unwrap_or_default();
        let id = SemanticIds::occurrence(delta.file.id, span);
        let expression = if let Some(name) = literals.get(&(span.start, span.end)) {
            Some(SyntaxExpression::Literal {
                type_name: (*name).into(),
            })
        } else if occurrences.contains(&id)
            && matches!(
                node.kind(),
                "identifier"
                    | "simple_identifier"
                    | "type_identifier"
                    | "field_identifier"
                    | "interpolated_identifier"
                    | "this_expression"
                    | "super_expression"
            )
        {
            Some(SyntaxExpression::Reference { occurrence_id: id })
        } else {
            match node.kind() {
                "call_expression" | "constructor_invocation" => node
                    .child_by_field_name("function")
                    .or_else(|| children(node).first().copied())
                    .map(|callee| SyntaxExpression::Call {
                        callee: range(callee),
                        type_arguments: type_arguments(node, source),
                    }),
                "navigation_expression" | "field_expression" => {
                    let receiver = node
                        .child_by_field_name("argument")
                        .or_else(|| children(node).first().copied());
                    let field = node
                        .child_by_field_name("field")
                        .or_else(|| children(node).last().copied())
                        .and_then(super::receiver_node);
                    receiver.zip(field).and_then(|(receiver, field)| {
                        let id = SemanticIds::occurrence(delta.file.id, range(field));
                        occurrences.contains(&id).then(|| SyntaxExpression::Member {
                            receiver: range(receiver),
                            occurrence_id: id,
                        })
                    })
                }
                "as_expression" | "cast_expression" => {
                    let value = node
                        .child_by_field_name("left")
                        .or_else(|| node.child_by_field_name("value"))
                        .or_else(|| children(node).first().copied());
                    let ty = node
                        .child_by_field_name("right")
                        .or_else(|| node.child_by_field_name("type"))
                        .or_else(|| children(node).last().copied());
                    value.zip(ty).map(|(value, ty)| SyntaxExpression::Cast {
                        value: range(value),
                        type_name: source[ty.byte_range()].into(),
                    })
                }
                "parenthesized_expression" => {
                    children(node)
                        .first()
                        .map(|inner| SyntaxExpression::Parenthesized {
                            inner: range(*inner),
                        })
                }
                "pointer_expression" | "subscript_expression" => node
                    .child_by_field_name("argument")
                    .map(|inner| SyntaxExpression::Parenthesized {
                        inner: range(inner),
                    }),
                _ => None,
            }
        };
        if node.is_named()
            && !node.is_missing()
            && !span.is_empty()
            && (expression.is_some() || !assigned.is_empty())
        {
            let binding_hash = (!assigned.is_empty())
                .then(|| SemanticIds::content_hash(&source[node.byte_range()]));
            result.push(ExpressionSyntaxRecord {
                range: span,
                scope_id: scopes.get(&node.id()).copied().unwrap_or(root_scope),
                expression: expression.unwrap_or(SyntaxExpression::Unknown),
                assigned_to: assigned,
                binding_hash,
            });
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return Ok(result);
            }
        }
    }
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn range(node: Node<'_>) -> SourceRange {
    SourceRange::new(ByteOffset(node.start_byte()), ByteOffset(node.end_byte()))
        .expect("ordered expression range")
}
fn type_arguments(node: Node<'_>, source: &str) -> Vec<String> {
    for child in children(node) {
        let arguments = if child.kind() == "type_arguments" {
            Some(child)
        } else if matches!(child.kind(), "call_suffix" | "user_type") {
            children(child)
                .into_iter()
                .find(|node| node.kind() == "type_arguments")
        } else {
            None
        };
        if let Some(arguments) = arguments {
            return children(arguments)
                .iter()
                .map(|node| source[node.byte_range()].to_owned())
                .collect();
        }
    }
    vec![]
}
