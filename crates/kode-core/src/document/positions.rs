use crate::{ByteOffset, CharOffset, DomainError, LineIndex, Result, Utf16Column};
use ropey::{Rope, RopeSlice};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPosition {
    pub line: LineIndex,
    pub column: CharOffset,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utf16Position {
    pub line: LineIndex,
    pub column: Utf16Column,
}

pub fn validate_byte(rope: &Rope, byte: ByteOffset) -> Result<CharOffset> {
    if byte.0 > rope.len_bytes() {
        return Err(DomainError::InvalidBoundary(byte.0));
    }
    let scalar = rope.byte_to_char(byte.0);
    if rope.char_to_byte(scalar) != byte.0 {
        return Err(DomainError::InvalidBoundary(byte.0));
    }
    Ok(CharOffset(scalar))
}
pub fn scalar_to_byte(rope: &Rope, scalar: CharOffset) -> Result<ByteOffset> {
    if scalar.0 > rope.len_chars() {
        return Err(DomainError::InvalidCoordinate {
            unit: "scalar",
            index: scalar.0,
            limit: rope.len_chars(),
        });
    }
    Ok(ByteOffset(rope.char_to_byte(scalar.0)))
}
pub fn clamp_byte(rope: &Rope, byte: ByteOffset) -> ByteOffset {
    ByteOffset(rope.char_to_byte(rope.byte_to_char(byte.0.min(rope.len_bytes()))))
}
pub fn line(rope: &Rope, line: LineIndex) -> Result<RopeSlice<'_>> {
    rope.get_line(line.0).ok_or(DomainError::InvalidCoordinate {
        unit: "line",
        index: line.0,
        limit: rope.len_lines().saturating_sub(1),
    })
}
pub fn content(rope: &Rope, index: LineIndex) -> Result<RopeSlice<'_>> {
    let line = line(rope, index)?;
    let mut end = line.len_chars();
    if end > 0 && line.char(end - 1) == '\n' {
        end -= 1;
        if end > 0 && line.char(end - 1) == '\r' {
            end -= 1;
        }
    }
    Ok(line.slice(..end))
}
pub fn line_end(rope: &Rope, index: LineIndex) -> Result<ByteOffset> {
    let content = content(rope, index)?;
    Ok(ByteOffset(rope.line_to_byte(index.0) + content.len_bytes()))
}
pub fn byte_to_position(rope: &Rope, byte: ByteOffset) -> Result<TextPosition> {
    let scalar = validate_byte(rope, byte)?;
    let line = rope.byte_to_line(byte.0);
    Ok(TextPosition {
        line: LineIndex(line),
        column: CharOffset(scalar.0 - rope.line_to_char(line)),
    })
}
pub fn position_to_byte(rope: &Rope, position: TextPosition) -> Result<ByteOffset> {
    let line = line(rope, position.line)?;
    let maximum = line.len_chars()
        - usize::from(line.len_chars() > 0 && line.char(line.len_chars() - 1) == '\n');
    if position.column.0 > maximum {
        return Err(DomainError::InvalidCoordinate {
            unit: "line scalar column",
            index: position.column.0,
            limit: maximum,
        });
    }
    Ok(ByteOffset(rope.char_to_byte(
        rope.line_to_char(position.line.0) + position.column.0,
    )))
}
pub fn byte_to_utf16(rope: &Rope, byte: ByteOffset) -> Result<Utf16Position> {
    let scalar = validate_byte(rope, byte)?;
    let line = rope.byte_to_line(byte.0);
    let start = rope.line_to_char(line);
    let column = rope.char_to_utf16_cu(scalar.0) - rope.char_to_utf16_cu(start);
    let column = u32::try_from(column).map_err(|_| DomainError::InvalidCoordinate {
        unit: "UTF-16 column",
        index: column,
        limit: u32::MAX as usize,
    })?;
    Ok(Utf16Position {
        line: LineIndex(line),
        column: Utf16Column(column),
    })
}
/// Legacy/LSP positions clamp to the preceding scalar boundary, including a
/// UTF-16 column in the middle of a surrogate pair. Out-of-range lines clamp too.
pub fn utf16_to_byte_clamped(rope: &Rope, position: Utf16Position) -> ByteOffset {
    let line_index = position.line.0.min(rope.len_lines() - 1);
    let line = rope.line(line_index);
    let start = rope.line_to_char(line_index);
    let end = start + line.len_chars()
        - usize::from(line.len_chars() > 0 && line.char(line.len_chars() - 1) == '\n');
    let base = rope.char_to_utf16_cu(start);
    let target = base
        .saturating_add(position.column.0 as usize)
        .min(rope.char_to_utf16_cu(end));
    ByteOffset(rope.char_to_byte(rope.utf16_cu_to_char(target)))
}
