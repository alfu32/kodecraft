use crate::document::TextSnapshot;
use crate::{ByteOffset, Result, Selection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretState {
    pub selection: Selection,
}
impl CaretState {
    pub fn cursor(&self) -> ByteOffset {
        self.selection.head
    }
    pub fn move_to(
        &mut self,
        snapshot: &TextSnapshot,
        position: ByteOffset,
        extend: bool,
    ) -> Result<()> {
        snapshot.byte_to_char(position)?;
        if !extend {
            self.selection.anchor = position;
        }
        self.selection.head = position;
        Ok(())
    }
    pub fn set_selection(&mut self, snapshot: &TextSnapshot, selection: Selection) -> Result<()> {
        snapshot.byte_to_char(selection.anchor)?;
        snapshot.byte_to_char(selection.head)?;
        self.selection = selection;
        Ok(())
    }
    pub fn restore_clamped(&mut self, snapshot: &TextSnapshot, selection: Selection) {
        self.selection = Selection {
            anchor: snapshot.clamp_byte(selection.anchor),
            head: snapshot.clamp_byte(selection.head),
        };
    }
    pub fn select_all(&mut self, snapshot: &TextSnapshot) {
        self.selection = Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(snapshot.len_bytes()),
        };
    }
    pub fn clear_selection(&mut self) {
        self.selection.anchor = self.selection.head;
    }
    pub fn selected_text(&self, snapshot: &TextSnapshot) -> Result<String> {
        Ok(snapshot.slice(self.selection.range())?.to_string())
    }
}
