use crate::{DomainError, Result};
use serde::{Deserialize, Deserializer, Serialize};

macro_rules! coordinate {
    ($name:ident, $inner:ty) => {
        #[derive(
            Debug,
            Default,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);
    };
}
coordinate!(ByteOffset, usize);
coordinate!(CharOffset, usize);
coordinate!(LineIndex, usize);
coordinate!(Utf16Column, u32);
coordinate!(CellColumn, u16);

/// A normalized half-open UTF-8 byte range. Validity against text is checked at
/// the document boundary; deserialization rejects reversed ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TextRange {
    pub start: ByteOffset,
    pub end: ByteOffset,
}

impl TextRange {
    pub fn new(start: ByteOffset, end: ByteOffset) -> Result<Self> {
        if start > end {
            return Err(DomainError::InvalidRange {
                start: start.0,
                end: end.0,
            });
        }
        Ok(Self { start, end })
    }

    pub fn caret(at: ByteOffset) -> Self {
        Self { start: at, end: at }
    }
    pub fn len(self) -> usize {
        self.end.0.saturating_sub(self.start.0)
    }
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
    pub fn contains(self, offset: ByteOffset) -> bool {
        self.start <= offset && offset < self.end
    }
    pub fn validate(self, text: &str) -> Result<Self> {
        Self::new(self.start, self.end)?;
        for offset in [self.start, self.end] {
            if offset.0 > text.len() || !text.is_char_boundary(offset.0) {
                return Err(DomainError::InvalidBoundary(offset.0));
            }
        }
        Ok(self)
    }
}

impl<'de> Deserialize<'de> for TextRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wire {
            start: ByteOffset,
            end: ByteOffset,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.start, wire.end).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub anchor: ByteOffset,
    pub head: ByteOffset,
}
impl Selection {
    pub fn range(self) -> TextRange {
        TextRange {
            start: self.anchor.min(self.head),
            end: self.anchor.max(self.head),
        }
    }
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }
    pub fn is_reverse(self) -> bool {
        self.head < self.anchor
    }
}
