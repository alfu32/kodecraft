//! C declaration/occurrence extraction; no preprocessing or compiler validation.
mod builder;
use super::LanguageCompletionContext;
use crate::model::FileSemanticDelta;
use kode_core::{ByteOffset, Result, TextRange, WorkspaceGeneration, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{
    parser::{ParsedDocument, ParserSession},
    registry::GrammarRegistry,
};
pub struct CSemanticAdapter {
    parser: ParserSession,
}
impl CSemanticAdapter {
    pub fn last_document(&self) -> Option<&ParsedDocument> {
        self.parser.last_document()
    }
    pub fn new(registry: &GrammarRegistry) -> Result<Self> {
        let grammar = registry.get("c").ok_or_else(|| {
            kode_core::DomainError::Unsupported("native C grammar unavailable".into())
        })?;
        Ok(Self {
            parser: ParserSession::new(grammar)?,
        })
    }
    pub fn extract(
        &mut self,
        path: &str,
        snapshot: &TextSnapshot,
        generation: WorkspaceGeneration,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        let parsed = self.parser.parse(snapshot, generation, cancel)?;
        Self::extract_parsed(path, &parsed, cancel)
    }
    pub fn extract_parsed(
        path: &str,
        parsed: &ParsedDocument,
        cancel: &CancellationToken,
    ) -> Result<FileSemanticDelta> {
        if parsed.grammar().id != "c" {
            return Err(kode_core::DomainError::Unsupported(
                "C extraction requires the selected C grammar".into(),
            ));
        }
        builder::Builder::new(path, parsed, cancel).build()
    }
}
pub fn completion_context(text: &str, offset: ByteOffset) -> Result<LanguageCompletionContext> {
    TextRange::new(offset, offset)?.validate(text)?;
    let before = &text[..offset.0];
    let prefix_start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prefix = before[prefix_start..].to_owned();
    let operator = before[..prefix_start].trim_end();
    let receiver = if let Some(receiver) = operator.strip_suffix("->") {
        receiver
    } else if let Some(receiver) = operator.strip_suffix('.') {
        receiver
    } else {
        return Ok(LanguageCompletionContext {
            member_access: false,
            receiver_range: None,
            prefix,
        });
    };
    let receiver = receiver.trim_end();
    let (mut depth, mut start) = (0usize, 0usize);
    for (index, ch) in receiver.char_indices().rev() {
        let stop = match ch {
            ')' | ']' => {
                depth += 1;
                false
            }
            '(' | '[' => {
                if depth > 0 {
                    depth -= 1;
                    false
                } else {
                    true
                }
            }
            _ => depth == 0 && !(ch.is_alphanumeric() || "_.->".contains(ch)),
        };
        if stop {
            start = index + ch.len_utf8();
            break;
        }
    }
    let range = (start < receiver.len())
        .then(|| TextRange::new(ByteOffset(start), ByteOffset(receiver.len())))
        .transpose()?;
    Ok(LanguageCompletionContext {
        member_access: range.is_some(),
        receiver_range: range,
        prefix,
    })
}
