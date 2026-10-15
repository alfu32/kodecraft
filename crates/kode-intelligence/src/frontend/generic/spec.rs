use crate::model::{ScopeKind, SymbolKind};
use tree_sitter::Node;
pub(super) fn identifier(node: Node<'_>) -> bool {
    node.is_named()
        && !node.is_missing()
        && node.child_count() == 0
        && matches!(
            node.kind(),
            "identifier"
                | "field_identifier"
                | "property_identifier"
                | "private_property_identifier"
                | "type_identifier"
                | "namespace_identifier"
                | "variable_name"
                | "name"
                | "simple_identifier"
                | "constant"
                | "shorthand_property_identifier_pattern"
        )
}
pub(super) fn declaration<'tree>(
    node: Node<'tree>,
    source: &str,
) -> Option<(Node<'tree>, SymbolKind)> {
    use SymbolKind::*;
    let kind = match node.kind() {
        "namespace_definition"
        | "namespace_declaration"
        | "module_declaration"
        | "internal_module" => Namespace,
        "package_clause" => Package,
        "module" => Module,
        "interface_declaration" | "interface_definition" | "protocol_declaration" => Interface,
        "enum_declaration" | "enum_specifier" => Enum,
        "struct_specifier" | "struct_declaration" => Struct,
        "type_alias_declaration" | "alias_declaration" | "type_alias" => TypeAlias,
        "class" | "class_declaration" | "class_definition" | "class_specifier" | "type_spec" => {
            let text = &source[node.byte_range()];
            if text.starts_with("struct ") {
                Struct
            } else if text.starts_with("enum ") {
                Enum
            } else {
                Class
            }
        }
        "constructor_declaration" => Constructor,
        "method"
        | "method_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature" => Method,
        "function_declaration" | "function_definition" | "function_item" | "function_signature" => {
            Function
        }
        "parameter"
        | "required_parameter"
        | "optional_parameter"
        | "formal_parameter"
        | "typed_parameter"
        | "variadic_parameter"
        | "rest_parameter"
        | "parameter_declaration" => Parameter,
        "field_declaration"
        | "property_declaration"
        | "public_field_definition"
        | "private_field_definition"
        | "protected_field_definition"
        | "property_signature" => Property,
        "variable_declarator"
        | "var_spec"
        | "init_declarator"
        | "let_declaration"
        | "local_variable_declaration"
        | "assignment_pattern"
        | "variable_assignment" => Variable,
        "const_spec" => Constant,
        // Python's real grammar has no variable_declarator node.
        "assignment"
            if node
                .parent()
                .is_some_and(|n| n.kind() == "expression_statement") =>
        {
            Variable
        }
        _ => return None,
    };
    let candidate = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("declarator"))
        .or_else(|| node.child_by_field_name("pattern"))
        .or_else(|| {
            if kind == Variable {
                node.child_by_field_name("left")
            } else {
                None
            }
        })
        .or_else(|| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find(|n| identifier(*n))
        })?;
    let name = first_identifier(candidate, 0)?;
    if name.start_byte() == name.end_byte() {
        return None;
    }
    let kind = if kind == Variable
        && node.parent().is_some_and(|n| {
            n.kind() == "lexical_declaration"
                && source[n.byte_range()].trim_start().starts_with("const ")
        }) {
        Constant
    } else {
        kind
    };
    Some((name, kind))
}
fn first_identifier(node: Node<'_>, depth: usize) -> Option<Node<'_>> {
    if identifier(node) || (node.kind() == "word" && node.child_count() == 0) {
        return Some(node);
    }
    if depth > 8 {
        return None;
    }
    let named = node
        .child_by_field_name("declarator")
        .or_else(|| node.child_by_field_name("name"));
    if let Some(node) = named {
        return first_identifier(node, depth + 1);
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|n| first_identifier(n, depth + 1))
}
pub(super) fn scope(kind: &str) -> Option<ScopeKind> {
    match kind {
        "class"
        | "module"
        | "class_declaration"
        | "class_definition"
        | "class_specifier"
        | "interface_declaration"
        | "interface_definition"
        | "struct_specifier"
        | "struct_declaration"
        | "enum_declaration"
        | "enum_specifier"
        | "protocol_declaration"
        | "type_spec" => Some(ScopeKind::Type),
        "namespace_definition"
        | "namespace_declaration"
        | "module_declaration"
        | "internal_module" => Some(ScopeKind::Namespace),
        "method"
        | "method_declaration"
        | "method_definition"
        | "method_signature"
        | "function_declaration"
        | "function_definition"
        | "function_item"
        | "function_signature"
        | "constructor_declaration" => Some(ScopeKind::Function),
        "lambda" | "lambda_expression" | "arrow_function" => Some(ScopeKind::Lambda),
        "block"
        | "statement_block"
        | "compound_statement"
        | "class_body"
        | "function_body"
        | "body_statement"
        | "declaration_list"
        | "field_declaration_list" => Some(ScopeKind::Block),
        _ => None,
    }
}
pub(super) fn keyword(text: &str) -> bool {
    matches!(
        text,
        "as" | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "fun"
            | "function"
            | "if"
            | "import"
            | "in"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "object"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "struct"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "type"
            | "val"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "def"
            | "from"
            | "async"
            | "await"
            | "func"
            | "go"
            | "defer"
            | "map"
            | "range"
            | "select"
            | "chan"
            | "where"
            | "using"
            | "namespace"
    )
}
