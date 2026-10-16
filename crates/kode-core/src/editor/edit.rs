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
    pub caret: CaretState,
    pub history_discarded: usize,
}

impl Document {
    /// All edits refer to the original snapshot; disjoint replacements are applied
    /// in descending byte order and committed as one revision/undo transaction.
    pub fn apply_transaction(
        &mut self,
        expected: Revision,
        edits: Vec<TextEdit>,
        kind: EditKind,
        before_caret: Selection,
        after_caret: Option<Selection>,
    ) -> Result<Option<EditOutcome>> {
        self.apply_caret_transaction(
            expected,
            edits,
            kind,
            &CaretState::single(before_caret),
            after_caret.map(CaretState::single),
        )
    }
    /// Multi-selection variant with complete, atomically validated undo state.
    /// The single-selection API remains available to explicit-range callers.
    pub fn apply_caret_transaction(
        &mut self,
        expected: Revision,
        mut edits: Vec<TextEdit>,
        kind: EditKind,
        before_caret: &CaretState,
        after_caret: Option<CaretState>,
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
        let before_caret = before_caret.normalized(&self.snapshot())?;
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
        let caret = after_caret.unwrap_or_else(|| {
            CaretState::single(Selection {
                anchor: ByteOffset(end),
                head: ByteOffset(end),
            })
        });
        let mut candidate_snapshot = self.snapshot();
        candidate_snapshot.rope = candidate.clone();
        candidate_snapshot.flattened = Arc::default();
        let caret = caret.normalized(&candidate_snapshot)?;
        let selection = caret.selection;
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
            after_caret: caret.clone(),
            changed_bytes,
        });
        Ok(Some(EditOutcome {
            deltas,
            selection,
            caret,
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
    let end = range
        .start
        .0
        .checked_add(text.len())
        .ok_or_else(|| DomainError::Unsupported("edit coordinate overflow".into()))?;
    let remap = |offset: ByteOffset| {
        if offset < range.start {
            offset
        } else if offset < range.end {
            ByteOffset(end)
        } else {
            ByteOffset(end.saturating_add(offset.0.saturating_sub(range.end.0)))
        }
    };
    let after = CaretState {
        selection: Selection {
            anchor: ByteOffset(end),
            head: ByteOffset(end),
        },
        secondary: caret
            .secondary
            .iter()
            .map(|selection| Selection {
                anchor: remap(selection.anchor),
                head: remap(selection.head),
            })
            .collect(),
    };
    let result = doc.apply_caret_transaction(
        doc.revision(),
        vec![TextEdit {
            range,
            inserted: text.into(),
        }],
        kind,
        caret,
        Some(after),
    )?;
    if let Some(outcome) = &result {
        *caret = outcome.caret.clone();
    }
    Ok(result)
}
pub fn insert(
    doc: &mut Document,
    caret: &mut CaretState,
    text: &str,
    kind: EditKind,
) -> Result<Option<EditOutcome>> {
    let targets = caret.normalized(&doc.snapshot())?;
    apply_selections(doc, caret, &targets, kind, |_, selection| {
        Ok(TextEdit {
            range: selection.range(),
            inserted: text.into(),
        })
    })
}
pub fn newline(doc: &mut Document, caret: &mut CaretState) -> Result<Option<EditOutcome>> {
    let targets = caret.normalized(&doc.snapshot())?;
    apply_selections(
        doc,
        caret,
        &targets,
        EditKind::Newline,
        |snapshot, selection| {
            let position = snapshot.byte_to_position(selection.head)?;
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
            Ok(TextEdit {
                range: selection.range(),
                inserted: format!("{terminator}{indent}"),
            })
        },
    )
}
pub fn delete(
    doc: &mut Document,
    caret: &mut CaretState,
    backward: bool,
) -> Result<Option<EditOutcome>> {
    let snapshot = doc.snapshot();
    let mut targets = caret.normalized(&snapshot)?;
    for selection in std::iter::once(&mut targets.selection).chain(targets.secondary.iter_mut()) {
        if !selection.is_empty() {
            continue;
        }
        let mut other = CaretState::single(*selection);
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
        *selection = other.selection;
    }
    let targets = targets.normalized(&snapshot)?;
    apply_selections(doc, caret, &targets, EditKind::Delete, |_, selection| {
        Ok(TextEdit {
            range: selection.range(),
            inserted: String::new(),
        })
    })
}

fn apply_selections(
    doc: &mut Document,
    caret: &mut CaretState,
    targets: &CaretState,
    kind: EditKind,
    mut plan: impl FnMut(&crate::document::TextSnapshot, Selection) -> Result<TextEdit>,
) -> Result<Option<EditOutcome>> {
    let snapshot = doc.snapshot();
    let mut plans = targets
        .selections()
        .enumerate()
        .map(|(index, selection)| plan(&snapshot, selection).map(|edit| (edit, index == 0)))
        .collect::<Result<Vec<_>>>()?;
    plans.sort_by_key(|(edit, _)| edit.range.start);
    let mut shift = 0i128;
    let mut after = CaretState::default();
    let mut edits = Vec::with_capacity(plans.len());
    for (edit, primary) in plans {
        let position =
            usize::try_from(edit.range.start.0 as i128 + shift + edit.inserted.len() as i128)
                .map_err(|_| DomainError::Unsupported("edit coordinate overflow".into()))?;
        let selection = Selection {
            anchor: ByteOffset(position),
            head: ByteOffset(position),
        };
        if primary {
            after.selection = selection;
        } else {
            after.secondary.push(selection);
        }
        shift += edit.inserted.len() as i128 - edit.range.len() as i128;
        edits.push(edit);
    }
    let result = doc.apply_caret_transaction(snapshot.revision, edits, kind, caret, Some(after))?;
    if let Some(outcome) = &result {
        *caret = outcome.caret.clone();
    }
    Ok(result)
}
