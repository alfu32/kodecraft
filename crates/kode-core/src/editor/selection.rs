use crate::document::TextSnapshot;
use crate::{ByteOffset, Result, Selection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretState {
    pub selection: Selection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary: Vec<Selection>,
}
impl CaretState {
    /// Make an additional selection primary, retaining the others. Duplicate or
    /// overlapping positions are normalized before any edit is applied.
    pub fn add(&mut self, snapshot: &TextSnapshot, selection: Selection) -> Result<()> {
        let candidate = Self {
            selection,
            secondary: self.selections().collect(),
        };
        *self = candidate.normalized(snapshot)?;
        Ok(())
    }
    /// Toggle by head position, keeping at least one valid caret. Returns true
    /// when an existing cursor was removed (the final cursor is retained).
    pub fn toggle(&mut self, snapshot: &TextSnapshot, position: ByteOffset) -> Result<bool> {
        let mut candidate = self.normalized(snapshot)?;
        if candidate.selection.head == position {
            if !candidate.secondary.is_empty() {
                candidate.selection = candidate.secondary.remove(0);
            }
            *self = candidate;
            return Ok(true);
        }
        if let Some(index) = candidate
            .secondary
            .iter()
            .position(|selection| selection.head == position)
        {
            candidate.secondary.remove(index);
            *self = candidate;
            return Ok(true);
        }
        candidate.add(
            snapshot,
            Selection {
                anchor: position,
                head: position,
            },
        )?;
        *self = candidate;
        Ok(false)
    }
    pub fn single(selection: Selection) -> Self {
        Self {
            selection,
            secondary: vec![],
        }
    }
    /// The primary selection is first; secondary selections have no independent
    /// document identity and always refer to the same immutable snapshot.
    pub fn selections(&self) -> impl Iterator<Item = Selection> + '_ {
        std::iter::once(self.selection).chain(self.secondary.iter().copied())
    }
    pub fn validate(&self, snapshot: &TextSnapshot) -> Result<()> {
        for selection in self.selections() {
            snapshot.byte_to_char(selection.anchor)?;
            snapshot.byte_to_char(selection.head)?;
        }
        Ok(())
    }
    /// Merge overlapping ranges and duplicate/contained carets. Adjacent,
    /// nonempty ranges remain distinct replacements. A merged primary keeps its
    /// direction; other groups use their first selection's direction.
    pub fn normalized(&self, snapshot: &TextSnapshot) -> Result<Self> {
        self.validate(snapshot)?;
        let mut sorted: Vec<_> = self
            .selections()
            .enumerate()
            .map(|(index, selection)| (selection, index == 0))
            .collect();
        sorted.sort_by_key(|(selection, _)| (selection.range().start, selection.range().end));
        let mut groups: Vec<(Selection, bool)> = vec![];
        for (selection, primary) in sorted {
            if let Some((last, last_primary)) = groups.last_mut() {
                let left = last.range();
                let right = selection.range();
                if left.end > right.start
                    || (left.end == right.start && (left.is_empty() || right.is_empty()))
                {
                    let reversed = if primary {
                        selection.anchor > selection.head
                    } else {
                        last.anchor > last.head
                    };
                    let start = left.start.min(right.start);
                    let end = left.end.max(right.end);
                    *last = if reversed {
                        Selection {
                            anchor: end,
                            head: start,
                        }
                    } else {
                        Selection {
                            anchor: start,
                            head: end,
                        }
                    };
                    *last_primary |= primary;
                    continue;
                }
            }
            groups.push((selection, primary));
        }
        let primary = groups
            .iter()
            .position(|(_, primary)| *primary)
            .expect("primary retained");
        let selection = groups.remove(primary).0;
        Ok(Self {
            selection,
            secondary: groups.into_iter().map(|(selection, _)| selection).collect(),
        })
    }
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
        self.secondary.clear();
        Ok(())
    }
    pub fn restore_clamped(&mut self, snapshot: &TextSnapshot, selection: Selection) {
        self.selection = Selection {
            anchor: snapshot.clamp_byte(selection.anchor),
            head: snapshot.clamp_byte(selection.head),
        };
        for selection in &mut self.secondary {
            selection.anchor = snapshot.clamp_byte(selection.anchor);
            selection.head = snapshot.clamp_byte(selection.head);
        }
    }
    pub fn select_all(&mut self, snapshot: &TextSnapshot) {
        self.secondary.clear();
        self.selection = Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(snapshot.len_bytes()),
        };
    }
    pub fn clear_selection(&mut self) {
        self.selection.anchor = self.selection.head;
        for selection in &mut self.secondary {
            selection.anchor = selection.head;
        }
    }
    pub fn selected_text(&self, snapshot: &TextSnapshot) -> Result<String> {
        Ok(snapshot.slice(self.selection.range())?.to_string())
    }
}
