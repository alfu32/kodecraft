use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, LineIndex, Revision, Selection, TextRange,
};
use kode_core::{commands::EditKind, document::Document};
use kode_tui::{
    editor::{
        display::display_text,
        layout::{CollapsedFold, LayoutCache, VisualLayout},
        render::{
            EditorGeometry, EditorHeader, EditorHit, OverlaySpan, RenderOptions, RenderState,
            render_editor,
        },
    },
    theme::Theme,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
use std::sync::Arc;

fn document(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "buffer".into(),
            label: "synthetic".into(),
        },
        Some("text".into()),
        text,
    )
}
fn source_row(layout: &VisualLayout, row: usize) -> String {
    let row = &layout.rows[row];
    layout.lines[row.line.0].runs[row.run_start..row.run_end]
        .iter()
        .map(|run| run.glyph.text())
        .collect()
}
fn text_row(buffer: &Buffer, area: Rect, row: u16) -> String {
    (area.x..area.right())
        .map(|x| buffer[(x, area.y + row)].symbol())
        .collect()
}

#[test]
fn compatibility_display_matches_fixed_tabs_controls_emoji_wide_and_combining() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/editor/display.json")).unwrap();
    assert_eq!(
        display_text(fixture["input"].as_str().unwrap()),
        fixture["display"].as_str().unwrap()
    );
    assert_eq!(unicode_general_category::UNICODE_VERSION, (15, 0, 0));
    assert_eq!(
        display_text("\u{e000}\u{378}\u{200d}✅"),
        "\\xE000\\x0378\\x200D\\x2705"
    );
}

#[test]
fn source_wrapping_keeps_fitting_tab_markers_atomic_and_uses_one_inverse_map() {
    let doc = document(&"\t".repeat(100));
    let snapshot = doc.snapshot();
    let geometry = EditorGeometry::new(Rect::new(0, 0, 23, 4), snapshot.len_lines(), 0);
    assert_eq!(geometry.text.width, 20);
    let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
    assert_eq!(layout.rows.len(), 20);
    assert_eq!(source_row(&layout, 0), "|-->".repeat(5));
    assert_eq!(layout.position_to_cell(ByteOffset(15)).unwrap(), (3, 0));
    assert_eq!(layout.cell_to_position(3, 0), ByteOffset(15));
    for cell in 1..4 {
        assert_eq!(layout.cell_to_position(3, cell), ByteOffset(16));
    }
    for (text, expected) in [("a\tb", vec!["a|-->", "b"]), ("aa\tb", vec!["aa", "|-->b"])] {
        let layout = VisualLayout::build(&document(text).snapshot(), 5, &[]).unwrap();
        assert_eq!(
            (0..layout.rows.len())
                .map(|row| source_row(&layout, row))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn scalar_boundaries_roundtrip_render_and_hit_maps_without_utf16_units() {
    let doc = document("a\t\x01😀中e\u{301}\nlast");
    let snapshot = doc.snapshot();
    let layout = VisualLayout::build(&snapshot, 10, &[]).unwrap();
    for line in &layout.lines {
        for byte in line
            .runs
            .iter()
            .map(|run| run.source.start)
            .chain(std::iter::once(line.source.end))
        {
            let (row, col) = layout.position_to_cell(byte).unwrap();
            assert_eq!(layout.cell_to_position(row, col), byte);
            assert!(snapshot.byte_to_char(byte).is_ok());
        }
    }
    assert_eq!(
        layout.lines[0].runs[3].source,
        TextRange {
            start: ByteOffset(3),
            end: ByteOffset(7)
        }
    );
}

#[test]
fn header_gutter_selection_overlay_cursor_and_owned_surface_have_exact_cells() {
    let doc = document("ab;()\nnext");
    let snapshot = doc.snapshot();
    let area = Rect::new(10, 5, 25, 5);
    let geometry = EditorGeometry::new(area, snapshot.len_lines(), 0);
    let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
    let header = EditorHeader::for_document(&doc, Some("text/plain".into()), " (keyword)".into());
    let state = RenderState {
        secondary: vec![],
        selection: Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(1),
        },
        scroll_rows: 0,
        focused: true,
    };
    let theme = Theme::compiled();
    let overlays = [
        OverlaySpan {
            range: TextRange::new(ByteOffset(0), ByteOffset(2)).unwrap(),
            style: Style::default().fg(Color::Red),
            priority: 0,
        },
        OverlaySpan {
            range: TextRange::new(ByteOffset(1), ByteOffset(2)).unwrap(),
            style: Style::default().fg(Color::Green),
            priority: 2,
        },
    ];
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 24));
    for cell in &mut buffer.content {
        cell.set_symbol("Z");
    }
    let caret = render_editor(
        &layout,
        &mut buffer,
        &theme,
        RenderOptions {
            geometry,
            header: &header,
            state: &state,
            overlays: &overlays,
            warning_lines: &[],
            fold_starts: &[],
        },
    )
    .unwrap();
    assert!(text_row(&buffer, geometry.text, 0).starts_with("ab;()"));
    assert_eq!(buffer[(geometry.gutter.x, geometry.gutter.y)].symbol(), "1");
    assert_eq!(
        buffer[(geometry.gutter.x, geometry.gutter.y + 1)].symbol(),
        "2"
    );
    assert_eq!(
        buffer[(geometry.text.x, geometry.text.y)].bg,
        theme.style("code-selection").bg.unwrap()
    );
    assert_eq!(caret.x, geometry.text.x + 1);
    assert_eq!(buffer[caret].bg, theme.style("code-cursor").bg.unwrap());
    assert_eq!(buffer[(geometry.text.x + 2, geometry.text.y)].symbol(), ";");
    assert_eq!(buffer[(0, 0)].symbol(), "Z");
    assert_eq!(buffer[(geometry.text.x + 7, geometry.text.y)].symbol(), " ");
    assert_eq!(
        geometry.hit_test(&layout, 0, geometry.text.x, geometry.text.y),
        EditorHit::Text(ByteOffset(0))
    );
}

