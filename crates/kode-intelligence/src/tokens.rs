//! Range-indexed semantic/lexical token preparation; no parsing or drawing.
use crate::{ids::FileId, model::*, snapshot::SemanticSnapshot};
use kode_core::intelligence::HighlightSpan;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SemanticTokenKind {
    Keyword,
    Type,
    Class,
    Interface,
    Function,
    Method,
    Field,
    Property,
    Variable,
    Parameter,
    Constant,
    Namespace,
    Module,
    Label,
    Macro,
    String,
    Number,
    Comment,
    Attribute,
    Lifetime,
    Operator,
    Unknown,
}
impl SemanticTokenKind {
    pub fn class(self) -> &'static str {
        match self {
            Self::Keyword => "semantic.keyword",
            Self::Type => "semantic.type",
            Self::Class => "semantic.class",
            Self::Interface => "semantic.interface",
            Self::Function => "semantic.function",
            Self::Method => "semantic.method",
            Self::Field => "semantic.field",
            Self::Property => "semantic.property",
            Self::Variable => "semantic.variable",
            Self::Parameter => "semantic.parameter",
            Self::Constant => "semantic.constant",
            Self::Namespace => "semantic.namespace",
            Self::Module => "semantic.module",
            Self::Label => "semantic.label",
            Self::Macro => "semantic.macro",
            Self::String => "semantic.string",
            Self::Number => "semantic.number",
            Self::Comment => "semantic.comment",
            Self::Attribute => "semantic.attribute",
            Self::Lifetime => "semantic.lifetime",
            Self::Operator => "semantic.operator",
            Self::Unknown => "semantic.unknown",
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticToken {
    pub range: SourceRange,
    pub kind: SemanticTokenKind,
    pub occurrence: Option<OccurrenceRecord>,
}
impl SemanticToken {
    pub fn highlights(&self) -> Vec<HighlightSpan> {
        let mut classes = vec![];
        if let Some(occurrence) = &self.occurrence {
            classes.push(if occurrence.kind == OccurrenceKind::Declaration {
                "codeintel.declaration"
            } else {
                "codeintel.usage"
            });
            match self.kind {
                SemanticTokenKind::Method => classes.push("codeintel.method"),
                SemanticTokenKind::Field | SemanticTokenKind::Property => {
                    classes.push("codeintel.field")
                }
                _ => {}
            }
        }
        classes.push(self.kind.class());
        classes
            .into_iter()
            .map(|class| HighlightSpan {
                range: self.range,
                class: class.into(),
            })
            .collect()
    }
}
pub struct SemanticTokenService;
impl SemanticTokenService {
    pub fn tokens(
        file: FileId,
        snapshot: &SemanticSnapshot,
        range: Option<SourceRange>,
    ) -> Vec<SemanticToken> {
        let mut result: Vec<_> = match range {
            Some(range) => snapshot
                .lexical_tokens_in(file, range)
                .map(lexical)
                .collect(),
            None => snapshot.lexical_tokens(file).map(lexical).collect(),
        };
        let semantic = |o: &OccurrenceRecord| SemanticToken {
            range: o.range,
            kind: symbol_kind(
                o.resolved_symbol_id
                    .and_then(|id| snapshot.symbol(id))
                    .map(|s| s.kind),
                o.kind,
            ),
            occurrence: Some(o.clone()),
        };
        match range {
            Some(range) => result.extend(snapshot.occurrences_in(file, range).map(semantic)),
            None => result.extend(snapshot.occurrences(file).map(semantic)),
        }
        result.sort_by_key(|t| {
            (
                t.range.start,
                Reverse(t.range.end),
                t.occurrence.is_some(),
                t.kind,
            )
        });
        result.dedup();
        result
    }
}
fn lexical(token: &LexicalTokenRecord) -> SemanticToken {
    use SemanticTokenKind as T;
    let kind = match token.kind {
        LexicalTokenKind::Keyword => T::Keyword,
        LexicalTokenKind::String => T::String,
        LexicalTokenKind::Number => T::Number,
        LexicalTokenKind::Comment => T::Comment,
        LexicalTokenKind::Attribute => T::Attribute,
        LexicalTokenKind::Lifetime => T::Lifetime,
        LexicalTokenKind::Operator => T::Operator,
    };
    SemanticToken {
        range: token.range,
        kind,
        occurrence: None,
    }
}
fn symbol_kind(kind: Option<SymbolKind>, occurrence: OccurrenceKind) -> SemanticTokenKind {
    use SemanticTokenKind as T;
    match kind {
        Some(SymbolKind::Class) => T::Class,
        Some(SymbolKind::Interface) => T::Interface,
        Some(SymbolKind::Struct | SymbolKind::Enum | SymbolKind::TypeAlias) => T::Type,
        Some(SymbolKind::Function | SymbolKind::Constructor) => T::Function,
        Some(SymbolKind::Method) => T::Method,
        Some(SymbolKind::Field) => T::Field,
        Some(SymbolKind::Property) => T::Property,
        Some(SymbolKind::Variable) => T::Variable,
        Some(SymbolKind::Parameter) => T::Parameter,
        Some(SymbolKind::Constant | SymbolKind::EnumMember) => T::Constant,
        Some(SymbolKind::Namespace | SymbolKind::Package) => T::Namespace,
        Some(SymbolKind::Module) => T::Module,
        Some(SymbolKind::Label) => T::Label,
        Some(SymbolKind::Macro) => T::Macro,
        _ if occurrence == OccurrenceKind::TypeReference => T::Type,
        _ => T::Unknown,
    }
}
