//! Portable facts: extraction does not perform cross-file resolution.
use super::ids::*;
use kode_core::{DomainError, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
pub type SourceRange = kode_core::TextRange;
macro_rules! semantic_enum { ($name:ident { $($variant:ident => $wire:literal),+ $(,)? })=>{
    #[derive(Debug,Clone,Copy,PartialEq,Eq,PartialOrd,Ord,Hash,Serialize,Deserialize)]
    pub enum $name { $(#[serde(rename=$wire)] $variant),+ }
    impl $name { pub fn wire_name(self)->&'static str { match self { $(Self::$variant=>$wire),+ } } }
}; }
semantic_enum!(SymbolKind { Package=>"PACKAGE",Module=>"MODULE",Namespace=>"NAMESPACE",Class=>"CLASS",Interface=>"INTERFACE",Struct=>"STRUCT",Enum=>"ENUM",TypeAlias=>"TYPE_ALIAS",Function=>"FUNCTION",Method=>"METHOD",Constructor=>"CONSTRUCTOR",Field=>"FIELD",Property=>"PROPERTY",Variable=>"VARIABLE",Parameter=>"PARAMETER",Constant=>"CONSTANT",EnumMember=>"ENUM_MEMBER",Label=>"LABEL",Macro=>"MACRO",Unknown=>"UNKNOWN" });
semantic_enum!(ScopeKind { File=>"FILE",Package=>"PACKAGE",Module=>"MODULE",Namespace=>"NAMESPACE",Type=>"TYPE",Function=>"FUNCTION",Block=>"BLOCK",Lambda=>"LAMBDA" });
semantic_enum!(OccurrenceKind { Declaration=>"DECLARATION",Read=>"READ",Write=>"WRITE",ReadWrite=>"READ_WRITE",Call=>"CALL",TypeReference=>"TYPE_REFERENCE",Import=>"IMPORT",MemberReference=>"MEMBER_REFERENCE",Unknown=>"UNKNOWN" });
semantic_enum!(RelationKind { Contains=>"CONTAINS",MemberOf=>"MEMBER_OF",Extends=>"EXTENDS",Implements=>"IMPLEMENTS",TypeOf=>"TYPE_OF",Returns=>"RETURNS",ParameterType=>"PARAMETER_TYPE",Imports=>"IMPORTS",Calls=>"CALLS",Overrides=>"OVERRIDES",AliasOf=>"ALIAS_OF" });
semantic_enum!(FileDependencyKind { Import=>"IMPORT",TypeReference=>"TYPE_REFERENCE",Call=>"CALL",Inheritance=>"INHERITANCE",MemberReference=>"MEMBER_REFERENCE",Reference=>"REFERENCE" });
semantic_enum!(SemanticSource { TreeSitter=>"TREE_SITTER",StaticInference=>"STATIC_INFERENCE",Lsp=>"LSP",Compiler=>"COMPILER",Scip=>"SCIP",Heuristic=>"HEURISTIC",Ml=>"ML" });
semantic_enum!(LexicalTokenKind { Keyword=>"KEYWORD",String=>"STRING",Number=>"NUMBER",Comment=>"COMMENT",Attribute=>"ATTRIBUTE",Lifetime=>"LIFETIME",Operator=>"OPERATOR" });
semantic_enum!(TypeKnowledgeLevel { Unknown=>"UNKNOWN",SyntacticallyDeclared=>"SYNTACTICALLY_DECLARED",LocallyInferred=>"LOCALLY_INFERRED",CrossFileResolved=>"CROSS_FILE_RESOLVED",ExternalProvider=>"EXTERNAL_PROVIDER" });
semantic_enum!(TypeRole { Declared=>"DECLARED",Return=>"RETURN",Parameter=>"PARAMETER",SuperType=>"SUPER_TYPE" });
semantic_enum!(TypeHintKind { InitializerCall=>"INITIALIZER_CALL",ConstructorCall=>"CONSTRUCTOR_CALL",Assignment=>"ASSIGNMENT",Cast=>"CAST",Literal=>"LITERAL" });
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Confidence {
    pub source: SemanticSource,
    value: f32,
}
impl Confidence {
    pub fn new(source: SemanticSource, value: f32) -> Result<Self> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(DomainError::Parse {
                format: "semantic confidence",
                message: "confidence must be finite and between zero and one".into(),
            });
        }
        Ok(Self { source, value })
    }
    pub fn value(self) -> f32 {
        self.value
    }
    pub fn tree_sitter() -> Self {
        Self {
            source: SemanticSource::TreeSitter,
            value: 0.9,
        }
    }
    pub fn inferred(value: f32) -> Result<Self> {
        Self::new(SemanticSource::StaticInference, value)
    }
}
impl Default for Confidence {
    fn default() -> Self {
        Self::tree_sitter()
    }
}
impl<'de> Deserialize<'de> for Confidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            source: SemanticSource,
            value: f32,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.source, wire.value).map_err(serde::de::Error::custom)
    }
}
pub struct SymbolFlags;
impl SymbolFlags {
    pub const PUBLIC: u64 = 1;
    pub const PROTECTED: u64 = 1 << 1;
    pub const INTERNAL: u64 = 1 << 2;
    pub const PRIVATE: u64 = 1 << 3;
    pub const VISIBILITY_MASK: u64 = 15;
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRecord {
    pub id: FileId,
    pub path: String,
    pub language_id: String,
    pub content_hash: String,
    pub parse_version: u64,
    pub semantic_version: u64,
    #[serde(default)]
    pub dependency_generation: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolRecord {
    pub id: SymbolId,
    pub file_id: FileId,
    pub name: String,
    pub qualified_name: Option<String>,
    pub kind: SymbolKind,
    pub declaration_range: SourceRange,
    pub name_range: SourceRange,
    pub scope_id: ScopeId,
    pub owner_symbol_id: Option<SymbolId>,
    pub declared_type_id: Option<TypeId>,
    pub inferred_type_id: Option<TypeId>,
    pub flags: u64,
    #[serde(default)]
    pub confidence: Confidence,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeRecord {
    pub id: ScopeId,
    pub file_id: FileId,
    pub parent_id: Option<ScopeId>,
    pub owner_symbol_id: Option<SymbolId>,
    pub kind: ScopeKind,
    pub range: SourceRange,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OccurrenceRecord {
    pub id: OccurrenceId,
    pub file_id: FileId,
    pub range: SourceRange,
    pub text: String,
    pub kind: OccurrenceKind,
    pub scope_id: ScopeId,
    pub resolved_symbol_id: Option<SymbolId>,
    pub receiver_occurrence_id: Option<OccurrenceId>,
    pub confidence: Confidence,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LexicalTokenRecord {
    pub file_id: FileId,
    pub range: SourceRange,
    pub kind: LexicalTokenKind,
    #[serde(default)]
    pub confidence: Confidence,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpressionTypeRecord {
    pub file_id: FileId,
    pub range: SourceRange,
    pub r#type: TypeRef,
    pub confidence: Confidence,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TypeRef {
    Named {
        symbol_id: SymbolId,
    },
    Primitive {
        name: String,
    },
    Generic {
        base: Box<TypeRef>,
        arguments: Vec<TypeRef>,
    },
    Function {
        parameters: Vec<TypeRef>,
        returns: Option<Box<TypeRef>>,
    },
    Union {
        alternatives: Vec<TypeRef>,
    },
    Nullable {
        inner: Box<TypeRef>,
    },
    Unknown,
}
impl TypeRef {
    pub fn named(symbol_id: SymbolId) -> Self {
        Self::Named { symbol_id }
    }
    pub fn primitive(name: impl Into<String>) -> Self {
        Self::Primitive { name: name.into() }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeRecord {
    pub id: TypeId,
    pub r#ref: TypeRef,
    pub confidence: Confidence,
    pub knowledge_level: TypeKnowledgeLevel,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationRecord {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: RelationKind,
    #[serde(default = "relation_confidence")]
    pub confidence: Confidence,
}
fn relation_confidence() -> Confidence {
    Confidence {
        source: SemanticSource::StaticInference,
        value: 0.95,
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDependencyRecord {
    pub from_file_id: FileId,
    pub to_file_id: FileId,
    pub kind: FileDependencyKind,
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedTypeRef {
    pub owner_symbol_id: SymbolId,
    pub scope_id: ScopeId,
    pub name: String,
    pub role: TypeRole,
    pub range: SourceRange,
    #[serde(default)]
    pub confidence: Confidence,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeHint {
    pub target_symbol_id: SymbolId,
    pub scope_id: ScopeId,
    pub expression_range: SourceRange,
    pub referenced_name: String,
    pub kind: TypeHintKind,
    #[serde(default = "hint_confidence")]
    pub confidence: Confidence,
}
fn hint_confidence() -> Confidence {
    Confidence {
        source: SemanticSource::StaticInference,
        value: 0.85,
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecord {
    pub file_id: FileId,
    pub scope_id: ScopeId,
    pub path: String,
    pub alias: Option<String>,
    pub wildcard: bool,
    pub range: SourceRange,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSemanticDelta {
    pub file: FileRecord,
    pub scopes: Vec<ScopeRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub occurrences: Vec<OccurrenceRecord>,
    pub relations: Vec<RelationRecord>,
    pub unresolved_types: Vec<UnresolvedTypeRef>,
    pub imports: Vec<ImportRecord>,
    #[serde(default)]
    pub type_hints: Vec<TypeHint>,
    #[serde(default)]
    pub expression_types: Vec<ExpressionTypeRecord>,
    #[serde(default)]
    pub lexical_tokens: Vec<LexicalTokenRecord>,
    pub exported_surface_hash: String,
}
impl FileSemanticDelta {
    pub fn file_id(&self) -> FileId {
        self.file.id
    }
    pub fn version(&self) -> u64 {
        self.file.semantic_version
    }
    pub fn ranges(&self) -> impl Iterator<Item = SourceRange> + '_ {
        self.scopes
            .iter()
            .map(|r| r.range)
            .chain(
                self.symbols
                    .iter()
                    .flat_map(|s| [s.declaration_range, s.name_range]),
            )
            .chain(self.occurrences.iter().map(|r| r.range))
            .chain(self.unresolved_types.iter().map(|r| r.range))
            .chain(self.imports.iter().map(|r| r.range))
            .chain(self.type_hints.iter().map(|r| r.expression_range))
            .chain(self.expression_types.iter().map(|r| r.range))
            .chain(self.lexical_tokens.iter().map(|r| r.range))
    }
    pub fn validate_source(&self, text: &str) -> Result<()> {
        for range in self.ranges() {
            range.validate(text)?;
        }
        Ok(())
    }
}
/// Resolver output and stored extraction facts have separate ownership. Queries
/// can use enriched records without changing the original file deltas.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSemanticProject {
    pub deltas: BTreeMap<FileId, Arc<FileSemanticDelta>>,
    pub symbols: Vec<SymbolRecord>,
    pub scopes: Vec<ScopeRecord>,
    pub occurrences: Vec<OccurrenceRecord>,
    pub relations: Vec<RelationRecord>,
    pub types: BTreeMap<TypeId, TypeRecord>,
    pub expression_types: Vec<ExpressionTypeRecord>,
    pub dependencies: Vec<FileDependencyRecord>,
    pub resolution_generations: BTreeMap<FileId, u64>,
}
