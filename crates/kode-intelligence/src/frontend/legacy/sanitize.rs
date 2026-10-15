use super::registry::LanguageConfig;
use crate::model::{LexicalTokenKind, SourceRange};
use kode_core::{ByteOffset, DomainError, Result};
use kode_platform::jobs::CancellationToken;

pub(super) fn sanitize(
    text: &str,
    config: &LanguageConfig,
    cancel: &CancellationToken,
) -> Result<(String, Vec<(SourceRange, LexicalTokenKind)>)> {
    let mut bytes = text.as_bytes().to_vec();
    let raw = text.as_bytes();
    let mut lexical = vec![];
    let mut index = 0;
    while index < raw.len() {
        if index.is_multiple_of(1024) && cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let start = index;
        let kind = if let Some(marker) = config
            .block_start
            .as_ref()
            .filter(|s| raw[index..].starts_with(s.as_bytes()))
        {
            index += marker.len();
            let end = config.block_end.as_deref().unwrap_or("*/");
            index = text[index..]
                .find(end)
                .map_or(raw.len(), |at| index + at + end.len());
            Some(LexicalTokenKind::Comment)
        } else if config
            .line_comments
            .iter()
            .any(|s| raw[index..].starts_with(s.as_bytes()))
        {
            index = text[index..].find('\n').map_or(raw.len(), |at| index + at);
            Some(LexicalTokenKind::Comment)
        } else if matches!(raw[index], b'\'' | b'"' | b'`') {
            let quote = raw[index];
            index += 1;
            while index < raw.len() && raw[index] != b'\n' {
                if raw[index] == b'\\' {
                    index = (index + 2).min(raw.len());
                } else if raw[index] == quote {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            Some(LexicalTokenKind::String)
        } else {
            index += 1;
            None
        };
        if let Some(kind) = kind {
            // Escapes can step into a multibyte scalar; consume its remaining
            // bytes before constructing a published source range.
            while index < raw.len() && !text.is_char_boundary(index) {
                index += 1;
            }
            for byte in &mut bytes[start..index] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
            lexical.push((
                SourceRange::new(ByteOffset(start), ByteOffset(index))?,
                kind,
            ));
            if lexical.len() > 100_000 {
                return Err(super::limit());
            }
        }
    }
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    Ok((
        String::from_utf8(bytes).expect("ASCII masking preserves UTF-8"),
        lexical,
    ))
}
