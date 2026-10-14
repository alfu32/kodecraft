use crossterm::event::MouseButton;
use kode_tui::{
    focus::{EventTarget, FocusId, FocusState, ModalStack, Route},
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    layout::{Axis, Splitter, local_position, split},
    theme::Theme,
    widgets::{
        controls::{Scrollbar, Slider},
        dialog::{DialogAction, TextDialog},
        field::{FieldAction, TextField},
        tabs::Tabs,
    },
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier},
    widgets::{List, ListState, Row, Table, TableState},
};
use std::{cell::RefCell, rc::Rc};

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
            ..Modifiers::default()
        },
        repeat: false,
    })
}
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Modifiers::default(),
    })
}

#[test]
fn theme_merges_exact_rule_defaults_decorations_and_extension_values() {
    let mut theme = Theme::parse(
        "/* comment */ default { fg: #112233; bg: #334455; text-decoration: bold; } field { bg: #abcdef; text-decoration: italic; future: keep; } root > child { left: 3; }",
    );
    let field = theme.properties("field");
    assert_eq!(field.style.fg, Some(Color::Rgb(17, 34, 51)));
    assert_eq!(field.style.bg, Some(Color::Rgb(171, 205, 239)));
    assert!(!field.style.add_modifier.contains(Modifier::BOLD));
    assert!(field.style.add_modifier.contains(Modifier::ITALIC));
    assert_eq!(field.extended["future"], "keep");
    assert_eq!(theme.properties("root.child").left, Some(3));
    theme.merge(&Theme::parse("field { fg: #ffffff; }"));
    assert_eq!(theme.style("field").bg, Some(Color::Rgb(171, 205, 239)));
    assert_eq!(theme.style("field").fg, Some(Color::Rgb(255, 255, 255)));
    assert!(
        Theme::compiled()
            .rules
            .contains_key("tab-button::selection:focus")
    );
}

#[test]
fn unicode_field_selection_paste_limits_and_readonly_use_scalar_boundaries() {
    let mut field = TextField::new("a😀b".into());
    field.handle(&key(Key::Left));
    assert_eq!(field.cursor, 5);
    field.handle(&key(Key::Left));
    assert_eq!(field.cursor, 1);
    field.handle(&modified(Key::Right, false, true));
    assert_eq!(field.cursor, 5);
    assert_eq!(
        field.handle(&modified(Key::Character('c'), true, false)),
        Some(FieldAction::Copy("😀".into()))
    );
    assert_eq!(
        field.handle(&InputEvent::Paste("X\nY".into())),
        Some(FieldAction::Changed)
    );
    assert_eq!(field.value, "aXYb");
    field.max_chars = 4;
    assert_eq!(
        field.handle(&key(Key::Character('z'))),
        Some(FieldAction::LimitReached)
    );
    assert_eq!(field.value, "aXYb");
    field.read_only = true;
    let before = field.clone();
    field.handle(&key(Key::Backspace));
    field.handle(&InputEvent::Paste("oops".into()));
    assert_eq!(field.value, before.value);
    assert_eq!(field.anchor, before.anchor);
}

#[test]
fn field_window_caret_and_selected_controls_have_expected_cells() {
    let theme = Theme::compiled();
    let area = Rect::new(0, 0, 12, 4);
    let mut buffer = Buffer::empty(area);
    let field = TextField::new("0123456789".into());
    let caret = field
        .render(Rect::new(2, 1, 8, 1), &mut buffer, &theme, true)
        .unwrap();
    assert_eq!(buffer[(2, 1)].symbol(), "[");
    assert_eq!(buffer[(3, 1)].symbol(), "5");
    assert_eq!(buffer[(9, 1)].symbol(), "]");
    assert_eq!(caret.x, 8);
    assert!(buffer[caret].modifier.contains(Modifier::REVERSED));
    assert_eq!(
        buffer[(3, 1)].fg,
        theme.style("code-search-field-active").fg.unwrap()
    );
    let tiny = Rect::new(0, 0, 1, 1);
    let mut buffer = Buffer::empty(tiny);
    let mut tabs = Tabs::new(vec!["Files".into(), "Git".into()], Axis::Horizontal);
    tabs.selected = 1;
    tabs.render(tiny, &mut buffer, &theme, true);
    assert_eq!(buffer[(0, 0)].symbol(), "G");
    let mut dialog = TextDialog::new("Name".into(), "value".into());
    dialog.focus = 1;
    dialog.render(tiny, &mut buffer, &theme);
    assert_eq!(buffer[(0, 0)].symbol(), "O");
    dialog.focus = 2;
    dialog.render(tiny, &mut buffer, &theme);
    assert_eq!(buffer[(0, 0)].symbol(), "C");
}

