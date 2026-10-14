use crossterm::event::MouseButton;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, JobTag, LineIndex, RequestId,
    WorkspaceGeneration, commands::EditKind, document::Document,
};
use kode_platform::jobs::{MonotonicClock, Notifications, OwnerId, Scheduler};
use kode_tui::{
    editor::{
        folding::{EditorVisuals, FoldReport, FoldState, discover, schedule_discovery},
        input::{EditorInput, InputViewport},
        layout::VisualLayout,
        minimap::{DOTS, Minimap},
        render::{EditorGeometry, EditorHeader, EditorHit, RenderOptions, render_editor},
    },
    focus::{FocusId, FocusState},
    input::terminal::*,
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::{sync::Arc, time::Duration};

fn document(text: &str, language: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "test".into(),
            key: "fold".into(),
            label: "fold.kt".into(),
        },
        Some(language.into()),
        text,
    )
}
fn folds(doc: &Document) -> FoldState {
    let mut folds = FoldState::default();
    assert!(folds.publish(
        discover(&doc.snapshot(), || false).unwrap(),
        &doc.snapshot()
    ));
    folds
}
fn mouse(x: u16, y: u16) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
fn key(key: Key, shift: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        repeat: false,
        modifiers: Modifiers {
            shift,
            ..Default::default()
        },
    })
}

#[test]
fn brace_discovery_excludes_comments_quotes_multiline_strings_and_handles_nesting() {
    let doc = document(
        "fun main() {\n // } {\n \"} {\"; '\\''; `}`;\n /* }\n { */\n if (ok) {\n  run()\n }\n}",
        "kotlin",
    );
    let result = discover(&doc.snapshot(), || false).unwrap();
    let ranges: Vec<_> = result
        .regions
        .iter()
        .map(|region| (region.start.0, region.end.0))
        .collect();
    assert_eq!(ranges, [(0, 8), (5, 7)]);
    let triple = document("val s = \"\"\"\n {\n }\n\"\"\"\nfun f() {\n}", "kotlin");
    let result = discover(&triple.snapshot(), || false).unwrap();
    assert_eq!(result.regions.len(), 1);
    assert_eq!(result.regions[0].start, LineIndex(4));
    let nested_comments = document(
        "/* outer\n /* inner */\n {\n }\n */\nfun f() {\n}",
        "kotlin",
    );
    let result = discover(&nested_comments.snapshot(), || false).unwrap();
    assert_eq!(result.regions.len(), 1);
    assert_eq!(result.regions[0].start, LineIndex(5));
    let raw = document("val s = \"\"\"backslash\\\"\"\"\nfun f() {\n}", "kotlin");
    assert_eq!(
        discover(&raw.snapshot(), || false).unwrap().regions[0].start,
        LineIndex(1)
    );
    assert!(matches!(
        discover(&doc.snapshot(), || true),
        Err(DomainError::Cancelled)
    ));
}

#[test]
fn indentation_languages_preserve_blank_lines_tabs_block_rules_and_ruby_keywords() {
    for language in ["python", "yaml", "yml", "bash", "powershell"] {
        let doc = document("outer:\n    inner:\n\t\twork\n\n    next\nend", language);
        let result = discover(&doc.snapshot(), || false).unwrap();
        assert_eq!(
            result
                .regions
                .iter()
                .map(|region| (region.start.0, region.end.0))
                .collect::<Vec<_>>(),
            [(0, 4), (1, 3)]
        );
    }
    let doc = document("class X\n  def f\n    work\n  end\nend", "ruby");
    assert_eq!(
        discover(&doc.snapshot(), || false)
            .unwrap()
            .regions
            .iter()
            .map(|r| (r.start.0, r.end.0))
            .collect::<Vec<_>>(),
        [(0, 3), (1, 2)]
    );
    let doc = document("# { ignored }\na:\n    b", "python");
    assert_eq!(
        discover(&doc.snapshot(), || false).unwrap().regions.len(),
        1
    );
}

