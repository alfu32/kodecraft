use super::{Document, TextSnapshot};
use crate::{DomainError, Result};

impl Document {
    /// A completed older save establishes that snapshot as the disk baseline;
    /// edits made while it was running remain dirty and retain their history.
    pub fn acknowledge_save(&mut self, snapshot: &TextSnapshot) -> Result<()> {
        if snapshot.id != self.id()
            || snapshot.origin != *self.origin()
            || snapshot.format != self.format()
        {
            return Err(DomainError::Unsupported(
                "save belongs to a different document or target".into(),
            ));
        }
        if snapshot.revision > self.revision || snapshot.revision < self.saved_revision {
            return Err(DomainError::StaleRevision {
                expected: snapshot.revision,
                current: self.saved_revision,
            });
        }
        self.saved_text = snapshot.rope.clone();
        self.saved_revision = snapshot.revision;
        Ok(())
    }
}
