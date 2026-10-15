use kode_core::{LineIndex, TextRange};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxSpan {
    pub range: TextRange,
    pub class: String,
    pub color: Option<[u8; 3]>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineToken {
    pub line: LineIndex,
    pub line_range: TextRange,
    pub class: String,
    pub text: String,
    pub color: Option<[u8; 3]>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternDiagnostic {
    pub source: String,
    pub line: usize,
    pub language: String,
    pub message: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrammarMode {
    TreeSitter,
    Keywords,
    Regex,
    Plain,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrammarStatus {
    pub language: String,
    pub mode: GrammarMode,
    pub message: Option<String>,
}
