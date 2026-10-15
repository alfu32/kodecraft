use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Revision, Selection, TextRange,
    WorkspaceGeneration, commands::EditKind, document::Document, editor::edit::TextEdit,
};
use kode_intelligence::facade::IntelligenceResources;
use kode_platform::jobs::CancellationToken;
use kode_syntax::{
    model::{GrammarMode, GrammarStatus},
    service::tag_for,
};
use kode_tui::{
    editor::{
        layout::{CollapsedFold, VisualLayout},
        overlays::{
            OverlayCache, OverlayJob, OverlayReport, OverlayWorker, PaintSpan, compose,
            remap::remap_ranges,
        },
        render::{
            EditorGeometry, EditorHeader, OverlaySpan, RenderOptions, RenderState, render_editor,
        },
    },
    theme::Theme,
};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use std::sync::Arc;
fn range(start: usize, end: usize) -> TextRange {
    TextRange {
        start: ByteOffset(start),
        end: ByteOffset(end),
    }
}
fn doc(text: &str) -> Document {
    Document::new(
        DocumentId::fresh().unwrap(),
        DocumentOrigin::Virtual {
            owner: "overlay-test".into(),
            key: "source".into(),
            label: "synthetic".into(),
        },
        Some("kotlin".into()),
        text,
    )
}
fn paint(range: TextRange, class: &str, priority: i32) -> PaintSpan {
    PaintSpan {
        range,
        class: class.into(),
        color: None,
        priority,
    }
}
fn report(
    job: &OverlayJob,
    lexical: Vec<PaintSpan>,
    semantic: Option<Vec<PaintSpan>>,
) -> OverlayReport {
    OverlayReport {
        tag: job.tag,
        range: job.range,
        lexical,
        semantic,
        status: GrammarStatus {
            language: "kotlin".into(),
            mode: GrammarMode::TreeSitter,
            message: None,
        },
        notes: vec![],
    }
}
fn request(cache: &mut OverlayCache, doc: &Document) -> OverlayJob {
    let source = doc.snapshot();
    cache
        .request(
            &source,
            range(0, source.len_bytes()),
            range(0, source.len_bytes()),
            WorkspaceGeneration(1),
            "kotlin".into(),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn joining_identifiers_on_either_boundary_and_touching_interiors_discards_cached_paint() {
    for (source, edit, inserted, token, expected) in [
        ("foo", range(3, 3), "bar", range(0, 3), None),
        ("foo", range(0, 0), "bar", range(0, 3), None),
        ("foo", range(1, 2), "x", range(0, 3), None),
        ("foo bar", range(3, 4), "", range(4, 7), None),
        ("foo$", range(3, 4), " ", range(0, 3), None),
        ("foo", range(0, 3), "", range(0, 3), None),
        ("foo;", range(3, 3), ";", range(0, 3), Some(range(0, 3))),
        ("foo", range(0, 0), "😀 ", range(0, 3), Some(range(5, 8))),
    ] {
        let mut document = doc(source);
        let old = document.snapshot();
        let delta = document
            .apply_edit(Revision(0), edit, inserted, EditKind::Typing)
            .unwrap()
            .unwrap();
        for deltas in [Some(std::slice::from_ref(&delta)), None] {
            assert_eq!(
                remap_ranges(&old, &document.snapshot(), &[token], deltas),
                vec![expected],
                "{source:?} → {inserted:?}"
            );
        }
    }
}
#[test]
fn successive_line_edits_preserve_utf8_tab_positions_and_drop_only_replaced_tokens() {
    let mut document = doc("// naïve — 😀\r\n\ta.save(); second");
    let text = document.snapshot().text();
    let a = text.find("a.save").unwrap();
    let save = a + 2;
    let second = text.find("second").unwrap();
    let old = document.snapshot();
    let delta = document
        .apply_edit(
            document.revision(),
            range(a - 1, a),
            "// new\n\t\t",
            EditKind::Typing,
        )
        .unwrap()
        .unwrap();
    let ranges = [
        range(a, a + 1),
        range(save, save + 4),
        range(second, second + 6),
    ];
    let moved = remap_ranges(&old, &document.snapshot(), &ranges, Some(&[delta]));
    assert_eq!(
        moved,
        ranges.map(|r| Some(range(r.start.0 + 8, r.end.0 + 8)))
    );
    let old = document.snapshot();
    let start = moved[1].unwrap().start.0;
    let delta = document
        .apply_edit(
            document.revision(),
            range(start, start + 4),
            "replacement",
            EditKind::Typing,
        )
        .unwrap()
        .unwrap();
    let ranges: Vec<_> = moved.into_iter().flatten().collect();
    let moved = remap_ranges(&old, &document.snapshot(), &ranges, Some(&[delta]));
    assert!(moved[0].is_some() && moved[1].is_none());
    let old = document.snapshot();
    let second = moved[2].unwrap();
    document
        .apply_edit(
            document.revision(),
            range(0, second.start.0),
            "",
            EditKind::Typing,
        )
        .unwrap();
    assert_eq!(
        remap_ranges(&old, &document.snapshot(), &[second], None),
        vec![Some(range(0, 6))]
    );
}
#[test]
fn one_transaction_uses_core_right_to_left_deltas_without_discarding_between_edits() {
    let mut document = doc("left middle right");
    let old = document.snapshot();
    let outcome = document
        .apply_transaction(
            document.revision(),
            vec![
                TextEdit {
                    range: range(0, 4),
                    inserted: "L".into(),
                },
                TextEdit {
                    range: range(12, 17),
                    inserted: "RIGHT!!".into(),
                },
            ],
            EditKind::Typing,
            Selection {
                anchor: ByteOffset(0),
                head: ByteOffset(0),
            },
            None,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        remap_ranges(
            &old,
            &document.snapshot(),
            &[range(0, 4), range(5, 11), range(12, 17)],
            Some(&outcome.deltas)
        ),
        vec![None, Some(range(2, 8)), None]
    );
    assert_eq!(
        remap_ranges(&old, &document.snapshot(), &[range(5, 11)], None),
        vec![None],
        "fallback is deliberately conservative when deltas are unavailable"
    );
}
#[test]
fn cache_remaps_twice_rejects_stale_jobs_and_semantic_revisions_then_accepts_empty_ready_result() {
    let mut document = doc("first second");
    let mut cache = OverlayCache::default();
    cache.enable();
    let job = request(&mut cache, &document);
    cache
        .complete(
            job.tag,
            Ok(report(
                &job,
                vec![],
                Some(vec![
                    paint(range(0, 5), "codeintel.declaration", 20),
                    paint(range(6, 12), "semantic.property", 20),
                ]),
            )),
            &document.snapshot(),
        )
        .unwrap();
    cache.invalidate();
    let stale = request(&mut cache, &document);
    let delta = document
        .apply_edit(
            document.revision(),
            range(0, 5),
            "replacement",
            EditKind::Typing,
        )
        .unwrap()
        .unwrap();
    cache.changed(&document.snapshot(), Some(&[delta]));
    assert_eq!(
        cache.spans().map(|s| s.range).collect::<Vec<_>>(),
        vec![range(12, 18)]
    );
    assert!(
        !cache
            .complete(
                stale.tag,
                Ok(report(&stale, vec![], Some(vec![]))),
                &document.snapshot()
            )
            .unwrap()
    );
    document
        .apply_edit(document.revision(), range(0, 12), "", EditKind::Typing)
        .unwrap();
    cache.changed(&document.snapshot(), None);
    assert_eq!(cache.spans().next().unwrap().range, range(0, 6));
    let stale = request(&mut cache, &document);
    cache.invalidate();
    let current = request(&mut cache, &document);
    assert!(
        !cache
            .complete(
                stale.tag,
                Ok(report(&stale, vec![], Some(vec![]))),
                &document.snapshot()
            )
            .unwrap()
    );
    assert!(
        !cache
            .complete(
                current.tag,
                Err(DomainError::Cancelled),
                &document.snapshot()
            )
            .unwrap()
    );
    let current = request(&mut cache, &document);
    cache
        .complete(
            current.tag,
            Ok(report(&current, vec![], None)),
            &document.snapshot(),
        )
        .unwrap();
    assert_eq!(
        cache.spans().count(),
        1,
        "provider failure keeps safely remapped paint"
    );
    cache.invalidate();
    let current = request(&mut cache, &document);
    cache
        .complete(
            current.tag,
            Ok(report(&current, vec![], Some(vec![]))),
            &document.snapshot(),
        )
        .unwrap();
    assert_eq!(
        cache.spans().count(),
        0,
        "ready empty result clears old paint"
    );
}
#[test]
fn composition_is_disjoint_nested_comments_win_and_semantic_kind_keeps_declaration_decoration() {
    let theme = Theme::compiled();
    let spans = vec![
        paint(range(0, 15), "semantic.comment", 14),
        paint(range(3, 6), "semantic.keyword", 10),
        paint(range(16, 20), "codeintel.declaration", 20),
        paint(range(16, 20), "semantic.class", 20),
    ];
    let segments = compose(&spans, &theme);
    assert_eq!(segments.len(), 2);
    assert!(
        segments
            .windows(2)
            .all(|p| p[0].range.end <= p[1].range.start)
    );
    assert_eq!(segments[0].style.fg, theme.style("semantic.comment").fg);
    assert_eq!(segments[1].style.fg, theme.style("semantic.class").fg);
    assert!(segments[1].style.add_modifier.contains(Modifier::BOLD));
}
#[test]
fn actual_parser_reuses_state_cancels_and_reports_explicit_missing_grammar_without_ui_calls() {
    let worker = OverlayWorker::new(Arc::new(IntelligenceResources::compiled().unwrap()));
    let mut document = doc("/* first\r\nsecond 😀 */\nclass Model { val id: Long = 1 }");
    let snapshot = document.snapshot();
    let mut job = OverlayJob {
        tag: tag_for(&snapshot, WorkspaceGeneration(1)).unwrap(),
        range: range(0, snapshot.len_bytes()),
        snapshot,
        language: "kotlin".into(),
    };
    let first = worker
        .prepare(&job, None, &CancellationToken::new())
        .unwrap();
    assert_eq!(first.status.mode, GrammarMode::TreeSitter);
    for span in &first.lexical {
        assert!(
            !job.snapshot
                .slice(span.range)
                .unwrap()
                .to_string()
                .is_empty()
        );
    }
    assert!(first.lexical.iter().any(|span| {
        span.class.contains("comment")
            && job
                .snapshot
                .slice(span.range)
                .unwrap()
                .to_string()
                .contains("second 😀")
    }));
    let start = document.snapshot().text().rfind('1').unwrap();
    document
        .apply_edit(
            document.revision(),
            range(start, start + 1),
            "42",
            EditKind::Typing,
        )
        .unwrap();
    job.snapshot = document.snapshot();
    job.tag = tag_for(&job.snapshot, WorkspaceGeneration(1)).unwrap();
    job.range = range(0, job.snapshot.len_bytes());
    let next = worker
        .prepare(&job, None, &CancellationToken::new())
        .unwrap();
    assert!(next.lexical.iter().any(
        |span| span.class.contains("number") && job.snapshot.slice(span.range).unwrap() == "42"
    ));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        worker.prepare(&job, None, &cancel),
        Err(DomainError::Cancelled)
    ));
    job.language = "no-grammar".into();
    let fallback = worker
        .prepare(&job, None, &CancellationToken::new())
        .unwrap();
    assert_eq!(fallback.status.mode, GrammarMode::Plain);
    assert!(fallback.status.message.unwrap().contains("unavailable"));
}

