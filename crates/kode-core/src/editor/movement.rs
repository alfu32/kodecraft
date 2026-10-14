use super::selection::CaretState;
use crate::{ByteOffset, CharOffset, DomainError, LineIndex, Result};
use crate::{
    commands::Movement,
    document::{TextPosition, TextSnapshot},
};

pub fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}

fn column_target(
    snapshot: &TextSnapshot,
    line: LineIndex,
    column: CharOffset,
) -> Result<ByteOffset> {
    let column = CharOffset(column.0.min(snapshot.line_content(line)?.len_chars()));
    snapshot.position_to_byte(TextPosition { line, column })
}
fn word_left(snapshot: &TextSnapshot, at: ByteOffset) -> Result<ByteOffset> {
    let position = snapshot.byte_to_position(at)?;
    let mut line = position.line;
    let mut column = position
        .column
        .0
        .min(snapshot.line_content(line)?.len_chars());
    if column == 0 {
        if line.0 == 0 {
            return Ok(ByteOffset(0));
        }
        line.0 -= 1;
        column = snapshot.line_content(line)?.len_chars();
    }
    let content = snapshot.line_content(line)?;
    if column == 0 {
        return snapshot.line_start(line);
    }
    let mut index = column - 1;
    while index > 0 && !is_word_char(content.char(index)) {
        index -= 1;
    }
    while index > 0 && is_word_char(content.char(index - 1)) {
        index -= 1;
    }
    column_target(snapshot, line, CharOffset(index))
}
fn word_right(snapshot: &TextSnapshot, at: ByteOffset) -> Result<ByteOffset> {
    let position = snapshot.byte_to_position(at)?;
    let line = position.line;
    let content = snapshot.line_content(line)?;
    let mut index = position.column.0.min(content.len_chars());
    if index == content.len_chars() {
        return if line.0 + 1 < snapshot.len_lines() {
            snapshot.line_start(LineIndex(line.0 + 1))
        } else {
            snapshot.line_end(line)
        };
    }
    while index < content.len_chars() && !is_word_char(content.char(index)) {
        index += 1;
    }
    while index < content.len_chars() && is_word_char(content.char(index)) {
        index += 1;
    }
    column_target(snapshot, line, CharOffset(index))
}

/// Logical arrow movement is independent of wrapping. Only page commands receive
/// a target from the visual layout; movement never reads terminal geometry.
pub fn move_caret(
    snapshot: &TextSnapshot,
    state: &mut CaretState,
    movement: Movement,
    extend: bool,
    page_target: Option<ByteOffset>,
) -> Result<()> {
    let cursor = state.cursor();
    let position = snapshot.byte_to_position(cursor)?;
    let target = match movement {
        Movement::Left => {
            if cursor == snapshot.line_start(position.line)? && position.line.0 > 0 {
                snapshot.line_end(LineIndex(position.line.0 - 1))?
            } else {
                let scalar = snapshot.byte_to_char(cursor)?.0;
                snapshot.char_to_byte(CharOffset(scalar.saturating_sub(1)))?
            }
        }
        Movement::Right => {
            if cursor >= snapshot.line_end(position.line)?
                && position.line.0 + 1 < snapshot.len_lines()
            {
                snapshot.line_start(LineIndex(position.line.0 + 1))?
            } else {
                let scalar = snapshot.byte_to_char(cursor)?.0;
                snapshot.char_to_byte(CharOffset((scalar + 1).min(snapshot.len_chars())))?
            }
        }
        Movement::Up => {
            if position.line.0 == 0 {
                ByteOffset(0)
            } else {
                column_target(snapshot, LineIndex(position.line.0 - 1), position.column)?
            }
        }
        Movement::Down => {
            if position.line.0 + 1 == snapshot.len_lines() {
                snapshot.line_end(position.line)?
            } else {
                column_target(snapshot, LineIndex(position.line.0 + 1), position.column)?
            }
        }
        Movement::LineStart => snapshot.line_start(position.line)?,
        Movement::LineEnd => snapshot.line_end(position.line)?,
        Movement::DocumentStart => ByteOffset(0),
        Movement::DocumentEnd => ByteOffset(snapshot.len_bytes()),
        Movement::WordLeft => word_left(snapshot, cursor)?,
        Movement::WordRight => word_right(snapshot, cursor)?,
        Movement::PageUp | Movement::PageDown => page_target.ok_or_else(|| {
            DomainError::Unsupported("page navigation requires a visual layout target".into())
        })?,
    };
    state.move_to(snapshot, target, extend)
}