#[test]
fn exact_wrap_and_eof_cursor_never_erase_source_punctuation() {
    let doc = document("abc;()");
    let snapshot = doc.snapshot();
    let area = Rect::new(0, 0, 6, 4);
    let geometry = EditorGeometry::new(area, 1, 0);
    let layout = VisualLayout::build(&snapshot, 3, &[]).unwrap();
    assert_eq!(layout.position_to_cell(ByteOffset(3)).unwrap(), (1, 0));
    assert_eq!(layout.position_to_cell(ByteOffset(6)).unwrap(), (1, 3));
    let header = EditorHeader::for_document(&doc, None, String::new());
    let state = RenderState {
        secondary: vec![],
        selection: Selection {
            anchor: ByteOffset(6),
            head: ByteOffset(6),
        },
        focused: true,
        ..RenderState::default()
    };
    let mut buffer = Buffer::empty(area);
    render_editor(
        &layout,
        &mut buffer,
        &Theme::compiled(),
        RenderOptions {
            geometry,
            header: &header,
            state: &state,
            overlays: &[],
            warning_lines: &[],
            fold_starts: &[],
        },
    );
    assert_eq!(text_row(&buffer, geometry.text, 0), "abc");
    assert_eq!(text_row(&buffer, geometry.text, 1), ";()");
}

#[test]
fn cache_keys_include_identity_revision_width_and_folds() {
    let mut doc = document("head\nbody\ntail");
    let mut cache = LayoutCache::default();
    let first = cache.get(&doc.snapshot(), 10, &[]).unwrap();
    let same = cache.get(&doc.snapshot(), 10, &[]).unwrap();
    assert!(Arc::ptr_eq(&first, &same));
    let folded = cache
        .get(
            &doc.snapshot(),
            10,
            &[CollapsedFold {
                start: LineIndex(0),
                end: LineIndex(1),
            }],
        )
        .unwrap();
    assert_eq!(folded.rows.len(), 2);
    assert_eq!(folded.position_to_cell(ByteOffset(6)).unwrap().0, 0);
    assert!(!Arc::ptr_eq(&same, &folded));
    let narrow = cache.get(&doc.snapshot(), 2, &[]).unwrap();
    assert!(!Arc::ptr_eq(&narrow, &folded));
    doc.apply_edit(
        Revision(0),
        TextRange::caret(ByteOffset(0)),
        "new",
        EditKind::Typing,
    )
    .unwrap();
    let edited = cache.get(&doc.snapshot(), 2, &[]).unwrap();
    assert!(!Arc::ptr_eq(&narrow, &edited));
    assert_eq!(first.snapshot.text().as_ref(), "head\nbody\ntail");
    assert!(
        VisualLayout::build(
            &doc.snapshot(),
            2,
            &[CollapsedFold {
                start: LineIndex(0),
                end: LineIndex(99)
            }]
        )
        .is_err()
    );
}

