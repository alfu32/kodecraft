//! Worker-side UAX #29 selection, retaining source byte offsets and line breaks.
use crate::{ByteOffset, DomainError, Result, TextRange, document::TextSnapshot};
use unicode_segmentation::UnicodeSegmentation;

/// Select the UAX #29 segment under a scalar, including whitespace/punctuation
/// segments. Line endings, empty lines and positions beyond displayed content
/// remain carets. No segment crosses a logical line or includes CR/LF.
pub fn range_at(
    snapshot: &TextSnapshot,
    at: ByteOffset,
    cancelled: impl Fn() -> bool,
) -> Result<TextRange> {
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let point = snapshot.byte_to_position(at)?;
    let start = snapshot.line_start(point.line)?;
    let line = snapshot.line_content(point.line)?;
    if at.0 >= start.0 + line.len_bytes() {
        return Ok(TextRange::caret(at));
    }
    let mut text = String::with_capacity(line.len_bytes());
    for chunk in line.chunks() {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        text.push_str(chunk);
    }
    let local = at.0 - start.0;
    for (offset, segment) in text.split_word_bound_indices() {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        if local < offset + segment.len() {
            return TextRange::new(
                ByteOffset(start.0 + offset),
                ByteOffset(start.0 + offset + segment.len()),
            );
        }
    }
    Ok(TextRange::caret(at))
}