#[test]
fn source_gutter_fold_summary_ellipsis_and_closing_tail_share_the_hit_map() {
    let text = "fun main() {\n    println(\"one\")\n    println(\"two\")\n}";
    let mut doc = document(text, "kotlin");
    let mut owner = EditorVisuals { folds: folds(&doc) };
    let mut input = EditorInput::new(FocusId(1), doc.id());
    let mut focus = FocusState {
        focused: Some(input.id),
        ..Default::default()
    };
    let area = Rect::new(10, 5, 80, 8);
    let geometry = EditorGeometry::new(area, doc.snapshot().len_lines(), 0);
    let viewport = InputViewport {
        geometry,
        folds: &[],
    };
    input
        .dispatch(
            &mouse(geometry.gutter.x, geometry.gutter.y),
            &mut doc,
            &mut focus,
            viewport,
            &mut owner,
        )
        .unwrap();
    assert_eq!(focus.captured, None);
    let collapsed = owner.folds.collapsed(&doc.snapshot());
    let layout = input
        .layout(
            &doc,
            InputViewport {
                geometry,
                folds: &collapsed,
            },
        )
        .unwrap();
    assert_eq!(layout.rows.len(), 1);
    let mut buffer = Buffer::empty(area);
    let starts: Vec<_> = owner.folds.regions().map(|region| region.start).collect();
    render_editor(
        &layout,
        &mut buffer,
        &Theme::compiled(),
        RenderOptions {
            geometry,
            header: &EditorHeader::for_document(&doc, None, String::new()),
            state: &input.render_state(&focus),
            overlays: &[],
            warning_lines: &[],
            fold_starts: &starts,
        },
    );
    let rendered: String = (geometry.text.x..geometry.text.right())
        .map(|x| buffer[(x, geometry.text.y)].symbol())
        .collect();
    assert!(rendered.starts_with("fun main() { ... }"), "{rendered}");
    let summary = layout.rows[0].summary.as_ref().unwrap();
    let marker_x = geometry.text.x + summary.marker.start as u16;
    assert_eq!(
        geometry.hit_test(&layout, 0, marker_x, geometry.text.y),
        EditorHit::FoldToggle { line: LineIndex(0) }
    );
    assert_eq!(
        layout.cell_to_position(0, summary.cells.len() - 1),
        doc.snapshot().line_start(LineIndex(3)).unwrap()
    );
    input
        .dispatch(
            &mouse(marker_x, geometry.text.y),
            &mut doc,
            &mut focus,
            viewport,
            &mut owner,
        )
        .unwrap();
    assert!(owner.folds.collapsed(&doc.snapshot()).is_empty());
    assert_eq!(doc.snapshot().to_service_snapshot().text.as_ref(), text);
}

#[test]
fn navigation_reveals_nested_folds_and_edits_reload_and_late_results_cannot_hide_other_lines() {
    let mut doc = document("{\n {\n  work\n }\n}", "kotlin");
    let mut owner = EditorVisuals { folds: folds(&doc) };
    owner.folds.toggle(LineIndex(0));
    owner.folds.toggle(LineIndex(1));
    let mut input = EditorInput::new(FocusId(1), doc.id());
    let mut focus = FocusState {
        focused: Some(input.id),
        ..Default::default()
    };
    let geometry = EditorGeometry::new(Rect::new(0, 0, 35, 5), 5, 2);
    input
        .dispatch(
            &key(Key::Down, true),
            &mut doc,
            &mut focus,
            InputViewport {
                geometry,
                folds: &[],
            },
            &mut owner,
        )
        .unwrap();
    assert_eq!(owner.folds.collapsed(&doc.snapshot()).len(), 1);
    input
        .dispatch(
            &key(Key::Down, true),
            &mut doc,
            &mut focus,
            InputViewport {
                geometry,
                folds: &[],
            },
            &mut owner,
        )
        .unwrap();
    assert!(owner.folds.collapsed(&doc.snapshot()).is_empty());
    assert_eq!(input.state.caret.selection.anchor, ByteOffset(0));
    assert_eq!(
        doc.snapshot()
            .byte_to_position(input.state.caret.cursor())
            .unwrap()
            .line,
        LineIndex(2)
    );
    let stale = discover(&doc.snapshot(), || false).unwrap();
    owner.folds.toggle(LineIndex(0));
    doc.apply_edit(
        doc.revision(),
        kode_core::TextRange {
            start: ByteOffset(0),
            end: ByteOffset(1),
        },
        "",
        EditKind::Typing,
    )
    .unwrap();
    assert!(owner.folds.collapsed(&doc.snapshot()).is_empty());
    assert!(!owner.folds.publish(stale, &doc.snapshot()));
    let other = document("new\nfile", "kotlin");
    assert!(owner.folds.collapsed(&other.snapshot()).is_empty());
}

#[test]
fn braille_masks_raw_utf16_shapes_caret_centering_and_zero_based_hits_are_exact() {
    assert_eq!(DOTS, [[1, 8], [2, 16], [4, 32], [64, 128]]);
    for (text, expected) in [
        ("xX\nxX\nxX\nxX", 0xff),
        ("x \n x\nx \n x", 0x95),
        ("😀", 9),
        ("\t", 1),
    ] {
        let doc = document(text, "text");
        let preview = Minimap::prepare(&doc.snapshot(), 1, 1, ByteOffset(0)).unwrap();
        assert_eq!(
            preview.rows[0].cells,
            char::from_u32(0x2800 + expected).unwrap().to_string()
        );
    }
    let doc = document(
        &(0..100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "text",
    );
    let caret = doc.snapshot().line_start(LineIndex(52)).unwrap();
    let preview = Minimap::prepare(&doc.snapshot(), 20, 4, caret).unwrap();
    assert_eq!(preview.block_offset, 11);
    assert_eq!(preview.rows[0].first_line, LineIndex(44));
    assert!(preview.rows[2].highlighted);
    let layout = VisualLayout::build(&doc.snapshot(), 20, &[]).unwrap();
    let area = Rect::new(60, 3, 20, 4);
    let (byte, row) = preview.hit(area, 60, 3, &layout).unwrap().unwrap();
    assert_eq!(byte, doc.snapshot().line_start(LineIndex(44)).unwrap());
    assert_eq!(row, 44);
    assert!(preview.hit(area, 59, 3, &layout).unwrap().is_none());
    let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 12));
    preview.render(area, &mut buffer, &Theme::compiled());
    assert_eq!(
        buffer[(60, 5)].bg,
        Theme::compiled().style("code-selection").bg.unwrap()
    );
}