#[test]
fn modal_and_fields_clear_their_surface_when_drawn_over_existing_content() {
    let theme = Theme::compiled();
    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    for cell in &mut buffer.content {
        cell.set_symbol("Z");
    }
    let mut dialog = TextDialog::new("Name".into(), "a".into());
    dialog.render(area, &mut buffer, &theme);
    let layout = dialog.layout(area);
    assert_eq!(
        buffer[(layout.modal.x + 1, layout.modal.y + 4)].symbol(),
        " "
    );
    assert_eq!(buffer[(layout.field.x + 2, layout.field.y)].symbol(), " ");
    assert_eq!(buffer[(0, 0)].symbol(), "Z");
}

#[test]
fn dialog_fields_buttons_and_focus_have_distinct_bounds_at_all_sizes() {
    let theme = Theme::compiled();
    for (width, height) in [(80, 24), (120, 40), (20, 6), (1, 1)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let mut dialog = TextDialog::new("Name".into(), "initial".into());
        assert!(dialog.render(area, &mut buffer, &theme).is_some());
        dialog.handle(&key(Key::Character('a')));
        assert_eq!(dialog.field.value, "initiala");
        dialog.handle(&key(Key::Tab));
        assert_eq!(dialog.focus, 1);
        dialog.render(area, &mut buffer, &theme);
        dialog.handle(&key(Key::Character('z')));
        assert_eq!(dialog.field.value, "initiala");
        dialog.handle(&key(Key::Tab));
        assert_eq!(dialog.focus, 2);
        dialog.render(area, &mut buffer, &theme);
        dialog.handle(&key(Key::Enter));
        assert_eq!(dialog.action, Some(DialogAction::Dismissed));
    }
    let area = Rect::new(0, 0, 80, 24);
    let mut dialog = TextDialog::new("Name".into(), "value".into());
    dialog.render(area, &mut Buffer::empty(area), &theme);
    let layout = dialog.layout(area);
    assert_eq!(layout.ok.right(), layout.cancel.x);
    dialog.handle(&pointer(
        layout.cancel.x,
        layout.cancel.y,
        PointerAction::Down(MouseButton::Left),
    ));
    assert_eq!(dialog.action, Some(DialogAction::Dismissed));
}

struct Target {
    name: &'static str,
    consume: bool,
    calls: Rc<RefCell<Vec<&'static str>>>,
}
impl EventTarget for Target {
    fn dispatch(&mut self, _: &InputEvent) -> bool {
        self.calls.borrow_mut().push(self.name);
        self.consume
    }
}

#[test]
fn modal_popup_child_panel_global_priority_consumes_input_once_and_restores_focus() {
    let calls = Rc::new(RefCell::new(vec![]));
    let mut modal = Target {
        name: "modal",
        consume: false,
        calls: calls.clone(),
    };
    let mut child = Target {
        name: "child",
        consume: true,
        calls: calls.clone(),
    };
    let mut global = Target {
        name: "global",
        consume: true,
        calls: calls.clone(),
    };
    Route {
        modal: Some(&mut modal),
        capture_or_popup: None,
        child: Some(&mut child),
        panel: None,
        global: Some(&mut global),
    }
    .dispatch(&key(Key::Character('x')));
    assert_eq!(*calls.borrow(), vec!["modal"]);
    calls.borrow_mut().clear();
    let mut popup = Target {
        name: "definition",
        consume: false,
        calls: calls.clone(),
    };
    Route {
        modal: None,
        capture_or_popup: Some(&mut popup),
        child: Some(&mut child),
        panel: None,
        global: Some(&mut global),
    }
    .dispatch(&key(Key::Character('x')));
    assert_eq!(*calls.borrow(), vec!["definition", "child"]);
    calls.borrow_mut().clear();
    Route {
        modal: None,
        capture_or_popup: None,
        child: Some(&mut child),
        panel: None,
        global: Some(&mut global),
    }
    .dispatch(&modified(Key::Enter, true, false));
    assert_eq!(*calls.borrow(), vec!["child"]);
    let mut focus = FocusState {
        focused: Some(FocusId(7)),
        hovered: Some(FocusId(8)),
        captured: Some(FocusId(9)),
    };
    let mut stack = ModalStack::default();
    stack.push(FocusId(1), "first", &mut focus);
    stack.push(FocusId(2), "second", &mut focus);
    assert_eq!(focus.focused, Some(FocusId(2)));
    assert_eq!(focus.hovered, Some(FocusId(8)));
    assert_eq!(focus.captured, None);
    stack.pop(&mut focus);
    assert_eq!(focus.focused, Some(FocusId(1)));
    stack.pop(&mut focus);
    assert_eq!(focus.focused, Some(FocusId(7)));
}

