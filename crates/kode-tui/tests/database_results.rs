use kode_core::RequestId;
use kode_database::{
    bridge::wire::{CancelStatus, Cell, Column, Outcome, QueryFinal, SqlError},
    execution::{Phase, Preview, ResultSet, Snapshot},
};
use kode_tui::{
    focus::{FocusId, FocusState},
    input::terminal::*,
    panels::database::{results::ResultsPanel, session_dialog::default_filename},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::sync::Arc;
fn key(key: Key, alt: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            alt,
            ..Default::default()
        },
        repeat: false,
    })
}
fn error(message: &str) -> SqlError {
    SqlError {
        message: message.into(),
        sql_state: Some("42000".into()),
        vendor_code: 7,
        chained: vec![],
    }
}
fn fixture() -> Snapshot {
    let columns: Arc<[Column]> = Arc::from([
        Column {
            label: "N".into(),
            jdbc_type: 0,
            type_name: "NULL".into(),
            nullable: true,
        },
        Column {
            label: "B".into(),
            jdbc_type: -2,
            type_name: "BINARY".into(),
            nullable: false,
        },
        Column {
            label: "VALUE\x1b".into(),
            jdbc_type: 12,
            type_name: "VARCHAR".into(),
            nullable: false,
        },
    ]);
    let rows: Vec<_> = (0..1200)
        .map(|row| {
            vec![
                Cell::Null,
                Cell::Binary {
                    base64: "AP8=".into(),
                    truncated: false,
                },
                Cell::Text {
                    text: format!("row{row}"),
                    truncated: false,
                },
            ]
        })
        .collect();
    let set = ResultSet {
        id: "one".into(),
        columns,
        chunks: rows
            .chunks(200)
            .map(|chunk| Arc::from(chunk.to_vec()))
            .collect(),
        received_rows: 1200,
        limited: true,
        incomplete: false,
    };
    let second = ResultSet {
        id: "two".into(),
        ..set.clone()
    };
    let mut preview = Preview::default();
    preview.result_sets = vec![set, second];
    preview.update_counts = vec![2, i64::MAX];
    preview.warnings = vec![error("warning detail")];
    let mut failure = error("failure detail");
    failure.chained.push(error("nested detail"));
    Snapshot {
        phase: Phase::Finished,
        preview: Arc::new(preview),
        completion: Some(QueryFinal {
            execution_id: "query".into(),
            outcome: Outcome::Failed,
            elapsed_ms: 123,
            transaction_usable: false,
            event_count: Some(1),
            preview_incomplete: false,
            error: Some(failure),
        }),
        ..Default::default()
    }
}
fn render(panel: &mut ResultsPanel, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    panel.render(area, &mut buffer, &Theme::compiled(), false);
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}
#[test]
fn typed_cells_chunks_result_tabs_messages_and_last_row_are_accessible_without_editor_focus_theft()
{
    let mut panel = ResultsPanel::new().unwrap();
    panel.publish(fixture());
    let screen = render(&mut panel, 100, 16);
    assert!(screen.contains("<null>"));
    assert!(screen.contains("0x00FF (2 bytes)"));
    assert!(screen.contains("VARCHAR"));
    assert!(!screen.contains('\x1b'));
    assert!(screen.contains("limited"));
    let editor = FocusId(RequestId::fresh().unwrap().0);
    let mut focus = FocusState {
        focused: Some(editor),
        ..Default::default()
    };
    assert!(!panel.dispatch(&key(Key::Down, false), &mut focus));
    assert_eq!(panel.row, 0);
    assert!(panel.dispatch(&key(Key::PageDown, true), &mut focus));
    assert!(panel.row > 0);
    assert_eq!(focus.focused, Some(editor));
    panel.dispatch(&key(Key::End, true), &mut focus);
    assert!(render(&mut panel, 100, 16).contains("row1199"));
    panel.dispatch(&key(Key::Tab, true), &mut focus);
    assert_eq!(panel.tab, 1);
    assert_eq!(panel.row, 0);
    panel.dispatch(&key(Key::Tab, true), &mut focus);
    assert_eq!(panel.tab, 2);
    let messages = render(&mut panel, 100, 16);
    for value in [
        "Update count: 2",
        "9223372036854775807",
        "warning detail",
        "failure detail",
        "nested detail",
        "SQLSTATE 42000 / 7",
        "123 ms",
    ] {
        assert!(messages.contains(value), "{value}: {messages}");
    }
    let point = panel.area;
    panel.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: point.x,
            y: point.y,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers::default(),
        }),
        &mut focus,
    );
    assert_eq!(focus.focused, Some(panel.id));
    assert!(panel.dispatch(&key(Key::Up, false), &mut focus));
    for size in [(0, 0), (1, 1), (3, 4), (20, 6)] {
        render(&mut panel, size.0, size.1);
    }
}
#[test]
fn cancel_acknowledgement_and_unknown_outcome_remain_separate_from_final_completion() {
    let mut panel = ResultsPanel::new().unwrap();
    panel.publish(Snapshot {
        phase: Phase::Cancelling,
        cancel_status: Some(CancelStatus::Unsupported),
        unknown_outcome: true,
        ..Default::default()
    });
    let screen = render(&mut panel, 120, 12);
    assert!(screen.contains("Cancelling"));
    assert!(screen.contains("OUTCOME UNKNOWN"));
    assert!(screen.contains("Cancel request: Unsupported"));
    assert!(screen.contains("completion is reported separately"));
    assert!(screen.contains("no SQL was replayed"));
    panel.reset();
    assert_eq!(panel.tab, 0);
    assert_eq!(panel.row, 0);
    assert_eq!(default_filename("sql@My SQL:2"), "My_SQL_2.sql");
    assert_eq!(default_filename("query.SQL"), "query.SQL");
}