#[test]
fn minimap_clicks_expand_targets_and_bounds_remain_valid_with_search_wrap_resize() {
    let mut doc = document(
        &format!(
            "{{\n{}\n}}",
            (0..36)
                .map(|_| "long wrapped source line".repeat(5))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        "kotlin",
    );
    let mut owner = EditorVisuals { folds: folds(&doc) };
    owner.folds.toggle(LineIndex(0));
    let mut input = EditorInput::new(FocusId(1), doc.id());
    let mut focus = FocusState::default();
    for (width, height, search) in [(80, 12, 0), (60, 8, 2), (40, 6, 2)] {
        let geometry = EditorGeometry::new(
            Rect::new(9, 7, width, height),
            doc.snapshot().len_lines(),
            search,
        );
        let y = geometry.minimap.y + 1;
        input
            .state
            .caret
            .move_to(&doc.snapshot(), ByteOffset(0), false)
            .unwrap();
        input
            .dispatch(
                &mouse(geometry.minimap.x, y),
                &mut doc,
                &mut focus,
                InputViewport {
                    geometry,
                    folds: &[],
                },
                &mut owner,
            )
            .unwrap();
        assert_eq!(
            doc.snapshot()
                .byte_to_position(input.state.caret.cursor())
                .unwrap()
                .line,
            LineIndex(4)
        );
        assert_eq!(focus.focused, Some(input.id));
        assert!(owner.folds.collapsed(&doc.snapshot()).is_empty());
    }
    for (width, height) in [(0, 0), (1, 1), (20, 6), (39, 12)] {
        let geometry = EditorGeometry::new(
            Rect::new(0, 0, width, height),
            doc.snapshot().len_lines(),
            2,
        );
        assert_eq!(geometry.minimap.width, 0);
        let preview = Minimap::prepare(
            &doc.snapshot(),
            0,
            geometry.minimap.height as usize,
            ByteOffset(0),
        )
        .unwrap();
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        preview.render(geometry.minimap, &mut buffer, &Theme::compiled());
    }
}

#[tokio::test]
async fn real_fold_discovery_worker_is_cancellable_and_publishes_current_revision() {
    let doc = document("{\n body\n}", "kotlin");
    let tag = JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: Some((doc.id(), doc.revision())),
    };
    let mut scheduler: Scheduler<FoldReport> =
        Scheduler::new(Notifications::new(Arc::new(MonotonicClock::default())));
    schedule_discovery(&mut scheduler, OwnerId(1), tag, doc.snapshot()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(2), scheduler.next())
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    let mut state = FoldState::default();
    assert!(state.publish(report, &doc.snapshot()));
    assert_eq!(state.regions().count(), 1);
    assert!(scheduler.shutdown().await.is_empty());
}

#[test]
fn scrolled_gutter_toggle_and_tiny_fold_summaries_use_visible_rows() {
    let mut doc = document(
        "first\nsecond\nthird\n{\n body\n}\nlast\nmore\ntail",
        "kotlin",
    );
    let mut owner = EditorVisuals { folds: folds(&doc) };
    let mut input = EditorInput::new(FocusId(1), doc.id());
    input.state.scroll_rows = 3;
    let mut focus = FocusState {
        focused: Some(input.id),
        ..Default::default()
    };
    let geometry = EditorGeometry::new(Rect::new(0, 0, 35, 3), doc.snapshot().len_lines(), 0);
    input
        .dispatch(
            &mouse(geometry.gutter.x, geometry.gutter.y),
            &mut doc,
            &mut focus,
            InputViewport {
                geometry,
                folds: &[],
            },
            &mut owner,
        )
        .unwrap();
    assert_eq!(
        owner.folds.collapsed(&doc.snapshot())[0].start,
        LineIndex(3)
    );
    for width in 1..=12 {
        let collapsed = owner.folds.collapsed(&doc.snapshot());
        let layout = VisualLayout::build(&doc.snapshot(), width, &collapsed).unwrap();
        let row_index = layout.lines[3].first_row;
        let row = &layout.rows[row_index];
        let summary = row.summary.as_ref().unwrap();
        assert!(summary.cells.len() <= width);
        for cell in 0..=width {
            assert!(
                doc.snapshot()
                    .byte_to_position(layout.cell_to_position(row_index, cell))
                    .is_ok()
            );
        }
    }
}
