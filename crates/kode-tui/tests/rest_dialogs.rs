use crossterm::event::MouseButton;
use kode_tui::{
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    panels::rest::sidebar::{
        choice_dialog::ChoiceDialog,
        entry_dialog::{EntryDialog, EntryValue},
    },
    theme::Theme,
    widgets::field::FieldAction,
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn modified(key: Key, ctrl: bool, shift: bool) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers {
            ctrl,
            shift,
            ..Default::default()
        },
        repeat: false,
    })
}
fn click(rect: Rect) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x: rect.x,
        y: rect.y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
fn value() -> EntryValue {
    EntryValue {
        key: " X-Test ".into(),
        value: "évalue".into(),
        description: "kept".into(),
        kind: "number".into(),
        enabled: false,
    }
}
#[test]
fn rest_entry_dialog_fields_buttons_unicode_and_clipboard_keep_distinct_ownership() {
    let mut dialog = EntryDialog::new("Header".into(), value(), false, "Description").unwrap();
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    let theme = Theme::compiled();
    let cursor = dialog.render(area, &mut buffer, &theme).unwrap();
    let geometry = dialog.layout(area);
    assert!(geometry.fields[0].contains(cursor));
    dialog.dispatch(&key(Key::Tab));
    dialog.dispatch(&key(Key::Home));
    dialog.dispatch(&key(Key::Right));
    assert_eq!(dialog.fields[1].cursor, 2);
    dialog.dispatch(&modified(Key::Character('a'), true, false));
    dialog.dispatch(&modified(Key::Character('c'), true, false));
    assert!(
        dialog
            .take_actions()
            .contains(&FieldAction::Copy("évalue".into()))
    );
    let proof = dialog.proof().unwrap();
    dialog.paste(proof, "pasted".into());
    assert_eq!(dialog.fields[1].value, "pasted");
    dialog.focus = dialog.fields.len();
    dialog.dispatch(&key(Key::Character('z')));
    assert_eq!(dialog.fields[1].value, "pasted");
    dialog.dispatch(&key(Key::Right));
    assert_eq!(dialog.focus, dialog.fields.len() + 1);
    dialog.dispatch(&key(Key::Enter));
    assert!(dialog.finished);
    assert!(dialog.result.is_none());
    let mut dialog = EntryDialog::new("Header".into(), value(), false, "Description").unwrap();
    dialog.render(area, &mut buffer, &theme);
    dialog.dispatch(&click(dialog.layout(area).ok));
    let result = dialog.result.unwrap();
    assert_eq!(result.key, "X-Test");
    assert_eq!(result.description, "kept");
}
#[test]
fn variable_metadata_enabled_type_and_all_dialog_buttons_are_visible_and_clickable() {
    for apply in [false, true] {
        let mut dialog = EntryDialog::new("Variable".into(), value(), true, "Description").unwrap();
        let area = Rect::new(0, 0, 120, 40);
        let mut buffer = Buffer::empty(area);
        dialog.render(area, &mut buffer, &Theme::compiled());
        let geometry = dialog.layout(area);
        assert!(geometry.ok.intersection(geometry.cancel).is_empty());
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("OK"));
        assert!(text.contains("Cancel"));
        dialog.dispatch(&click(if apply { geometry.ok } else { geometry.cancel }));
        assert!(dialog.finished);
        if apply {
            let result = dialog.result.unwrap();
            assert_eq!(result.kind, "number");
            assert!(!result.enabled);
            assert_eq!(result.description, "kept");
        } else {
            assert!(dialog.result.is_none());
        }
    }
}
#[test]
fn paste_proofs_reject_changed_fields_focus_and_replaced_dialogs_and_limits_are_reported() {
    let mut dialog = EntryDialog::new("Entry".into(), value(), true, "Description").unwrap();
    let proof = dialog.proof().unwrap();
    dialog.dispatch(&key(Key::Tab));
    dialog.paste(proof, "wrong".into());
    assert_eq!(dialog.fields[1].value, "évalue");
    let proof = dialog.proof().unwrap();
    dialog.dispatch(&key(Key::Character('x')));
    let expected = dialog.fields[1].value.clone();
    dialog.paste(proof, "wrong".into());
    assert_eq!(dialog.fields[1].value, expected);
    let proof = dialog.proof().unwrap();
    let mut other = EntryDialog::new("Other".into(), value(), true, "Description").unwrap();
    other.paste(proof, "wrong".into());
    assert_eq!(other.fields[0].value, " X-Test ");
    other.fields[0].max_chars = 8;
    other.dispatch(&InputEvent::Paste("oversized-content".into()));
    assert!(other.take_actions().contains(&FieldAction::LimitReached));
}
#[test]
fn choice_dialog_scrolls_to_every_action_and_folder_before_confirming_or_cancelling() {
    for immediate in [false, true] {
        let mut dialog = ChoiceDialog::new(
            "Move".into(),
            (0..100)
                .map(|index| (format!("Folder {index}"), index))
                .collect(),
            "Move",
            immediate,
        );
        let area = Rect::new(0, 0, 20, 6);
        let mut buffer = Buffer::empty(area);
        let theme = Theme::compiled();
        dialog.render(area, &mut buffer, &theme);
        dialog.dispatch(&key(Key::End));
        dialog.render(area, &mut buffer, &theme);
        assert!(dialog.rows.iter().any(|(_, index)| *index == 99));
        let target = dialog.rows.last().unwrap().0;
        dialog.dispatch(&click(target));
        assert_eq!(dialog.finished, immediate);
        if !immediate {
            dialog.dispatch(&key(Key::Tab));
            dialog.dispatch(&key(Key::Enter));
        }
        assert_eq!(dialog.result, Some(99));
    }
    let mut dialog =
        ChoiceDialog::new("Actions".into(), vec![("Delete".into(), 7)], "Choose", true);
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    dialog.render(area, &mut buffer, &Theme::compiled());
    dialog.dispatch(&click(dialog.cancel));
    assert!(dialog.finished);
    assert_eq!(dialog.result, None);
}
#[test]
fn narrow_and_single_cell_dialogs_preserve_accessible_targets_and_do_not_panic() {
    for (width, height) in [(80, 24), (120, 40), (20, 6), (1, 1), (0, 0)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let theme = Theme::compiled();
        let mut dialog = EntryDialog::new("Variable".into(), value(), true, "Description").unwrap();
        for focus in 0..7 {
            dialog.focus = focus;
            let cursor = dialog.render(area, &mut buffer, &theme);
            if let Some(cursor) = cursor {
                assert!(area.contains(cursor));
            }
            let geometry = dialog.layout(area);
            for target in geometry
                .fields
                .iter()
                .chain([&geometry.ok, &geometry.cancel])
            {
                if !target.is_empty() {
                    assert!(area.contains(Position::new(target.x, target.y)));
                }
            }
        }
        let mut choice = ChoiceDialog::new(
            "Pick".into(),
            (0..30).map(|i| (i.to_string(), i)).collect(),
            "Move",
            false,
        );
        for focus in 0..3 {
            choice.focus = focus;
            choice.dispatch(&key(Key::End));
            choice.render(area, &mut buffer, &theme);
            if !choice.ok.is_empty() {
                assert!(area.contains(Position::new(choice.ok.x, choice.ok.y)));
            }
            if !choice.cancel.is_empty() {
                assert!(area.contains(Position::new(choice.cancel.x, choice.cancel.y)));
            }
        }
    }
}
