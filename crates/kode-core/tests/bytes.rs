use kode_core::document::bytes::{ByteBuffer, ByteIndex, ByteViewport};

#[test]
fn all_byte_values_reverse_selection_clipboard_and_clone_are_exact() {
    let mut buffer = ByteBuffer::new((0..=255).collect());
    buffer.move_to(ByteIndex(255), false);
    buffer.move_to(ByteIndex(127), true);
    assert_eq!(buffer.anchor(), Some(ByteIndex(255)));
    assert_eq!(buffer.selection(), Some(127..255));
    assert_eq!(buffer.selection_bytes(), &(127..255).collect::<Vec<_>>());
    assert!(buffer.copy_selection());
    let clone = buffer.clone();
    assert!(buffer.cut_selection());
    assert_eq!(buffer.len(), 128);
    assert_eq!(buffer.cursor(), ByteIndex(127));
    assert!(buffer.paste_clipboard());
    assert_eq!(buffer.bytes(), &(0..=255).collect::<Vec<_>>());
    assert_eq!(clone.selection(), Some(127..255));
    assert_eq!(clone.len(), 256);
    // Invalid UTF-8 remains bytes; copying/pasting never goes through String.
    buffer.load(vec![0xff, 0xfe, 0x80, 0, b'a']);
    buffer.select_all();
    buffer.cut_selection();
    assert!(buffer.is_empty());
    buffer.paste_clipboard();
    assert_eq!(buffer.bytes(), &[0xff, 0xfe, 0x80, 0, b'a']);
}
#[test]
fn cursor_clamps_selection_replacement_deletion_and_empty_paste_match_source() {
    let mut buffer = ByteBuffer::default();
    buffer.move_left(false);
    buffer.move_right(false);
    buffer.move_end(true);
    assert_eq!(buffer.cursor(), ByteIndex(0));
    assert!(!buffer.delete_forward());
    assert!(!buffer.delete_backspace());
    buffer.load(vec![1, 2, 3, 4]);
    buffer.move_end(false);
    buffer.move_up(16, false);
    assert_eq!(buffer.cursor(), ByteIndex(0));
    buffer.move_down(usize::MAX, false);
    assert_eq!(buffer.cursor(), ByteIndex(4));
    buffer.start_selection(ByteIndex(4));
    buffer.select_to(ByteIndex(1));
    assert!(!buffer.insert(&[]));
    assert_eq!(buffer.selection(), Some(1..4));
    buffer.insert(&[255, 128]);
    assert_eq!(buffer.bytes(), &[1, 255, 128]);
    assert_eq!(buffer.cursor(), ByteIndex(3));
    assert!(buffer.delete_backspace());
    buffer.move_start(false);
    assert!(buffer.delete_forward());
    assert_eq!(buffer.bytes(), &[255]);
    buffer.start_selection(ByteIndex(99));
    assert_eq!(buffer.anchor(), Some(ByteIndex(1)));
    buffer.clear_selection();
    assert!(buffer.selection().is_none());
}
#[test]
fn viewport_partial_rows_masks_and_extreme_bounds_never_use_text_offsets() {
    let mut buffer = ByteBuffer::new((0..35).collect());
    buffer.move_to(ByteIndex(32), false);
    buffer.move_to(ByteIndex(14), true);
    let rows = buffer.viewport(ByteViewport {
        start: ByteIndex(0),
        bytes_per_row: 16,
        rows: 3,
    });
    assert_eq!(
        rows.iter().map(|row| row.bytes.len()).collect::<Vec<_>>(),
        vec![16, 16, 3]
    );
    assert_eq!(rows[1].offset, ByteIndex(16));
    assert!(rows[1].selected.iter().all(|value| *value));
    assert_eq!(rows[0].selected.iter().filter(|value| **value).count(), 2);
    assert!(rows[2].selected.iter().all(|value| !*value));
    assert!(
        buffer
            .viewport(ByteViewport {
                start: ByteIndex(usize::MAX),
                bytes_per_row: usize::MAX,
                rows: usize::MAX
            })
            .is_empty()
    );
    assert_eq!(
        buffer
            .viewport(ByteViewport {
                start: ByteIndex(34),
                bytes_per_row: 0,
                rows: usize::MAX
            })
            .len(),
        1
    );
}
