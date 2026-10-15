//! Binary positions count bytes, independently of UTF-8 document coordinates.
use std::{ops::Range, sync::Arc};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteIndex(pub usize);
#[derive(Debug, Clone, Default)]
pub struct ByteBuffer {
    data: Arc<Vec<u8>>,
    cursor: ByteIndex,
    anchor: Option<ByteIndex>,
    clipboard: Arc<Vec<u8>>,
}
#[derive(Debug, Clone, Copy)]
pub struct ByteViewport {
    pub start: ByteIndex,
    pub bytes_per_row: usize,
    pub rows: usize,
}
#[derive(Debug)]
pub struct ByteViewportRow<'a> {
    pub offset: ByteIndex,
    pub bytes: &'a [u8],
    pub selected: Vec<bool>,
}
impl ByteBuffer {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            data: Arc::new(bytes),
            ..Default::default()
        }
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
    pub fn cursor(&self) -> ByteIndex {
        self.cursor
    }
    pub fn anchor(&self) -> Option<ByteIndex> {
        self.anchor
    }
    pub fn load(&mut self, bytes: Vec<u8>) {
        self.data = Arc::new(bytes);
        self.cursor = ByteIndex(0);
        self.anchor = None;
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?.0;
        (anchor != self.cursor.0).then(|| anchor.min(self.cursor.0)..anchor.max(self.cursor.0))
    }
    pub fn selection_bytes(&self) -> &[u8] {
        self.selection().map_or(&[], |range| &self.data[range])
    }
    pub fn move_to(&mut self, index: ByteIndex, expand: bool) {
        if expand {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = ByteIndex(index.0.min(self.len()));
    }
    pub fn move_by(&mut self, delta: isize, expand: bool) {
        self.move_to(
            ByteIndex(self.cursor.0.saturating_add_signed(delta)),
            expand,
        );
    }
    pub fn move_left(&mut self, expand: bool) {
        self.move_by(-1, expand);
    }
    pub fn move_right(&mut self, expand: bool) {
        self.move_by(1, expand);
    }
    pub fn move_up(&mut self, width: usize, expand: bool) {
        self.move_to(
            ByteIndex(self.cursor.0.saturating_sub(width.max(1))),
            expand,
        );
    }
    pub fn move_down(&mut self, width: usize, expand: bool) {
        self.move_to(
            ByteIndex(self.cursor.0.saturating_add(width.max(1))),
            expand,
        );
    }
    pub fn move_start(&mut self, expand: bool) {
        self.move_to(ByteIndex(0), expand);
    }
    pub fn move_end(&mut self, expand: bool) {
        self.move_to(ByteIndex(self.len()), expand);
    }
    pub fn select_all(&mut self) {
        self.anchor = Some(ByteIndex(0));
        self.cursor = ByteIndex(self.len());
    }
    pub fn start_selection(&mut self, index: ByteIndex) {
        self.cursor = ByteIndex(index.0.min(self.len()));
        self.anchor = Some(self.cursor);
    }
    pub fn select_to(&mut self, index: ByteIndex) {
        self.move_to(index, true);
    }
    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.cursor = ByteIndex(range.start);
        self.anchor = None;
        Arc::make_mut(&mut self.data).drain(range);
        true
    }
    pub fn insert(&mut self, bytes: &[u8]) -> bool {
        // The source does not delete a selection when pasting an empty clipboard.
        if bytes.is_empty() {
            return false;
        }
        self.delete_selection();
        Arc::make_mut(&mut self.data).splice(self.cursor.0..self.cursor.0, bytes.iter().copied());
        self.cursor.0 += bytes.len();
        self.anchor = None;
        true
    }
    pub fn delete_backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor.0 == 0 {
            return false;
        }
        self.cursor.0 -= 1;
        Arc::make_mut(&mut self.data).remove(self.cursor.0);
        self.anchor = None;
        true
    }
    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor.0 >= self.len() {
            return false;
        }
        Arc::make_mut(&mut self.data).remove(self.cursor.0);
        self.anchor = None;
        true
    }
    /// Like the reference byte buffer, the clipboard contains exact local bytes.
    /// No UTF-8 decoding or lossy native text conversion is involved.
    pub fn copy_selection(&mut self) -> bool {
        let Some(range) = self.selection() else {
            return false;
        };
        self.clipboard = Arc::new(self.data[range].to_vec());
        true
    }
    pub fn cut_selection(&mut self) -> bool {
        self.copy_selection() && self.delete_selection()
    }
    pub fn paste_clipboard(&mut self) -> bool {
        let bytes = self.clipboard.clone();
        self.insert(&bytes)
    }
    pub fn viewport(&self, view: ByteViewport) -> Vec<ByteViewportRow<'_>> {
        let width = view.bytes_per_row.max(1);
        let selection = self.selection();
        self.data
            .get(view.start.0..)
            .unwrap_or_default()
            .chunks(width)
            .take(view.rows)
            .enumerate()
            .map(|(row, bytes)| {
                let offset = view.start.0 + row * width;
                ByteViewportRow {
                    offset: ByteIndex(offset),
                    bytes,
                    selected: (offset..offset + bytes.len())
                        .map(|index| {
                            selection
                                .as_ref()
                                .is_some_and(|range| range.contains(&index))
                        })
                        .collect(),
                }
            })
            .collect()
    }
}
