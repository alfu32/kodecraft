use super::positions::{self, TextPosition, Utf16Position};
use crate::intelligence::DocumentSnapshot;
use crate::{
    ByteOffset, CharOffset, DocumentId, DocumentOrigin, LineIndex, Result, Revision, TextRange,
};
use ropey::{Rope, RopeSlice};
use std::sync::{Arc, OnceLock};

/// O(1) immutable rope clone. Worker DTO text is materialized lazily once per
/// revision, while layout/search use lines/slices/chunks without whole-file copies.
#[derive(Debug, Clone)]
pub struct TextSnapshot {
    pub id: DocumentId,
    pub revision: Revision,
    pub origin: DocumentOrigin,
    pub language: Option<String>,
    pub format: super::encoding::FileFormat,
    pub(crate) rope: Rope,
    pub(crate) flattened: Arc<OnceLock<Arc<str>>>,
}
impl TextSnapshot {
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }
    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }
    pub fn text(&self) -> Arc<str> {
        self.flattened
            .get_or_init(|| Arc::from(self.rope.to_string()))
            .clone()
    }
    pub fn to_service_snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            id: self.id,
            revision: self.revision,
            origin: self.origin.clone(),
            language: self.language.clone(),
            format: self.format,
            text: self.text(),
        }
    }
    pub fn line(&self, index: LineIndex) -> Result<RopeSlice<'_>> {
        positions::line(&self.rope, index)
    }
    pub fn line_content(&self, index: LineIndex) -> Result<RopeSlice<'_>> {
        positions::content(&self.rope, index)
    }
    pub fn line_start(&self, index: LineIndex) -> Result<ByteOffset> {
        self.line(index)?;
        Ok(ByteOffset(self.rope.line_to_byte(index.0)))
    }
    pub fn line_end(&self, index: LineIndex) -> Result<ByteOffset> {
        positions::line_end(&self.rope, index)
    }
    pub fn slice(&self, range: TextRange) -> Result<RopeSlice<'_>> {
        TextRange::new(range.start, range.end)?;
        positions::validate_byte(&self.rope, range.start)?;
        positions::validate_byte(&self.rope, range.end)?;
        Ok(self.rope.byte_slice(range.start.0..range.end.0))
    }
    pub fn chunks(&self) -> ropey::iter::Chunks<'_> {
        self.rope.chunks()
    }
    pub fn chars(&self) -> ropey::iter::Chars<'_> {
        self.rope.chars()
    }
    pub fn lines(&self) -> ropey::iter::Lines<'_> {
        self.rope.lines()
    }
    pub fn byte_to_char(&self, byte: ByteOffset) -> Result<CharOffset> {
        positions::validate_byte(&self.rope, byte)
    }
    pub fn char_to_byte(&self, scalar: CharOffset) -> Result<ByteOffset> {
        positions::scalar_to_byte(&self.rope, scalar)
    }
    pub fn clamp_byte(&self, byte: ByteOffset) -> ByteOffset {
        positions::clamp_byte(&self.rope, byte)
    }
    pub fn byte_to_position(&self, byte: ByteOffset) -> Result<TextPosition> {
        positions::byte_to_position(&self.rope, byte)
    }
    pub fn position_to_byte(&self, position: TextPosition) -> Result<ByteOffset> {
        positions::position_to_byte(&self.rope, position)
    }
    pub fn byte_to_utf16(&self, byte: ByteOffset) -> Result<Utf16Position> {
        positions::byte_to_utf16(&self.rope, byte)
    }
    pub fn utf16_to_byte_clamped(&self, position: Utf16Position) -> ByteOffset {
        positions::utf16_to_byte_clamped(&self.rope, position)
    }
}
