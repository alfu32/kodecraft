use crossterm::event::MouseButton;
use kode_rest::model::{
    NodePath, Workspace,
    editing::{Field, Rows},
};
use kode_tui::{
    focus::FocusState,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    panels::rest::{
        editor::form::{Action, BufferKind, Form, Intent, Prepared, Tab},
        sidebar::TreeChange,
    },
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
fn setup() -> (Workspace, Form, FocusState) {
    let mut model = Workspace::default();
    model.replace_collection(json!({"info":{"name":"API"},"item":[{"name":"Call","request":{"method":"GET","url":"https://example.test/path?q=old","body":{"mode":"raw","raw":"request bytes"},"header":[{"key":"X-Old","value":"value"}]},"variable":[{"key":"secret","value":"MUST-MASK","type":"secret","x-row":7}]}]})).unwrap();
    let path = NodePath(vec![0]);
    let prepared = Prepared::from_model(&model, &path).unwrap();
    let form = Form::new(path, "stable-item".into(), prepared).unwrap();
    let focus = FocusState {
        focused: Some(form.id),
        ..Default::default()
    };
    (model, form, focus)
}
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn ctrl(ch: char) -> InputEvent {
    InputEvent::Key(KeyInput {
        key: Key::Character(ch),
        modifiers: Modifiers {
            ctrl: true,
            ..Default::default()
        },
        repeat: false,
    })
}
fn draw(form: &mut Form, model: &Workspace, width: u16, height: u16) -> Buffer {
    let area = Rect::new(4, 2, width, height);
    let mut buffer = Buffer::empty(area);
    form.render(area, &mut buffer, &Theme::compiled(), model, true)
        .unwrap();
    buffer
}
fn text(buffer: &Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}
fn click(x: u16, y: u16) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action: PointerAction::Down(MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
fn label_position(form: &mut Form, model: &Workspace, label: &str) -> (u16, u16) {
    let buffer = draw(form, model, 80, 40);
    for y in buffer.area.y..buffer.area.bottom() {
        let row: String = (buffer.area.x..buffer.area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        if let Some(x) = row.find(label) {
            return (buffer.area.x + x as u16, y);
        }
    }
    panic!("missing control: {label}");
}
fn press_label(form: &mut Form, model: &mut Workspace, focus: &mut FocusState, label: &str) {
    let (x, y) = label_position(form, model, label);
    form.dispatch(&click(x, y), model, focus).unwrap();
}
#[test]
fn remaining_fields_and_button_families_are_mouse_reachable_and_apply_their_owned_semantics() {
    let (mut model, mut form, mut focus) = setup();
    form.perform(Action::Tab(Tab::Auth), &mut model, &mut focus)
        .unwrap();
    for (label, kind) in [
        ("Bearer", "bearer"),
        ("Basic", "basic"),
        ("API key", "apikey"),
        ("Digest", "digest"),
        ("OAuth 1.0", "oauth1"),
        ("OAuth 2.0", "oauth2"),
        ("Hawk", "hawk"),
        ("AWS Signature", "awsv4"),
        ("NTLM", "ntlm"),
        ("Akamai EdgeGrid", "edgegrid"),
        ("No auth", "noauth"),
    ] {
        press_label(&mut form, &mut model, &mut focus, label);
        assert_eq!(
            model
                .node(&form.path)
                .unwrap()
                .pointer("/request/auth/type")
                .unwrap(),
            kind
        );
    }
    press_label(&mut form, &mut model, &mut focus, "Reveal secrets");
    assert!(form.reveal);
    press_label(&mut form, &mut model, &mut focus, "Hide secrets");
    assert!(!form.reveal);
    press_label(&mut form, &mut model, &mut focus, "Inherit");
    assert!(
        model
            .node(&form.path)
            .unwrap()
            .pointer("/request/auth")
            .is_none()
    );
    form.perform(Action::Tab(Tab::Settings), &mut model, &mut focus)
        .unwrap();
    for (label, field) in [
        ("Follow redirects", "disableRedirects"),
        ("TLS certificate", "disableSslVerification"),
    ] {
        press_label(&mut form, &mut model, &mut focus, label);
        assert_eq!(
            model.node(&form.path).unwrap()["protocolProfileBehavior"][field],
            true
        );
    }
    let (x, y) = label_position(&mut form, &model, "Timeout ms");
    form.dispatch(&click(x + 12, y), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(&InputEvent::Paste("1234".into()), &mut model, &mut focus)
        .unwrap();
    assert_eq!(
        model
            .node(&form.path)
            .unwrap()
            .pointer("/protocolProfileBehavior/requestTimeout")
            .unwrap(),
        "1234"
    );
    press_label(&mut form, &mut model, &mut focus, "Clear cookies");
    assert!(
        form.take_intents()
            .iter()
            .any(|intent| matches!(intent, Intent::ClearCookies))
    );
    form.perform(Action::Tab(Tab::Body), &mut model, &mut focus)
        .unwrap();
    for mode in ["none", "raw", "urlencoded", "formdata", "file", "graphql"] {
        let (x, y) = label_position(&mut form, &model, "none raw urlencoded");
        let offset = "none raw urlencoded formdata file graphql"
            .find(mode)
            .unwrap() as u16;
        form.dispatch(&click(x + offset, y), &mut model, &mut focus)
            .unwrap();
        assert_eq!(
            model
                .node(&form.path)
                .unwrap()
                .pointer("/request/body/mode")
                .unwrap(),
            mode
        );
        if mode == "file" {
            let (x, y) = label_position(&mut form, &model, "File");
            form.dispatch(&click(x + 6, y), &mut model, &mut focus)
                .unwrap();
            form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
            form.dispatch(
                &InputEvent::Paste("body.bin".into()),
                &mut model,
                &mut focus,
            )
            .unwrap();
            assert_eq!(
                model
                    .node(&form.path)
                    .unwrap()
                    .pointer("/request/body/file/src")
                    .unwrap(),
                "body.bin"
            );
        }
    }
    press_label(&mut form, &mut model, &mut focus, "Variables");
    assert_eq!(form.selected_buffer, Some(BufferKind::GraphqlVariables));
    press_label(&mut form, &mut model, &mut focus, "Query");
    assert_eq!(form.selected_buffer, Some(BufferKind::GraphqlQuery));
    form.perform(Action::Tab(Tab::Params), &mut model, &mut focus)
        .unwrap();
    for label in ["+ Query", "+ Path variable"] {
        press_label(&mut form, &mut model, &mut focus, label);
        assert!(form.has_modal());
        form.dispatch(&key(Key::Escape), &mut model, &mut focus)
            .unwrap();
    }
    form.perform(Action::Tab(Tab::Variables), &mut model, &mut focus)
        .unwrap();
    press_label(&mut form, &mut model, &mut focus, "[x]");
    assert_eq!(
        model.node(&form.path).unwrap()["variable"][0]["disabled"],
        true
    );
    press_label(&mut form, &mut model, &mut focus, "secret =");
    assert!(form.has_modal());
    form.dispatch(&key(Key::Escape), &mut model, &mut focus)
        .unwrap();
    press_label(&mut form, &mut model, &mut focus, "Remove");
    assert!(
        model.node(&form.path).unwrap()["variable"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    press_label(&mut form, &mut model, &mut focus, "+ Variable");
    assert!(form.has_modal());
    form.dispatch(&key(Key::Escape), &mut model, &mut focus)
        .unwrap();
    model
        .set_node_field(&form.path, "x-kode-import-incomplete", Some(true.into()))
        .unwrap();
    press_label(&mut form, &mut model, &mut focus, "Import notices");
    assert!(
        form.take_intents()
            .iter()
            .any(|intent| matches!(intent, Intent::Notices))
    );
    press_label(
        &mut form,
        &mut model,
        &mut focus,
        "Reviewed: allow execution",
    );
    assert_eq!(
        model.node(&form.path).unwrap()["x-kode-import-incomplete"],
        false
    );
    let path = NodePath::default();
    let mut form = Form::new(
        path.clone(),
        "root".into(),
        Prepared::from_model(&model, &path).unwrap(),
    )
    .unwrap();
    focus.focused = Some(form.id);
    for (label, field, value) in [
        ("Name", Field::Name, "New collection"),
        ("Description", Field::Description, "Retained description"),
    ] {
        let (x, y) = label_position(&mut form, &model, label);
        form.dispatch(
            &click(x + label.len() as u16 + 2, y),
            &mut model,
            &mut focus,
        )
        .unwrap();
        form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
        form.dispatch(&InputEvent::Paste(value.into()), &mut model, &mut focus)
            .unwrap();
        assert_eq!(form.fields[&field].value, value);
    }
    assert_eq!(model.collection()["info"]["name"], "New collection");
    assert_eq!(
        model.collection()["info"]["description"],
        "Retained description"
    );
}
#[test]
fn inherited_rows_scroll_independently_and_never_push_local_headers_out_of_the_view() {
    let (mut model, mut form, mut focus) = setup();
    let headers: Vec<_> = (0..2000)
        .map(|index| json!({"key":format!("X-Inherited-{index}"),"value":"value"}))
        .collect();
    let mut collection = model.collection().clone();
    collection["x-kode-headers"] = json!(headers);
    model.replace_collection(collection).unwrap();
    let resolver = kode_rest::resolve::RequestResolver::new(
        model.collection(),
        form.path.clone(),
        std::path::Path::new("."),
        None,
    )
    .unwrap();
    form.set_derived(kode_tui::panels::rest::editor::form::Derived {
        headers: resolver.effective_headers().unwrap(),
        ..Default::default()
    });
    form.perform(Action::Tab(Tab::Headers), &mut model, &mut focus)
        .unwrap();
    let first = draw(&mut form, &model, 80, 24);
    let editor = form.geometry.editor;
    assert!(editor.height >= 8);
    assert!(text(&first).contains("Local headers"));
    let area = form.geometry.content;
    form.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: area.x,
            y: area.y,
            action: PointerAction::Scroll {
                horizontal: 0,
                vertical: 3,
            },
            modifiers: Modifiers::default(),
        }),
        &mut model,
        &mut focus,
    )
    .unwrap();
    let next = draw(&mut form, &model, 80, 24);
    assert_eq!(form.geometry.editor, editor);
    assert!(!text(&next).contains("Inherited headers (read-only)"));
    assert!(text(&next).contains("Local headers"));
}
#[test]
fn method_url_fields_and_send_are_real_mouse_targets_and_field_typing_updates_only_the_request() {
    let (mut model, mut form, mut focus) = setup();
    let buffer = draw(&mut form, &model, 80, 26);
    assert!(text(&buffer).contains("REQUEST | Call"));
    form.dispatch(&click(6, 3), &mut model, &mut focus).unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(&InputEvent::Paste("PATCH".into()), &mut model, &mut focus)
        .unwrap();
    assert_eq!(
        model.node(&form.path).unwrap()["request"]["method"],
        "PATCH"
    );
    draw(&mut form, &model, 80, 26);
    form.dispatch(&click(20, 3), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(
        &InputEvent::Paste("https://changed.example.test/?q=next".into()),
        &mut model,
        &mut focus,
    )
    .unwrap();
    assert_eq!(
        model.node(&form.path).unwrap()["request"]["url"],
        "https://changed.example.test/?q=next"
    );
    form.dispatch(&click(81, 3), &mut model, &mut focus)
        .unwrap();
    assert!(
        form.take_intents()
            .iter()
            .any(|intent| matches!(intent, Intent::Send))
    );
    assert_eq!(
        model.node(&form.path).unwrap()["variable"][0]["value"],
        "MUST-MASK"
    );
}
#[test]
fn request_tabs_are_all_reachable_by_keyboard_and_mouse_and_open_the_matching_shared_buffer() {
    let (mut model, mut form, mut focus) = setup();
    for tab in [
        Tab::Auth,
        Tab::Headers,
        Tab::Body,
        Tab::Variables,
        Tab::Settings,
        Tab::Raw,
        Tab::Params,
    ] {
        draw(&mut form, &model, 30, 24);
        form.dispatch(&key(Key::Tab), &mut model, &mut focus)
            .unwrap();
        assert_eq!(form.tab, tab);
    }
    draw(&mut form, &model, 80, 26);
    let rect = form
        .geometry
        .tabs
        .iter()
        .find(|(tab, _)| *tab == Tab::Headers)
        .unwrap()
        .1;
    form.dispatch(&click(rect.x, rect.y), &mut model, &mut focus)
        .unwrap();
    assert_eq!(form.tab, Tab::Headers);
    assert!(
        form.take_intents()
            .iter()
            .any(|intent| matches!(intent, Intent::Buffer(BufferKind::Headers)))
    );
    draw(&mut form, &model, 80, 26);
    assert!(!form.geometry.editor.is_empty());
    form.perform(Action::Tab(Tab::Body), &mut model, &mut focus)
        .unwrap();
    for (mode, buffer) in [
        ("graphql", Some(BufferKind::GraphqlQuery)),
        ("file", None),
        ("formdata", None),
        ("urlencoded", None),
        ("none", None),
        ("raw", Some(BufferKind::RawBody)),
    ] {
        form.take_intents();
        form.perform(Action::Body(mode), &mut model, &mut focus)
            .unwrap();
        let intents = form.take_intents();
        assert_eq!(
            intents
                .iter()
                .find_map(|intent| if let Intent::Buffer(kind) = intent {
                    Some(*kind)
                } else {
                    None
                }),
            buffer
        );
        assert_eq!(
            model.node(&form.path).unwrap()["request"]["body"]["mode"],
            mode
        );
    }
}
#[test]
fn row_dialogs_keep_secrets_masked_and_preserve_metadata_and_reject_stale_model_publication() {
    let (mut model, mut form, mut focus) = setup();
    form.perform(Action::Tab(Tab::Variables), &mut model, &mut focus)
        .unwrap();
    let buffer = draw(&mut form, &model, 80, 26);
    assert!(!text(&buffer).contains("MUST-MASK"));
    assert!(text(&buffer).contains("********"));
    form.perform(Action::Edit(Rows::Variable, 0), &mut model, &mut focus)
        .unwrap();
    let area = Rect::new(0, 0, 90, 24);
    let mut buffer = Buffer::empty(area);
    form.render_modal(area, &mut buffer, &Theme::compiled());
    assert!(!text(&buffer).contains("MUST-MASK"));
    form.dispatch(&key(Key::Tab), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(
        &InputEvent::Paste("new secret".into()),
        &mut model,
        &mut focus,
    )
    .unwrap();
    form.dispatch(&key(Key::Enter), &mut model, &mut focus)
        .unwrap();
    assert!(!form.has_modal());
    assert_eq!(
        model.node(&form.path).unwrap()["variable"][0]["value"],
        "new secret"
    );
    assert_eq!(model.node(&form.path).unwrap()["variable"][0]["x-row"], 7);
    form.perform(Action::Reveal, &mut model, &mut focus)
        .unwrap();
    assert!(text(&draw(&mut form, &model, 80, 26)).contains("new secret"));
    form.perform(Action::Edit(Rows::Variable, 0), &mut model, &mut focus)
        .unwrap();
    model.rename(&form.path, "Externally changed").unwrap();
    form.dispatch(&key(Key::Tab), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(&InputEvent::Paste("stale".into()), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&key(Key::Enter), &mut model, &mut focus)
        .unwrap();
    assert_eq!(
        model.node(&form.path).unwrap()["variable"][0]["value"],
        "new secret"
    );
    assert!(form.message.contains("changed while the dialog"));
}
#[test]
fn every_extended_auth_secret_stays_masked_in_rows_and_value_dialogs() {
    for scheme in kode_rest::auth::SCHEMES {
        for (index, (name, _)) in scheme
            .fields
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| kode_rest::auth::secret_field(name))
        {
            let (mut model, mut form, mut focus) = setup();
            form.perform(Action::Auth(Some(scheme.kind)), &mut model, &mut focus)
                .unwrap();
            let row = kode_rest::model::editing::rows(
                model.node(&form.path).unwrap(),
                &form.path,
                Rows::Auth,
            )
            .unwrap()[index]
                .clone();
            let mut value = kode_rest::model::editing::entry_value(&row, Rows::Auth);
            value.value = "NEVER-PRINT-SECRET".into();
            kode_rest::model::editing::edit_row(&mut model, &form.path, Rows::Auth, index, &value)
                .unwrap();
            form.perform(Action::Tab(Tab::Auth), &mut model, &mut focus)
                .unwrap();
            let buffer = draw(&mut form, &model, 80, 40);
            assert!(
                !text(&buffer).contains("NEVER-PRINT-SECRET"),
                "{} {name}",
                scheme.kind
            );
            form.perform(Action::Edit(Rows::Auth, index), &mut model, &mut focus)
                .unwrap();
            let area = Rect::new(0, 0, 90, 24);
            let mut buffer = Buffer::empty(area);
            form.render_modal(area, &mut buffer, &Theme::compiled());
            assert!(
                !text(&buffer).contains("NEVER-PRINT-SECRET"),
                "dialog: {} {name}",
                scheme.kind
            );
            assert!(text(&buffer).contains("********"));
        }
    }
}
#[test]
fn late_field_paste_and_deleted_item_cannot_mutate_a_different_row_or_replacement_item() {
    let (mut model, mut form, mut focus) = setup();
    form.perform(Action::Focus(Field::Url), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('v'), &mut model, &mut focus).unwrap();
    let proof = form
        .take_intents()
        .into_iter()
        .find_map(|intent| {
            if let Intent::Clipboard { proof, .. } = intent {
                proof
            } else {
                None
            }
        })
        .unwrap();
    form.perform(Action::Focus(Field::Method), &mut model, &mut focus)
        .unwrap();
    assert!(
        !form
            .paste(proof, "wrong target".into(), &mut model, &focus)
            .unwrap()
    );
    form.rebase(&TreeChange::Removed(NodePath(vec![0])), &mut focus);
    assert!(form.closed);
    model.replace_collection(json!({"info":{},"item":[{"name":"Replacement","request":{"url":"https://replacement.example.test"}}]})).unwrap();
    let before = model.export_collection();
    form.perform(Action::Auth(Some("bearer")), &mut model, &mut focus)
        .unwrap();
    assert_eq!(model.collection(), &*before);
}
#[test]
fn import_marker_cookies_settings_and_collection_fields_have_explicit_actions() {
    let (mut model, mut form, mut focus) = setup();
    model
        .set_node_field(&form.path, "x-kode-import-incomplete", Some(true.into()))
        .unwrap();
    assert!(text(&draw(&mut form, &model, 80, 26)).contains("Import incomplete"));
    form.perform(Action::Setting("disableRedirects"), &mut model, &mut focus)
        .unwrap();
    assert_eq!(
        model.node(&form.path).unwrap()["x-kode-import-incomplete"],
        true
    );
    form.perform(Action::Acknowledge, &mut model, &mut focus)
        .unwrap();
    assert_eq!(
        model.node(&form.path).unwrap()["x-kode-import-incomplete"],
        false
    );
    form.perform(Action::Cookies, &mut model, &mut focus)
        .unwrap();
    form.perform(Action::Notices, &mut model, &mut focus)
        .unwrap();
    let intents = form.take_intents();
    assert!(
        intents
            .iter()
            .any(|intent| matches!(intent, Intent::ClearCookies))
    );
    assert!(
        intents
            .iter()
            .any(|intent| matches!(intent, Intent::Notices))
    );
    let path = NodePath::default();
    let mut form = Form::new(
        path.clone(),
        "collection".into(),
        Prepared::from_model(&model, &path).unwrap(),
    )
    .unwrap();
    focus.focused = Some(form.id);
    form.perform(Action::Focus(Field::Name), &mut model, &mut focus)
        .unwrap();
    form.dispatch(&ctrl('a'), &mut model, &mut focus).unwrap();
    form.dispatch(
        &InputEvent::Paste("Edited collection".into()),
        &mut model,
        &mut focus,
    )
    .unwrap();
    assert_eq!(model.collection_name(), "Edited collection");
}
#[test]
fn every_tab_and_keyboard_selection_stays_bounded_in_narrow_and_single_cell_views() {
    let (mut model, mut form, mut focus) = setup();
    for (width, height) in [(80, 24), (30, 12), (10, 6), (1, 1), (0, 0)] {
        for tab in [
            Tab::Params,
            Tab::Auth,
            Tab::Headers,
            Tab::Body,
            Tab::Variables,
            Tab::Settings,
            Tab::Raw,
        ] {
            form.perform(Action::Tab(tab), &mut model, &mut focus)
                .unwrap();
            for _ in 0..5 {
                draw(&mut form, &model, width, height);
                form.dispatch(&key(Key::Down), &mut model, &mut focus)
                    .unwrap();
            }
        }
    }
}