#[test]
fn scrollbar_track_click_drag_release_and_slider_neighbor_bounds() {
    let area = Rect::new(10, 2, 1, 10);
    let mut scroll = Scrollbar::new(100, 10);
    scroll.handle(
        &pointer(10, 10, PointerAction::Down(MouseButton::Left)),
        area,
    );
    assert_eq!(scroll.offset, 80);
    assert!(scroll.captured());
    scroll.handle(
        &pointer(99, 4, PointerAction::Drag(MouseButton::Left)),
        area,
    );
    assert_eq!(scroll.offset, 20);
    scroll.handle(&pointer(99, 99, PointerAction::Up(MouseButton::Left)), area);
    assert!(!scroll.captured());
    let area = Rect::new(10, 2, 5, 1);
    let mut slider = Slider::new(0.0, 100.0, 0.0).unwrap();
    assert!(!slider.handle(
        &pointer(15, 2, PointerAction::Down(MouseButton::Left)),
        area,
        false
    ));
    slider.handle(
        &pointer(12, 2, PointerAction::Down(MouseButton::Left)),
        area,
        false,
    );
    assert_eq!(slider.value, 50.0);
    slider.handle(
        &pointer(0, 99, PointerAction::Drag(MouseButton::Left)),
        area,
        false,
    );
    assert_eq!(slider.value, 0.0);
    slider.handle(
        &pointer(99, 99, PointerAction::Up(MouseButton::Left)),
        area,
        false,
    );
    assert!(!slider.captured());
    assert!(Slider::new(0.0, f64::NAN, 0.0).is_err());
}

#[test]
fn panels_tabs_fields_and_standard_widgets_clip_at_tiny_sizes() {
    let theme = Theme::compiled();
    for (width, height) in [(80, 24), (20, 6), (2, 5), (1, 1), (0, 0)] {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let panes = split(area, axis, 30);
            assert_eq!(panes.first.intersection(area), panes.first);
            let mut tabs = Tabs::new(vec!["Files".into(), "Project".into(), "Git".into()], axis);
            tabs.selected = 2;
            tabs.render(area, &mut buffer, &theme, true);
            assert_eq!(tabs.selected, 2);
            assert!(!tabs.handle(&key(Key::Left), false));
        }
        TextField::new("long 😀 field value".into()).render(area, &mut buffer, &theme, true);
        Slider::new(0.0, 1.0, 0.5)
            .unwrap()
            .render(area, &mut buffer, &theme);
        Scrollbar::new(100, 10).render(area, &mut buffer, &theme);
        kode_tui::widgets::standard::list(
            List::new(["one", "two"]),
            &mut ListState::default().with_selected(Some(1)),
            area,
            &mut buffer,
            &theme,
        );
        kode_tui::widgets::standard::table(
            Table::new(
                [Row::new(["a", "b"])],
                [Constraint::Percentage(50), Constraint::Percentage(50)],
            ),
            &mut TableState::default(),
            area,
            &mut buffer,
            &theme,
        );
    }
    let area = Rect::new(10, 20, 30, 10);
    assert_eq!(local_position(area, 12, 23).unwrap().x, 2);
    assert!(local_position(area, 40, 20).is_none());
    let mut splitter = Splitter::new(Axis::Horizontal, 10);
    splitter.handle(
        &pointer(20, 23, PointerAction::Down(MouseButton::Left)),
        area,
    );
    assert!(splitter.captured());
    splitter.handle(
        &pointer(200, 99, PointerAction::Drag(MouseButton::Left)),
        area,
    );
    assert_eq!(splitter.size, 28);
    splitter.handle(
        &pointer(200, 99, PointerAction::Up(MouseButton::Left)),
        area,
    );
    assert!(!splitter.captured());
}
