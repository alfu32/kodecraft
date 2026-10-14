use super::selection::CaretState;
use crate::document::{Document, TextSnapshot, positions};
use crate::{DomainError, Result, Revision, Selection};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};

pub const HISTORY_LIMIT: usize = 200;
pub const HISTORY_BYTE_LIMIT: usize = 64 * 1024 * 1024;
pub const PERSIST_HISTORY_LIMIT: usize = 30;

#[derive(Debug, Clone)]
pub(crate) struct HistoryEntry {
    pub before: TextSnapshot,
    pub after: TextSnapshot,
    pub before_caret: Selection,
    pub after_caret: Selection,
    pub changed_bytes: usize,
}
#[derive(Debug, Default, Clone)]
pub(crate) struct History {
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    bytes: usize,
    omitted_undo: usize,
    omitted_redo: usize,
}
impl History {
    pub fn record(&mut self, entry: HistoryEntry) -> usize {
        for old in self.redo.drain(..) {
            self.bytes = self.bytes.saturating_sub(old.changed_bytes);
        }
        self.omitted_redo = 0;
        self.bytes = self.bytes.saturating_add(entry.changed_bytes);
        self.undo.push_back(entry);
        let mut discarded = 0;
        while self.undo.len() > HISTORY_LIMIT || self.bytes > HISTORY_BYTE_LIMIT {
            let Some(old) = self.undo.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(old.changed_bytes);
            discarded += 1;
        }
        self.omitted_undo = self.omitted_undo.saturating_add(discarded);
        discarded
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedBuffer {
    pub text: String,
    pub selection: Selection,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryStateV1 {
    pub version: u32,
    pub revision: Revision,
    #[serde(default)]
    pub saved_revision: Revision,
    pub current: PersistedBuffer,
    pub saved_text: String,
    pub undo: Vec<PersistedBuffer>,
    pub redo: Vec<PersistedBuffer>,
    pub omitted_undo: usize,
    pub omitted_redo: usize,
}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HistoryImportReport {
    pub discarded_undo: usize,
    pub discarded_redo: usize,
}

impl Document {
    pub fn undo(&mut self, caret: &mut CaretState) -> Result<bool> {
        if self.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        if self.history.undo.is_empty() {
            return Ok(false);
        }
        positions::validate_byte(&self.text, caret.selection.anchor)?;
        positions::validate_byte(&self.text, caret.selection.head)?;
        let revision = self
            .revision
            .0
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("document revision exhausted".into()))?;
        let mut entry = self
            .history
            .undo
            .pop_back()
            .expect("history checked nonempty");
        entry.after = self.snapshot();
        entry.after.flattened = Arc::default();
        entry.after_caret = caret.selection;
        self.text = entry.before.rope.clone();
        self.revision = Revision(revision);
        self.flattened = Arc::default();
        caret.selection = entry.before_caret;
        self.history.redo.push(entry);
        Ok(true)
    }
    pub fn redo(&mut self, caret: &mut CaretState) -> Result<bool> {
        if self.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        if self.history.redo.is_empty() {
            return Ok(false);
        }
        positions::validate_byte(&self.text, caret.selection.anchor)?;
        positions::validate_byte(&self.text, caret.selection.head)?;
        let revision = self
            .revision
            .0
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("document revision exhausted".into()))?;
        let mut entry = self.history.redo.pop().expect("history checked nonempty");
        entry.before = self.snapshot();
        entry.before.flattened = Arc::default();
        entry.before_caret = caret.selection;
        self.text = entry.after.rope.clone();
        self.revision = Revision(revision);
        self.flattened = Arc::default();
        caret.selection = entry.after_caret;
        self.history.undo.push_back(entry);
        Ok(true)
    }
    pub fn history_depth(&self) -> (usize, usize) {
        (self.history.undo.len(), self.history.redo.len())
    }
    pub fn export_history(&self, caret: &CaretState) -> HistoryStateV1 {
        let mut budget = HISTORY_BYTE_LIMIT;
        let mut undo = vec![];
        let mut redo = vec![];
        for entry in self.history.undo.iter().rev().take(PERSIST_HISTORY_LIMIT) {
            if entry.before.len_bytes() > budget {
                break;
            }
            budget -= entry.before.len_bytes();
            undo.push(PersistedBuffer {
                text: entry.before.rope.to_string(),
                selection: entry.before_caret,
            });
        }
        for entry in self.history.redo.iter().rev().take(PERSIST_HISTORY_LIMIT) {
            if entry.after.len_bytes() > budget {
                break;
            }
            budget -= entry.after.len_bytes();
            redo.push(PersistedBuffer {
                text: entry.after.rope.to_string(),
                selection: entry.after_caret,
            });
        }
        undo.reverse();
        redo.reverse();
        let undo_start = self.history.undo.len() - undo.len();
        let redo_start = self.history.redo.len() - redo.len();
        HistoryStateV1 {
            version: 1,
            revision: self.revision,
            saved_revision: self.saved_revision,
            current: PersistedBuffer {
                text: self.snapshot().text().to_string(),
                selection: caret.selection,
            },
            saved_text: self.saved_text.to_string(),
            undo,
            redo,
            omitted_undo: self.history.omitted_undo.saturating_add(undo_start),
            omitted_redo: self.history.omitted_redo.saturating_add(redo_start),
        }
    }
    pub fn import_history(
        &mut self,
        state: HistoryStateV1,
        caret: &mut CaretState,
    ) -> Result<HistoryImportReport> {
        if self.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        if state.version != 1 {
            return Err(DomainError::Unsupported(format!(
                "history version {}",
                state.version
            )));
        }
        if state.saved_revision > state.revision {
            return Err(DomainError::Unsupported(
                "saved history revision is in the future".into(),
            ));
        }
        let revision = self
            .revision
            .0
            .max(state.revision.0)
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("document revision exhausted".into()))?;
        let current = imported_snapshot(self, &state.current, Revision(revision))?;
        let mut history = History::default();
        let mut previous = current.clone();
        let mut reversed = vec![];
        for buffer in state.undo.iter().rev().take(HISTORY_LIMIT) {
            let before = imported_snapshot(self, buffer, Revision(0))?;
            let cost = before.len_bytes();
            if history.bytes.saturating_add(cost) > HISTORY_BYTE_LIMIT {
                break;
            }
            history.bytes += cost;
            reversed.push(HistoryEntry {
                before: before.clone(),
                after: previous,
                before_caret: buffer.selection,
                after_caret: state.current.selection,
                changed_bytes: cost,
            });
            previous = before;
        }
        for entry in reversed.into_iter().rev() {
            history.undo.push_back(entry);
        }
        for buffer in state
            .redo
            .iter()
            .rev()
            .take(HISTORY_LIMIT.saturating_sub(history.undo.len()))
        {
            let after = imported_snapshot(self, buffer, Revision(0))?;
            let cost = after.len_bytes();
            if history.bytes.saturating_add(cost) > HISTORY_BYTE_LIMIT {
                break;
            }
            history.bytes += cost;
            history.redo.push(HistoryEntry {
                before: current.clone(),
                after,
                before_caret: state.current.selection,
                after_caret: buffer.selection,
                changed_bytes: cost,
            });
        }
        history.redo.reverse();
        let report = HistoryImportReport {
            discarded_undo: state.undo.len() - history.undo.len(),
            discarded_redo: state.redo.len() - history.redo.len(),
        };
        history.omitted_undo = state.omitted_undo.saturating_add(report.discarded_undo);
        history.omitted_redo = state.omitted_redo.saturating_add(report.discarded_redo);
        self.text = current.rope;
        self.saved_text = Rope::from_str(&state.saved_text);
        self.saved_revision = state.saved_revision;
        self.revision = Revision(revision);
        self.flattened = Arc::default();
        self.history = history;
        caret.selection = state.current.selection;
        Ok(report)
    }
}
fn imported_snapshot(
    document: &Document,
    buffer: &PersistedBuffer,
    revision: Revision,
) -> Result<TextSnapshot> {
    let rope = Rope::from_str(&buffer.text);
    positions::validate_byte(&rope, buffer.selection.anchor)?;
    positions::validate_byte(&rope, buffer.selection.head)?;
    let mut snapshot = document.snapshot();
    snapshot.rope = rope;
    snapshot.revision = revision;
    snapshot.flattened = Arc::default();
    Ok(snapshot)
}
