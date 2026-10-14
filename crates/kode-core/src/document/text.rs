use super::TextSnapshot;
use crate::commands::EditKind;
use crate::{ByteOffset, DocumentId, DocumentOrigin, Result, Revision, TextRange};
use ropey::Rope;
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDelta {
    pub range: TextRange,
    pub removed: String,
    pub inserted: String,
    pub previous_revision: Revision,
    pub revision: Revision,
    pub kind: EditKind,
}
impl EditDelta {
    pub fn inserted_range(&self) -> TextRange {
        TextRange {
            start: self.range.start,
            end: ByteOffset(self.range.start.0 + self.inserted.len()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Document {
    id: DocumentId,
    origin: DocumentOrigin,
    language: Option<String>,
    format: super::encoding::FileFormat,
    pub(crate) revision: Revision,
    pub(crate) saved_revision: Revision,
    pub(crate) text: Rope,
    pub(crate) saved_text: Rope,
    pub(crate) flattened: Arc<OnceLock<Arc<str>>>,
    read_only: bool,
    pub(crate) history: crate::editor::history::History,
}
impl Document {
    pub fn new(
        id: DocumentId,
        origin: DocumentOrigin,
        language: Option<String>,
        text: &str,
    ) -> Self {
        let text = Rope::from_str(text);
        Self {
            id,
            origin,
            language,
            format: super::encoding::FileFormat::default(),
            revision: Revision(0),
            saved_revision: Revision(0),
            saved_text: text.clone(),
            text,
            flattened: Arc::default(),
            read_only: false,
            history: crate::editor::history::History::default(),
        }
    }
    pub fn id(&self) -> DocumentId {
        self.id
    }
    pub fn from_decoded(
        id: DocumentId,
        origin: DocumentOrigin,
        language: Option<String>,
        decoded: super::encoding::DecodedText,
    ) -> Self {
        let mut document = Self::new(id, origin, language, &decoded.text);
        document.format = decoded.format;
        document
    }
    pub fn format(&self) -> super::encoding::FileFormat {
        self.format
    }
    pub fn origin(&self) -> &DocumentOrigin {
        &self.origin
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn saved_revision(&self) -> Revision {
        self.saved_revision
    }
    pub fn is_dirty(&self) -> bool {
        self.text != self.saved_text
    }
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }
    pub fn snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            id: self.id,
            revision: self.revision,
            origin: self.origin.clone(),
            language: self.language.clone(),
            format: self.format,
            rope: self.text.clone(),
            flattened: self.flattened.clone(),
        }
    }
    pub fn apply_edit(
        &mut self,
        expected: Revision,
        range: TextRange,
        inserted: &str,
        kind: EditKind,
    ) -> Result<Option<EditDelta>> {
        use crate::{Selection, editor::edit::TextEdit};
        let before = Selection {
            anchor: range.start,
            head: range.start,
        };
        Ok(self
            .apply_transaction(
                expected,
                vec![TextEdit {
                    range,
                    inserted: inserted.into(),
                }],
                kind,
                before,
                None,
            )?
            .map(|mut outcome| outcome.deltas.remove(0)))
    }
}