#[test]
fn renderer_preserves_punctuation_and_unicode_with_warning_hover_search_transient_tab_selection_cursor_order()
 {
    let document = doc("a\tb;😀()\nlast");
    let snapshot = document.snapshot();
    let geometry = EditorGeometry::new(Rect::new(0, 0, 60, 5), snapshot.len_lines(), 0);
    let layout = VisualLayout::build(&snapshot, geometry.text.width as usize, &[]).unwrap();
    let theme = Theme::compiled();
    let mut overlays = compose(&[paint(range(0, 11), "codeintel.declaration", 20)], &theme);
    overlays.extend([
        OverlaySpan {
            range: range(0, 1),
            priority: 26,
            style: Style::default().add_modifier(Modifier::UNDERLINED),
        },
        OverlaySpan {
            range: range(1, 2),
            priority: 30,
            style: Style::default().bg(Color::Green),
        },
        OverlaySpan {
            range: range(2, 3),
            priority: 31,
            style: Style::default().bg(Color::Blue),
        },
        OverlaySpan {
            range: range(3, 8),
            priority: 40,
            style: Style::default().bg(Color::Magenta),
        },
    ]);
    let state = RenderState {
        selection: Selection {
            anchor: ByteOffset(8),
            head: ByteOffset(9),
        },
        scroll_rows: 0,
        focused: true,
    };
    let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 5));
    render_editor(
        &layout,
        &mut buffer,
        &theme,
        RenderOptions {
            geometry,
            header: &EditorHeader::for_document(&document, None, "test".into()),
            state: &state,
            overlays: &overlays,
            warning_lines: &[kode_core::LineIndex(0)],
            fold_starts: &[],
        },
    );
    let at = |byte| {
        let (row, col) = layout.position_to_cell(ByteOffset(byte)).unwrap();
        &buffer[(geometry.text.x + col as u16, geometry.text.y + row as u16)]
    };
    assert_eq!(at(0).bg, theme.style("code-search-active").bg.unwrap());
    assert!(at(0).modifier.contains(Modifier::UNDERLINED));
    assert_eq!(at(1).fg, theme.style("code-tab").fg.unwrap());
    assert_eq!(at(2).bg, Color::Blue);
    assert_eq!(at(3).bg, Color::Magenta);
    assert_eq!(at(8).bg, theme.style("code-selection").bg.unwrap());
    assert_eq!(at(9).bg, theme.style("code-cursor").bg.unwrap());
    let drawn: String = (geometry.text.x..geometry.text.right())
        .map(|x| buffer[(x, geometry.text.y)].symbol())
        .collect();
    assert!(drawn.starts_with("a|-->b;\\x1F600()"), "{drawn:?}");
    // Fold summaries consume the same prepared overlays and tab priority.
    let folded_doc = doc("a\t {\n hidden\n} ;");
    let folded = VisualLayout::build(
        &folded_doc.snapshot(),
        55,
        &[CollapsedFold {
            start: kode_core::LineIndex(0),
            end: kode_core::LineIndex(2),
        }],
    )
    .unwrap();
    render_editor(
        &folded,
        &mut buffer,
        &theme,
        RenderOptions {
            geometry,
            header: &EditorHeader::for_document(&folded_doc, None, String::new()),
            state: &RenderState {
                focused: false,
                ..state
            },
            overlays: &overlays,
            warning_lines: &[],
            fold_starts: &[],
        },
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "|" && cell.fg == theme.style("code-tab").fg.unwrap())
    );
}
