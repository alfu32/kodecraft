use super::{history::HistoryEntry, movement::move_caret, selection::CaretState};
use crate::{ByteOffset, DomainError, Result, Revision, Selection, TextRange};
use crate::{
    commands::EditKind,
    document::{Document, EditDelta, positions},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: TextRange,
    pub inserted: String,
}
#[derive(Debug)]
pub struct EditOutcome {
    pub deltas: Vec<EditDelta>,
    pub selection: Selection,
    pub history_discarded: usize,
}

impl Document {
    /// All edits refer to the original snapshot; disjoint replacements are applied
    /// in descending byte order and committed as one revision/undo transaction.
    pub fn apply_transaction(
        &mut self,
        expected: Revision,
        mut edits: Vec<TextEdit>,
        kind: EditKind,
        before_caret: Selection,
        after_caret: Option<Selection>,
    ) -> Result<Option<EditOutcome>> {
        if self.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        if expected != self.revision {
            return Err(DomainError::StaleRevision {
                expected,
                current: self.revision,
            });
        }
        positions::validate_byte(&self.text, before_caret.anchor)?;
        positions::validate_byte(&self.text, before_caret.head)?;
        edits.sort_by_key(|edit| edit.range.start);
        let mut previous_end = 0;
        for edit in &edits {
            TextRange::new(edit.range.start, edit.range.end)?;
            positions::validate_byte(&self.text, edit.range.start)?;
            positions::validate_byte(&self.text, edit.range.end)?;
            if edit.range.start.0 < previous_end {
                return Err(DomainError::Unsupported(
                    "overlapping edits in one transaction".into(),
                ));
            }
            previous_end = edit.range.end.0;
        }
        edits.retain(|edit| {
            self.text.byte_slice(edit.range.start.0..edit.range.end.0) != edit.inserted.as_str()
        });
        if edits.is_empty() {
            return Ok(None);
        }
        let revision = Revision(
            self.revision
                .0
                .checked_add(1)
                .ok_or_else(|| DomainError::Unsupported("document revision exhausted".into()))?,
        );
        let mut before = self.snapshot();
        before.flattened = Arc::default();
        let mut candidate = self.text.clone();
        let mut deltas = vec![];
        for edit in edits.iter().rev() {
            let start = positions::validate_byte(&candidate, edit.range.start)?.0;
            let end = positions::validate_byte(&candidate, edit.range.end)?.0;
            let removed = candidate.slice(start..end).to_string();
            candidate.remove(start..end);
            candidate.insert(start, &edit.inserted);
            deltas.push(EditDelta {
                range: edit.range,
                removed,
                inserted: edit.inserted.clone(),
                previous_revision: self.revision,
                revision,
                kind,
            });
        }
        let mut shift = 0i128;
        let mut end = 0usize;
        for edit in &edits {
            end = usize::try_from(edit.range.start.0 as i128 + shift + edit.inserted.len() as i128)
                .map_err(|_| DomainError::Unsupported("edit coordinate overflow".into()))?;
            shift += edit.inserted.len() as i128 - edit.range.len() as i128;
        }
        let selection = after_caret.unwrap_or(Selection {
            anchor: ByteOffset(end),
            head: ByteOffset(end),
        });
        positions::validate_byte(&candidate, selection.anchor)?;
        positions::validate_byte(&candidate, selection.head)?;
        self.text = candidate;
        self.revision = revision;
        self.flattened = Arc::default();
        let changed_bytes = deltas
            .iter()
            .map(|delta| delta.removed.len().saturating_add(delta.inserted.len()))
            .fold(0usize, usize::saturating_add);
        let mut after = self.snapshot();
        after.flattened = Arc::default();
        let history_discarded = self.history.record(HistoryEntry {
            before,
            after,
            before_caret,
            after_caret: selection,
            changed_bytes,
        });
        Ok(Some(EditOutcome {
            deltas,
            selection,
            history_discarded,
        }))
    }
}

pub fn replace(
    doc: &mut Document,
    caret: &mut CaretState,
    range: TextRange,
    text: &str,
    kind: EditKind,
) -> Result<Option<EditOutcome>> {
    let result = doc.apply_transaction(
        doc.revision(),
        vec![TextEdit {
            range,
            inserted: text.into(),
        }],
        kind,
        caret.selection,
        None,
    )?;
    if let Some(outcome) = &result {
        caret.selection = outcome.selection;
    }
    Ok(result)
}
pub fn insert(
    doc: &mut Document,
    caret: &mut CaretState,
    text: &str,
    kind: EditKind,
) -> Result<Option<EditOutcome>> {
    replace(doc, caret, caret.selection.range(), text, kind)
}
pub fn newline(doc: &mut Document, caret: &mut CaretState) -> Result<Option<EditOutcome>> {
    let snapshot = doc.snapshot();
    let position = snapshot.byte_to_position(caret.cursor())?;
    let indent: String = snapshot
        .line_content(position.line)?
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let line = snapshot.line(position.line)?;
    let first_line = snapshot.line(crate::LineIndex(0))?;
    let crlf = |line: ropey::RopeSlice<'_>| {
        line.len_chars() >= 2
            && line.char(line.len_chars() - 1) == '\n'
            && line.char(line.len_chars() - 2) == '\r'
    };
    let terminator = if crlf(line) || crlf(first_line) {
        "\r\n"
    } else {
        "\n"
    };
    insert(
        doc,
        caret,
        &format!("{terminator}{indent}"),
        EditKind::Newline,
    )
}
pub fn delete(
    doc: &mut Document,
    caret: &mut CaretState,
    backward: bool,
) -> Result<Option<EditOutcome>> {
    if !caret.selection.is_empty() {
        return insert(doc, caret, "", EditKind::Delete);
    }
    let snapshot = doc.snapshot();
    let mut other = caret.clone();
    move_caret(
        &snapshot,
        &mut other,
        if backward {
            crate::commands::Movement::Left
        } else {
            crate::commands::Movement::Right
        },
        true,
        None,
    )?;
    replace(doc, caret, other.selection.range(), "", EditKind::Delete)
}