#[test]
fn zero_tiny_width_and_search_chrome_do_not_underflow_or_draw_outside() {
    let doc = document("\t\x01😀\n");
    let snapshot = doc.snapshot();
    let header = EditorHeader::for_document(&doc, None, String::new());
    let theme = Theme::compiled();
    for (width, height) in [(80, 24), (120, 40), (20, 6), (4, 2), (1, 1), (0, 0)] {
        let area = Rect::new(0, 0, width, height);
        let geometry = EditorGeometry::new(area, snapshot.len_lines(), 2);
        let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
        let mut buffer = Buffer::empty(area);
        render_editor(
            &layout,
            &mut buffer,
            &theme,
            RenderOptions {
                geometry,
                header: &header,
                state: &RenderState::default(),
                overlays: &[],
                warning_lines: &[],
                fold_starts: &[],
            },
        );
        assert_eq!(buffer.content.len(), width as usize * height as usize);
    }
}

#[test]
fn multiple_ranges_and_carets_paint_empty_lines_and_eof_with_only_primary_terminal_cursor() {
    let doc = document("abc\n\nz");
    let snapshot = doc.snapshot();
    let area = Rect::new(3, 2, 30, 7);
    let geometry = EditorGeometry::new(area, snapshot.len_lines(), 0);
    let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
    let state = RenderState {
        selection: Selection {
            anchor: ByteOffset(0),
            head: ByteOffset(1),
        },
        secondary: vec![
            Selection {
                anchor: ByteOffset(2),
                head: ByteOffset(3),
            },
            Selection {
                anchor: ByteOffset(4),
                head: ByteOffset(4),
            },
            Selection {
                anchor: ByteOffset(6),
                head: ByteOffset(6),
            },
        ],
        scroll_rows: 0,
        focused: true,
    };
    let theme = Theme::compiled();
    let header = EditorHeader::for_document(&doc, None, String::new());
    let mut buffer = Buffer::empty(area);
    let cursor = render_editor(
        &layout,
        &mut buffer,
        &theme,
        RenderOptions {
            geometry,
            header: &header,
            state: &state,
            overlays: &[],
            warning_lines: &[],
            fold_starts: &[],
        },
    )
    .unwrap();
    assert_eq!(
        cursor,
        ratatui::layout::Position::new(geometry.text.x + 1, geometry.text.y)
    );
    for column in [0, 2] {
        assert_eq!(
            buffer[(geometry.text.x + column, geometry.text.y)].bg,
            theme.style("code-selection").bg.unwrap()
        );
    }
    for (row, column) in [(0, 1), (0, 3), (1, 0), (2, 1)] {
        assert_eq!(
            buffer[(geometry.text.x + column, geometry.text.y + row)].bg,
            theme.style("code-cursor").bg.unwrap()
        );
    }
    for width in 0..=8 {
        for height in 0..=4 {
            let area = Rect::new(3, 2, width, height);
            let geometry = EditorGeometry::new(area, snapshot.len_lines(), 0);
            let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
            let mut buffer = Buffer::empty(area);
            render_editor(
                &layout,
                &mut buffer,
                &theme,
                RenderOptions {
                    geometry,
                    header: &header,
                    state: &state,
                    overlays: &[],
                    warning_lines: &[],
                    fold_starts: &[],
                },
            );
        }
    }
}
