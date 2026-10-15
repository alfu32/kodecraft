//! Compatibility facts keep the legacy metadata, with explicit native units.
use kode_core::{LineIndex, TextRange, Utf16Column};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxMetadata {
    pub language: Option<String>,
    pub parent: Option<String>,
    pub kind: Option<String>,
    pub is_named: Option<bool>,
    pub field_names: Option<String>,
    pub hierarchy_kind: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CompatibilitySymbolKind {
    Class,
    Interface,
    Function,
    Method,
    Field,
    Variable,
    Enum,
    Object,
    Module,
    Package,
    Keyword,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityDefinition {
    pub name: String,
    pub kind: CompatibilitySymbolKind,
    pub range_bytes: TextRange,
    pub container: Option<String>,
    pub parent_key: Option<String>,
    pub line: Option<LineIndex>,
    pub start_utf16_column: Option<Utf16Column>,
    pub language: Option<String>,
    pub syntax: SyntaxMetadata,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityUsage {
    pub name: String,
    pub range_bytes: TextRange,
    pub line: LineIndex,
    pub start_utf16_column: Utf16Column,
    pub end_utf16_column: Utf16Column,
    pub declaration: bool,
    pub container: Option<String>,
    pub parent_key: Option<String>,
    pub syntax: SyntaxMetadata,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityFile {
    pub path: String,
    pub definitions: Vec<CompatibilityDefinition>,
    pub usages: Vec<CompatibilityUsage>,
}
