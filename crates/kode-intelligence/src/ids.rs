//! Stable semantic identities retain Kotlin's signed 64-bit FNV-1a bit patterns.
//! Declaration/occurrence inputs are explicitly UTF-8 byte offsets in Rust.
use super::model::{ScopeKind, SymbolKind, TypeRef};
use kode_core::{ByteOffset, TextRange};
use serde::{Deserialize, Serialize};
macro_rules! id {
    ($name:ident) => {
        #[derive(
            Debug,
            Default,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);
    };
}
id!(FileId);
id!(SymbolId);
id!(ScopeId);
id!(TypeId);
id!(OccurrenceId);
pub struct SemanticIds;
impl SemanticIds {
    pub fn hash(value: &str) -> i64 {
        value
            .as_bytes()
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            }) as i64
    }
    pub fn file(path: &str) -> FileId {
        FileId(Self::hash(&format!("file:{path}")))
    }
    pub fn symbol(file: FileId, start: ByteOffset, kind: SymbolKind, name: &str) -> SymbolId {
        SymbolId(Self::hash(&format!(
            "symbol:{}:{}:{}:{name}",
            file.0,
            start.0,
            kind.wire_name()
        )))
    }
    pub fn scope(file: FileId, start: ByteOffset, kind: ScopeKind) -> ScopeId {
        ScopeId(Self::hash(&format!(
            "scope:{}:{}:{}",
            file.0,
            start.0,
            kind.wire_name()
        )))
    }
    pub fn occurrence(file: FileId, range: TextRange) -> OccurrenceId {
        OccurrenceId(Self::hash(&format!(
            "occurrence:{}:{}:{}",
            file.0, range.start.0, range.end.0
        )))
    }
    pub fn type_description(description: &str) -> TypeId {
        TypeId(Self::hash(&format!("type:{description}")))
    }
    pub fn type_ref(value: &TypeRef) -> TypeId {
        Self::type_description(
            &serde_json::to_string(value).expect("finite serializable type reference"),
        )
    }
    pub fn content_hash(text: &str) -> String {
        format!("{:x}", Self::hash(text) as u64)
    }
}

/// Stable exported signatures retain source IDs/visibility/import/type/hint
/// distinctions. Body ranges and local implementation facts do not participate.
pub fn public_surface_hash(delta: &super::model::FileSemanticDelta) -> String {
    use super::model::SymbolKind;
    use std::collections::{BTreeMap, BTreeSet};
    let symbols: BTreeMap<_, _> = delta
        .symbols
        .iter()
        .map(|symbol| (symbol.id, symbol))
        .collect();
    let mut exported: Vec<_> = delta
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.owner_symbol_id.is_none_or(|owner| {
                symbols.get(&owner).is_some_and(|owner| {
                    matches!(
                        owner.kind,
                        SymbolKind::Class
                            | SymbolKind::Interface
                            | SymbolKind::Struct
                            | SymbolKind::Enum
                            | SymbolKind::TypeAlias
                    )
                })
            })
        })
        .collect();
    exported.sort_by(|a, b| {
        a.qualified_name
            .as_ref()
            .unwrap_or(&a.name)
            .cmp(b.qualified_name.as_ref().unwrap_or(&b.name))
    });
    let ids: BTreeSet<_> = exported.iter().map(|symbol| symbol.id).collect();
    let declarations = exported
        .into_iter()
        .map(|symbol| {
            format!(
                "{}:{}:{}:{}",
                symbol.id.0,
                symbol.qualified_name.as_deref().unwrap_or("null"),
                symbol.kind.wire_name(),
                symbol.flags
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let mut types: Vec<_> = delta
        .unresolved_types
        .iter()
        .filter(|ty| ids.contains(&ty.owner_symbol_id))
        .map(|ty| {
            format!(
                "{}:{}:{}",
                ty.owner_symbol_id.0,
                ty.role.wire_name(),
                ty.name
            )
        })
        .collect();
    types.sort();
    let mut hints: Vec<_> = delta
        .type_hints
        .iter()
        .filter(|hint| ids.contains(&hint.target_symbol_id))
        .map(|hint| {
            format!(
                "{}:{}:{}",
                hint.target_symbol_id.0,
                hint.kind.wire_name(),
                hint.referenced_name
            )
        })
        .collect();
    hints.sort();
    let mut imports: Vec<_> = delta
        .imports
        .iter()
        .map(|import| {
            format!(
                "{}:{}:{}",
                import.path,
                import.alias.as_deref().unwrap_or("null"),
                import.wildcard
            )
        })
        .collect();
    imports.sort();
    let mut signature = format!(
        "{declarations}#types={}#hints={}#imports={}",
        types.join("|"),
        hints.join("|"),
        imports.join("|")
    );
    let mut expressions = Vec::new();
    for expression in &delta.syntax_expressions {
        for target in expression.assigned_to.iter().filter(|id| ids.contains(id)) {
            if delta.unresolved_types.iter().any(|ty| {
                ty.owner_symbol_id == *target && ty.role != super::model::TypeRole::SuperType
            }) {
                continue;
            }
            if let super::model::SyntaxExpression::Call { type_arguments, .. } =
                &expression.expression
                && !type_arguments.is_empty()
            {
                expressions.push(format!(
                    "{}:arguments:{}",
                    target.0,
                    type_arguments.join(",")
                ));
            } else if !delta.type_hints.iter().any(|hint| {
                hint.target_symbol_id == *target
                    && hint.expression_range.start >= expression.range.start
                    && hint.expression_range.end <= expression.range.end
            }) && let Some(hash) = &expression.binding_hash
            {
                expressions.push(format!("{}:{hash}", target.0));
            }
        }
    }
    if !expressions.is_empty() {
        expressions.sort();
        signature.push_str("#expressions=");
        signature.push_str(&expressions.join("|"));
    }
    SemanticIds::content_hash(&signature)
}
