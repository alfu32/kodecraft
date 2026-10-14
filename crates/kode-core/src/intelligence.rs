use crate::{ByteOffset, DocumentId, DocumentOrigin, JobTag, Result, Revision, TextRange};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSnapshot {
    pub id: DocumentId,
    pub revision: Revision,
    pub origin: DocumentOrigin,
    pub language: Option<String>,
    #[serde(default)]
    pub format: crate::document::encoding::FileFormat,
    pub text: Arc<str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
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
    Parameter,
    TypeAlias,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationTarget {
    pub path: PathBuf,
    pub document: Option<DocumentId>,
    pub identifier_range: TextRange,
    pub declaration_range: Option<TextRange>,
    pub language: Option<String>,
    pub kind: Option<SymbolKind>,
    pub name: Option<String>,
    pub missing_source: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    pub label: String,
    pub insert_text: String,
    pub detail: Option<String>,
    pub kind: Option<SymbolKind>,
    pub replacement_range: TextRange,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightSpan {
    pub range: TextRange,
    pub class: String,
}

#[derive(Debug, Clone)]
pub struct IntelligenceRequest {
    pub tag: JobTag,
    pub snapshot: DocumentSnapshot,
    pub position: ByteOffset,
}

/// Domain interface: no terminal types, implicit I/O or empty-success defaults.
pub trait IntelligenceProvider: Send + Sync {
    fn definitions(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>>;
    fn references(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>>;
    fn completions(&self, request: &IntelligenceRequest) -> Result<Vec<CompletionItem>>;
    fn highlights(&self, snapshot: &DocumentSnapshot) -> Result<Vec<HighlightSpan>>;
}

/// Prefix fallback is deliberately ASCII-compatible with Kode's identifier rules.
pub fn identifier_prefix(text: &str, cursor: ByteOffset) -> Result<TextRange> {
    TextRange::caret(cursor).validate(text)?;
    let mut start = cursor.0;
    for (offset, ch) in text[..cursor.0].char_indices().rev() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$') {
            break;
        }
        start = offset;
    }
    TextRange::new(ByteOffset(start), cursor)
}
