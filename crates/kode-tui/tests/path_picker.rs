use kode_core::{DomainError, WorkspaceGeneration};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, MonotonicClock},
};
use kode_tui::{
    app::{runtime::AppRuntime, workspace::prepare_workspace},
    focus::{FocusId, FocusState},
    input::terminal::*,
    panels::rest::sidebar::Action,
    theme::Theme,
    widgets::{
        dialog::{DialogAction, TextDialog},
        file_picker::{self, FilePicker, Mode, Report, Request},
    },
};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::json;
use std::{fs, path::Path, sync::Arc, time::Duration};
fn key(key: Key) -> InputEvent {
    modified(key, Modifiers::default())
}
fn modified(key: Key, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers,
        repeat: false,
    })
}
fn ctrl(ch: char) -> InputEvent {
    modified(
        Key::Character(ch),
        Modifiers {
            ctrl: true,
            ..Default::default()
        },
    )
}
fn browse() -> InputEvent {
    modified(
        Key::Character('p'),
        Modifiers {
            alt: true,
            ..Default::default()
        },
    )
}
fn picker(base: &Path, mode: Mode, value: &str) -> (FilePicker, FocusState) {
    let mut focus = FocusState {
        focused: Some(FocusId(900)),
        ..Default::default()
    };
    let prepared = file_picker::prepare(
        base,
        value,
        WorkspaceGeneration(1),
        &CancellationToken::new(),
    )
    .unwrap();
    let picker = FilePicker::new(
        Request::new("Choose path".into(), mode, value.into()).unwrap(),
        prepared,
        &mut focus,
    )
    .unwrap();
    (picker, focus)
}
fn work(picker: &mut FilePicker, focus: &mut FocusState) {
    for job in picker.take_work() {
        let tag = job.tag();
        let result = job.run(&CancellationToken::new());
        assert!(picker.complete(tag, result, focus).unwrap());
    }
}
fn paint(picker: &mut FilePicker, size: (u16, u16)) {
    let area = Rect::new(0, 0, size.0, size.1);
    picker.render(area, &mut Buffer::empty(area), &Theme::compiled());
}
fn replace(picker: &mut FilePicker, focus: &mut FocusState, text: &str) {
    picker.focus = 1;
    picker.dispatch(&ctrl('a'), focus).unwrap();
    picker
        .dispatch(&InputEvent::Paste(text.into()), focus)
        .unwrap();
}

#[test]
fn modes_validate_real_outside_paths_and_new_names_without_creating_or_overwriting() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("path with spaces 界.sql");
    fs::write(&file, "retained").unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        file_picker::validate(Mode::OpenFile, &file, &cancel).unwrap(),
        file
    );
    assert!(file_picker::validate(Mode::OpenFile, outside.path(), &cancel).is_err());
    assert!(file_picker::validate(Mode::Directory, &file, &cancel).is_err());
    assert!(file_picker::validate(Mode::Directory, outside.path(), &cancel).is_ok());
    assert!(file_picker::validate(Mode::AnyExisting, &file, &cancel).is_ok());
    let target = outside.path().join("new 界.sql");
    assert!(file_picker::validate(Mode::SaveFile, &target, &cancel).is_ok());
    assert!(!target.exists());
    assert!(
        file_picker::validate(
            Mode::SaveFile,
            &outside.path().join("missing/target.sql"),
            &cancel
        )
        .is_err()
    );
    assert!(file_picker::validate(Mode::SaveFile, &file, &cancel).is_ok());
    assert_eq!(fs::read_to_string(&file).unwrap(), "retained");
    let prepared = file_picker::prepare(
        root.path(),
        file.to_str().unwrap(),
        WorkspaceGeneration(1),
        &cancel,
    )
    .unwrap();
    assert_eq!(prepared.selected, outside.path());
    assert_eq!(prepared.value, file.to_str().unwrap());
    cancel.cancel();
    assert!(matches!(
        file_picker::prepare(root.path(), "", WorkspaceGeneration(1), &cancel),
        Err(DomainError::Cancelled)
    ));
}

#[test]
fn actual_tree_selection_replaces_old_absolute_candidate_and_selects_file() {
    let directory = tempfile::tempdir().unwrap();
    let a = directory.path().join("a.sql");
    let b = directory.path().join("b 界.sql");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let (mut picker, mut focus) = picker(directory.path(), Mode::OpenFile, a.to_str().unwrap());
    work(&mut picker, &mut focus);
    paint(&mut picker, (90, 26));
    assert_eq!(picker.browser.selected, directory.path());
    picker.dispatch(&key(Key::Down), &mut focus).unwrap();
    assert_eq!(picker.browser.selected, a);
    picker.dispatch(&key(Key::Down), &mut focus).unwrap();
    assert_eq!(picker.browser.selected, b);
    assert_eq!(picker.field.value, b.to_str().unwrap());
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    assert_eq!(picker.result, Some(b));
    assert!(picker.finished);
    assert_eq!(focus.focused, Some(FocusId(900)));
}

#[test]
fn folder_navigation_and_parent_keep_save_name_and_picker_identity() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    let (mut picker, mut focus) = picker(root.path(), Mode::SaveFile, "new 界.sql");
    work(&mut picker, &mut focus);
    let id = picker.id;
    picker.dispatch(&key(Key::Down), &mut focus).unwrap();
    assert_eq!(
        picker.field.value,
        root.path().join("sub/new 界.sql").to_str().unwrap()
    );
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    work(&mut picker, &mut focus);
    assert_eq!(picker.id, id);
    assert_eq!(focus.focused, Some(id));
    picker.dispatch(&key(Key::Function(4)), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    work(&mut picker, &mut focus);
    assert_eq!(
        picker.field.value,
        root.path().join("new 界.sql").to_str().unwrap()
    );
    picker.focus = 4;
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    assert_eq!(picker.result, Some(root.path().join("new 界.sql")));
    assert!(!root.path().join("new 界.sql").exists());
}

#[test]
fn stale_validation_after_typing_focus_change_cancel_or_new_request_cannot_choose() {
    let root = tempfile::tempdir().unwrap();
    for change in 0..4 {
        let (mut picker, mut focus) = picker(root.path(), Mode::SaveFile, "a.sql");
        work(&mut picker, &mut focus);
        picker.focus = 4;
        picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
        let job = picker.take_work().pop().unwrap();
        let tag = job.tag();
        // Work already finished when the user changes the state: publication still must reject it.
        let result = job.run(&CancellationToken::new());
        match change {
            0 => replace(&mut picker, &mut focus, "different.sql"),
            1 => {
                picker.dispatch(&key(Key::Tab), &mut focus).unwrap();
            }
            2 => {
                picker.dispatch(&key(Key::Escape), &mut focus).unwrap();
            }
            _ => {
                picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
            }
        }
        assert!(!picker.complete(tag, result, &mut focus).unwrap());
        assert!(picker.result.is_none());
    }
}

#[test]
fn loading_errors_preserve_selection_and_typed_path_and_can_be_corrected() {
    let root = tempfile::tempdir().unwrap();
    let (mut picker, mut focus) = picker(root.path(), Mode::OpenFile, "missing.json");
    let tree = picker.take_work().pop().unwrap();
    let selected = picker.browser.selected.clone();
    let value = picker.field.value.clone();
    assert!(
        picker
            .complete(
                tree.tag(),
                Ok(Report::Tree(Err(DomainError::Unsupported(
                    "cannot read directory".into()
                )))),
                &mut focus
            )
            .unwrap()
    );
    assert_eq!(picker.browser.selected, selected);
    assert_eq!(picker.field.value, value);
    assert!(
        picker
            .browser
            .error
            .as_ref()
            .unwrap()
            .contains("cannot read directory")
    );
    picker.focus = 4;
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    assert!(picker.error.is_some());
    assert!(!picker.finished);
    let existing = root.path().join("corrected.json");
    fs::write(&existing, "{}").unwrap();
    replace(&mut picker, &mut focus, existing.to_str().unwrap());
    picker.focus = 4;
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    work(&mut picker, &mut focus);
    assert_eq!(picker.result, Some(existing));
}

#[test]
fn all_chooser_controls_remain_visible_when_focused_at_tiny_sizes() {
    let root = tempfile::tempdir().unwrap();
    for size in [(90, 26), (20, 6), (10, 3), (1, 1), (1, 2), (8, 4), (0, 0)] {
        let (mut picker, mut focus) = picker(root.path(), Mode::SaveFile, "new.sql");
        work(&mut picker, &mut focus);
        for index in 0..6 {
            assert_eq!(picker.focus, index);
            paint(&mut picker, size);
            let g = picker.geometry;
            let rect = [g.rows, g.field, g.parent, g.go, g.confirm, g.cancel][index];
            if size != (0, 0) {
                assert!(!rect.is_empty(), "{size:?} control {index}");
            }
            picker.dispatch(&key(Key::Tab), &mut focus).unwrap();
        }
    }
}

#[test]
fn path_dialog_browse_mouse_keyboard_and_tiny_focus_preserve_draft() {
    for size in [(80, 24), (20, 6), (1, 1)] {
        let area = Rect::new(0, 0, size.0, size.1);
        let mut dialog =
            TextDialog::new("Save".into(), "draft.sql".into()).with_path(Mode::SaveFile);
        dialog.field.anchor = Some(0);
        dialog.render(area, &mut Buffer::empty(area), &Theme::compiled());
        dialog.handle(&browse());
        assert_eq!(dialog.action.take(), Some(DialogAction::Browse));
        assert!(!dialog.finished);
        assert_eq!(dialog.field.value, "draft.sql");
        assert_eq!(dialog.field.anchor, Some(0));
        dialog.focus = 3;
        dialog.render(area, &mut Buffer::empty(area), &Theme::compiled());
        let hit = dialog.layout(area).browse;
        assert!(!hit.is_empty());
        dialog.handle(&InputEvent::Pointer(PointerInput {
            x: hit.x,
            y: hit.y,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers::default(),
        }));
        assert_eq!(dialog.action.take(), Some(DialogAction::Browse));
        dialog.handle(&key(Key::Enter));
        assert_eq!(dialog.action.take(), Some(DialogAction::Browse));
    }
    let mut ordinary = TextDialog::new("Name".into(), "name".into());
    ordinary.handle(&browse());
    assert_ne!(ordinary.action, Some(DialogAction::Browse));
}

fn app(root: &Path) -> AppRuntime {
    let prepared = prepare_workspace(
        root,
        WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    AppRuntime::with_services(
        prepared,
        Arc::new(ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap()
}
async fn until(app: &mut AppRuntime, test: impl Fn(&AppRuntime) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            app.pump().unwrap();
            let area = Rect::new(0, 0, 100, 34);
            app.state
                .render(area, &mut Buffer::empty(area), &Theme::compiled())
                .unwrap();
            if test(app) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
fn rest_dialog(app: &mut AppRuntime, action: Action) {
    app.state
        .rest
        .sidebar
        .perform(action, &mut app.state.rest.model, &mut app.state.focus)
        .unwrap();
    app.state.drain_ui().unwrap();
}
async fn choose(app: &mut AppRuntime, path: &Path) {
    app.state.dispatch(&browse()).unwrap();
    until(app, |app| app.state.path_picker.is_some()).await;
    // Tab from tree to the actual path text field, paste a new absolute path, then Select.
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(path.to_str().unwrap().into()))
        .unwrap();
    for _ in 0..3 {
        app.state.dispatch(&key(Key::Tab)).unwrap();
    }
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(app, |app| app.state.path_picker.is_none()).await;
}

async fn request_form(app: &mut AppRuntime, collection: serde_json::Value) {
    use kode_tui::panels::rest::editor::form::{Action, Tab};
    app.state.rest.model.replace_collection(collection).unwrap();
    app.state
        .open_rest_node(kode_rest::model::NodePath(vec![0]))
        .unwrap();
    until(app, |app| app.state.rest.form.is_some()).await;
    app.state
        .rest
        .form
        .as_mut()
        .unwrap()
        .perform(
            Action::Tab(Tab::Body),
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    app.state.drain_ui().unwrap();
}

async fn receiver() -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chosen", listener.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
            assert!(header.len() < 65536);
        }
        let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
        let length = header
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap();
        assert!(length < 1024 * 1024);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        body
    });
    (url, worker)
}

#[tokio::test]
async fn binary_and_multipart_requests_send_actual_selected_file_bytes_and_preserve_metadata() {
    use kode_rest::model::editing::{Field, Rows};
    use kode_tui::panels::rest::editor::form::Action;
    for multipart in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let first = outside.path().join("first 界.bin");
        let chosen = outside.path().join("chosen file 界.bin");
        fs::write(&first, b"FIRST-FILE").unwrap();
        fs::write(&chosen, b"CHOSEN-FILE\0\xff").unwrap();
        let (url, worker) = receiver().await;
        let body = if multipart {
            json!({"mode":"formdata","x-body":"keep","formdata":[{"key":"upload","type":"file","src":[first],"x-row":{"keep":true}}]})
        } else {
            json!({"mode":"file","file":{"src":"old.bin","x-file":7},"x-body":"keep"})
        };
        let mut app = app(root.path());
        request_form(&mut app,json!({"info":{"name":"Picker"},"item":[{"name":"Upload","request":{"method":"POST","url":url,"body":body}}]})).await;
        let action = if multipart {
            Action::Edit(Rows::FormData, 0)
        } else {
            Action::Focus(Field::File)
        };
        app.state
            .rest
            .form
            .as_mut()
            .unwrap()
            .perform(action, &mut app.state.rest.model, &mut app.state.focus)
            .unwrap();
        choose(&mut app, &chosen).await;
        if multipart {
            // The row dialog still owns its draft until submission.
            assert_eq!(
                app.state.rest.model.collection()["item"][0]["request"]["body"]["formdata"][0]["src"],
                json!([first])
            );
            app.state.dispatch(&key(Key::Enter)).unwrap();
        }
        let body = &app.state.rest.model.collection()["item"][0]["request"]["body"];
        assert_eq!(body["x-body"], "keep");
        if multipart {
            assert_eq!(body["formdata"][0]["src"], json!([first, chosen]));
            assert_eq!(body["formdata"][0]["x-row"], json!({"keep":true}));
        } else {
            assert_eq!(body["file"]["src"], chosen.to_str().unwrap());
            assert_eq!(body["file"]["x-file"], 7);
        }
        app.state
            .rest
            .form
            .as_mut()
            .unwrap()
            .perform(
                Action::Send,
                &mut app.state.rest.model,
                &mut app.state.focus,
            )
            .unwrap();
        app.state.drain_ui().unwrap();
        until(&mut app, |app| !app.state.rest.run.responses.is_empty()).await;
        let sent = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        if multipart {
            assert!(
                sent.windows(b"FIRST-FILE".len())
                    .any(|bytes| bytes == b"FIRST-FILE")
            );
            assert!(
                sent.windows(b"CHOSEN-FILE\0\xff".len())
                    .any(|bytes| bytes == b"CHOSEN-FILE\0\xff")
            );
        } else {
            assert_eq!(sent, b"CHOSEN-FILE\0\xff");
        }
        app.flush_before_leave().await.unwrap();
        app.quiesce().await.unwrap();
    }
}

#[tokio::test]
async fn jar_choice_updates_only_current_classpath_component_and_real_saved_datasource() {
    use kode_database::{
        bridge::runtime::Installation,
        catalog::{DriverRegistry, UrlCatalog},
        config::{DataSourceRepository, DatabasePaths},
    };
    use kode_tui::app::runtime::database::DatabaseConfiguration;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let chosen = outside.path().join(if cfg!(unix) {
        "driver 界.jar "
    } else {
        "driver 界.jar"
    });
    fs::write(&chosen, b"selected test JAR").unwrap();
    let separator = if cfg!(windows) { ';' } else { ':' };
    let previous = format!("first.jar{separator}old.jar{separator}third.jar");
    let definition=serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","jarPath":previous,"x-driver":7},"jdbcUrl":"jdbc:h2:mem:picker","properties":{"keep":"yes"},"x-datasource":9})).unwrap();
    let mut app = app(root.path());
    let db = root.path().join("db");
    let registry = Arc::new(DriverRegistry::compiled().unwrap());
    app.configure_database(DatabaseConfiguration {
        paths: DatabasePaths {
            root: db.clone(),
            legacy_root: None,
            plugin_root: db.join("plugins"),
            legacy_plugin_root: None,
        },
        urls: Arc::new(UrlCatalog::compiled(&registry).unwrap()),
        registry,
        installation: Installation {
            java: "/synthetic/absent-java".into(),
            helper_candidates: vec![],
        },
    })
    .unwrap();
    app.state
        .database
        .edit(Some(definition), &mut app.state.focus)
        .unwrap();
    let form = app.state.database.form.as_mut().unwrap();
    form.focus = 11;
    form.fields[10].cursor = "first.jar:old".len();
    choose(&mut app, &chosen).await;
    let expected = format!(
        "first.jar{separator}{}{separator}third.jar",
        chosen.to_str().unwrap()
    );
    let form = app.state.database.form.as_ref().unwrap();
    assert_eq!(form.fields[10].value, expected);
    assert_eq!(form.focus, 11);
    app.state.dispatch(&ctrl('s')).unwrap();
    until(&mut app, |app| app.state.database.form.is_none()).await;
    let saved = DataSourceRepository::new(db).load().unwrap();
    assert_eq!(saved[0].driver.jar_path.as_deref(), Some(expected.as_str()));
    assert_eq!(saved[0].extra["x-datasource"], 9);
    assert_eq!(saved[0].driver.extra["x-driver"], 7);
    assert_eq!(saved[0].properties["keep"], "yes");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[test]
fn multipart_type_changes_remove_browse_and_invalidate_source_proof_at_tiny_sizes() {
    use kode_tui::panels::rest::sidebar::entry_dialog::{EntryDialog, EntryValue};
    let mut dialog = EntryDialog::new(
        "Form field".into(),
        EntryValue {
            key: "upload".into(),
            value: "[\"first.bin\",\"second.bin\"]".into(),
            description: "file".into(),
            kind: "string".into(),
            enabled: true,
        },
        false,
        "Type text/file",
    )
    .unwrap();
    dialog.file_source = true;
    let (_, proof) = dialog.path_request().unwrap().unwrap();
    assert!(dialog.apply_path(&proof, Path::new("chosen.bin")).unwrap());
    assert_eq!(
        dialog.fields[1].value,
        "[\"first.bin\",\"second.bin\",\"chosen.bin\"]"
    );
    for size in [(80, 24), (10, 3), (1, 1)] {
        let area = Rect::new(0, 0, size.0, size.1);
        dialog.focus = 5;
        dialog.render(area, &mut Buffer::empty(area), &Theme::compiled());
        assert!(!dialog.layout(area).browse.is_empty());
        dialog.dispatch(&key(Key::Enter));
        assert!(dialog.take_browse());
        assert!(!dialog.finished);
    }
    let (_, proof) = dialog.path_request().unwrap().unwrap();
    dialog.fields[2].value = "text".into();
    assert!(dialog.path_request().unwrap().is_none());
    assert!(
        !dialog
            .apply_path(&proof, Path::new("must-not-apply.bin"))
            .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_tree_selection_never_retargets_confirmation_to_previous_valid_file() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let root = tempfile::tempdir().unwrap();
    let valid = root.path().join("a.json");
    fs::write(&valid, "{}").unwrap();
    let invalid = root.path().join(OsString::from_vec(b"\xff.json".to_vec()));
    fs::write(&invalid, "bad").unwrap();
    let (mut picker, mut focus) = picker(root.path(), Mode::OpenFile, valid.to_str().unwrap());
    work(&mut picker, &mut focus);
    picker.dispatch(&key(Key::Down), &mut focus).unwrap();
    picker.dispatch(&key(Key::Down), &mut focus).unwrap();
    assert_eq!(picker.browser.selected, invalid);
    picker.focus = 4;
    picker.dispatch(&key(Key::Enter), &mut focus).unwrap();
    assert!(picker.take_work().is_empty());
    assert!(picker.error.as_ref().unwrap().contains("not UTF-8"));
    assert!(picker.result.is_none());
}

#[test]
fn jar_draft_rejects_separator_paths_and_late_driver_or_reopened_form_results() {
    use kode_database::catalog::{DriverRegistry, UrlCatalog};
    use kode_tui::focus::EventTarget;
    use kode_tui::panels::database::connection_dialog::ConnectionDialog;
    let registry = Arc::new(DriverRegistry::compiled().unwrap());
    let urls = Arc::new(UrlCatalog::compiled(&registry).unwrap());
    let mut form = ConnectionDialog::new(registry.clone(), urls.clone(), None).unwrap();
    form.focus = 11;
    let (_, proof) = form.path_request().unwrap().unwrap();
    let separator = if cfg!(windows) { ';' } else { ':' };
    assert!(!form.apply_path(&proof, Path::new(&format!("has{separator}separator.jar"))));
    assert!(form.fields[10].value.is_empty());
    assert!(form.status.contains("separator"));
    form.selected_driver = (form.selected_driver + 1) % registry.definitions().len();
    assert!(!form.apply_path(&proof, Path::new("late.jar")));
    let mut replacement = ConnectionDialog::new(registry, urls, None).unwrap();
    replacement.focus = 11;
    assert!(!replacement.apply_path(&proof, Path::new("late.jar")));
    for (width, height) in [(60, 24), (10, 3), (1, 1)] {
        replacement.focus = 20;
        let area = Rect::new(0, 0, width, height);
        replacement.render(area, &mut Buffer::empty(area), &Theme::compiled());
        replacement.dispatch(&key(Key::Enter));
        assert!(replacement.take_actions().iter().any(|action| matches!(
            action,
            kode_tui::panels::database::connection_dialog::Action::Browse
        )));
        assert!(replacement.path_request().unwrap().is_some());
    }
}

#[tokio::test]
async fn native_clipboard_path_paste_uses_worker_and_cannot_mutate_after_focus_moves() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    rest_dialog(&mut app, Action::Export);
    app.state.dispatch(&browse()).unwrap();
    until(&mut app, |app| app.state.path_picker.is_some()).await;
    let value = root
        .path()
        .join("clipboard path 界.json")
        .to_str()
        .unwrap()
        .to_string();
    app.state.clipboard.remember(&value).unwrap();
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state.dispatch(&ctrl('v')).unwrap();
    until(&mut app, |app| {
        app.state.path_picker.as_ref().unwrap().picker.field.value == value
    })
    .await;
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state.dispatch(&ctrl('c')).unwrap();
    assert_eq!(app.state.clipboard.local_text(), value);
    app.state.clipboard.remember("late paste").unwrap();
    app.state.dispatch(&ctrl('v')).unwrap();
    app.state.dispatch(&key(Key::Tab)).unwrap();
    // Pump through actual worker completion without allowing its old proof to modify the field.
    for _ in 0..10 {
        app.pump().unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        app.state.path_picker.as_ref().unwrap().picker.field.value,
        value
    );
    app.state.dispatch(&key(Key::Escape)).unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn replaced_collection_or_closed_and_reopened_modal_rejects_inflight_choices() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    rest_dialog(&mut app, Action::Export);
    app.state.dispatch(&browse()).unwrap();
    until(&mut app, |app| app.state.path_picker.is_some()).await;
    app.state
        .rest
        .model
        .replace_collection(json!({"info":{"name":"Replacement"},"item":[]}))
        .unwrap();
    app.pump().unwrap();
    assert!(app.state.path_picker.is_none());
    app.state.rest.sidebar.dismiss(&mut app.state.focus);
    rest_dialog(&mut app, Action::Export);
    app.state.dispatch(&browse()).unwrap();
    app.state.rest.sidebar.dismiss(&mut app.state.focus);
    rest_dialog(&mut app, Action::Export);
    for _ in 0..10 {
        app.pump().unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(app.state.path_picker.is_none());
    assert!(app.state.rest.sidebar.has_modal());
    assert_eq!(app.state.rest.model.collection_name(), "Replacement");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn pending_project_chooser_escape_focus_loss_and_workspace_departure_do_not_reopen_it() {
    use kode_tui::{app::state::AppWork, panels::project::ProjectWork};
    let root = tempfile::tempdir().unwrap();
    let next = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    for lose_focus in [false, true] {
        app.state
            .work
            .push(AppWork::Project(ProjectWork::AddSource));
        app.pump().unwrap();
        assert!(app.state.pending_picker.is_some());
        app.state
            .dispatch(&if lose_focus {
                InputEvent::Focus(false)
            } else {
                key(Key::Escape)
            })
            .unwrap();
        app.state.dispatch(&InputEvent::Focus(true)).unwrap();
        for _ in 0..10 {
            app.pump().unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(app.state.picker.is_none());
    }
    rest_dialog(&mut app, Action::Export);
    app.state.dispatch(&browse()).unwrap();
    until(&mut app, |app| app.state.path_picker.is_some()).await;
    app.switch_workspace(next.path().into()).await.unwrap();
    assert!(app.state.path_picker.is_none());
    assert!(app.state.picker.is_none());
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn source_exclusion_and_workspace_selectors_use_same_chooser_and_config_guards() {
    use kode_tui::{
        app::state::{AppWork, PickerKind},
        panels::project::ProjectWork,
    };
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("src 界");
    fs::create_dir(&source).unwrap();
    let file = source.join("excluded.txt");
    fs::write(&file, "exclude").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    for (work, mode, path) in [
        (ProjectWork::AddSource, Mode::Directory, &source),
        (ProjectWork::AddExclusion, Mode::AnyExisting, &file),
        (ProjectWork::AddExclusion, Mode::AnyExisting, &source),
        (
            ProjectWork::AddSource,
            Mode::Directory,
            &outside.path().to_path_buf(),
        ),
    ] {
        app.state.work.push(AppWork::Project(work));
        until(&mut app, |app| app.state.picker.is_some()).await;
        assert_eq!(app.state.picker.as_ref().unwrap().1.request.mode, mode);
        app.state.dispatch(&key(Key::Tab)).unwrap();
        app.state.dispatch(&ctrl('a')).unwrap();
        app.state
            .dispatch(&InputEvent::Paste(path.to_str().unwrap().into()))
            .unwrap();
        for _ in 0..3 {
            app.state.dispatch(&key(Key::Tab)).unwrap();
        }
        app.state.dispatch(&key(Key::Enter)).unwrap();
        until(&mut app, |app| app.state.picker.is_none()).await;
        if path == &source && mode == Mode::Directory {
            until(&mut app, |app| {
                app.state
                    .project
                    .config
                    .source_roots
                    .contains(&"src 界".into())
            })
            .await;
        } else if path == &file {
            until(&mut app, |app| {
                app.state
                    .project
                    .config
                    .exclusions
                    .contains(&"src 界/excluded.txt".into())
            })
            .await;
        } else if path == &source {
            until(&mut app, |app| {
                app.state
                    .project
                    .config
                    .exclusions
                    .contains(&"src 界".into())
            })
            .await;
        } else {
            until(&mut app, |app| {
                app.state.project_panel.message.contains("outside")
            })
            .await;
        }
    }
    // Exercise the actual Files [change] button to retain workspace-switch ownership.
    let area = Rect::new(0, 0, 100, 34);
    let mut buffer = Buffer::empty(area);
    app.state
        .render(area, &mut buffer, &Theme::compiled())
        .unwrap();
    let point = (0..34)
        .find_map(|y| {
            let line: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
            line.find("[change]").map(|x| (x as u16, y))
        })
        .unwrap();
    app.state
        .dispatch(&InputEvent::Pointer(PointerInput {
            x: point.0,
            y: point.1,
            action: PointerAction::Down(crossterm::event::MouseButton::Left),
            modifiers: Modifiers::default(),
        }))
        .unwrap();
    until(&mut app, |app| app.state.picker.is_some()).await;
    assert_eq!(app.state.picker.as_ref().unwrap().0, PickerKind::Workspace);
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste(outside.path().to_str().unwrap().into()))
        .unwrap();
    for _ in 0..3 {
        app.state.dispatch(&key(Key::Tab)).unwrap();
    }
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| app.state.switch_requested.is_some()).await;
    assert_eq!(app.state.switch_requested.as_deref(), Some(outside.path()));
    app.switch_workspace(outside.path().into()).await.unwrap();
    assert_eq!(app.state.project.root, outside.path());
    assert_eq!(app.state.generation, WorkspaceGeneration(2));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn rest_postman_import_and_export_use_chosen_outside_paths_and_existing_write_guard() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let input = outside.path().join("collection 界.json");
    let original = json!({"info":{"name":"Chosen collection"},"item":[],"x-future":{"keep":true}});
    fs::write(&input, original.to_string()).unwrap();
    let mut app = app(root.path());
    rest_dialog(&mut app, Action::ImportPostman);
    let original_focus = app.state.focus.focused;
    choose(&mut app, &input).await;
    assert_eq!(app.state.focus.focused, original_focus);
    assert_ne!(app.state.rest.model.collection(), &original);
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state.rest.model.collection() == &original
    })
    .await;
    let output = outside.path().join("export 界.json");
    rest_dialog(&mut app, Action::Export);
    choose(&mut app, &output).await;
    assert!(!output.exists());
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| app.state.rest.exports.contains_key(&output)).await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&output).unwrap()).unwrap(),
        original
    );
    fs::write(&output, "external change").unwrap();
    rest_dialog(&mut app, Action::Export);
    choose(&mut app, &output).await;
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .sidebar
            .message
            .contains("file changed on disk")
    })
    .await;
    assert_eq!(fs::read_to_string(&output).unwrap(), "external change");
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn openapi_chosen_file_retains_destination_folder_and_request_metadata() {
    use kode_rest::model::NodePath;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let input = outside.path().join("api 界.yaml");
    fs::write(&input,"openapi: 3.0.3\ninfo:\n  title: Selected API\n  version: 1.0.0\npaths:\n  /chosen:\n    get:\n      summary: Chosen endpoint\n      responses:\n        '200':\n          description: OK\n").unwrap();
    let mut app = app(root.path());
    let parent = app
        .state
        .rest
        .model
        .add_folder(&NodePath::default(), "Destination")
        .unwrap();
    rest_dialog(&mut app, Action::ImportOpenApi(parent.clone()));
    choose(&mut app, &input).await;
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state
            .rest
            .model
            .node(&parent)
            .and_then(|node| node["item"].as_array())
            .is_some_and(|items| !items.is_empty())
    })
    .await;
    let folder = app.state.rest.model.node(&parent).unwrap();
    assert_eq!(folder["name"], "Destination");
    assert!(folder["item"].to_string().contains("chosen"));
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}

#[tokio::test]
async fn cancel_and_stale_prepare_preserve_rest_draft_and_do_not_reopen_a_closed_modal() {
    let root = tempfile::tempdir().unwrap();
    let mut app = app(root.path());
    rest_dialog(&mut app, Action::Export);
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("original draft.json".into()))
        .unwrap();
    let focus = app.state.focus.focused;
    app.state.dispatch(&browse()).unwrap();
    until(&mut app, |app| app.state.path_picker.is_some()).await;
    app.state.dispatch(&key(Key::Tab)).unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("discarded.json".into()))
        .unwrap();
    app.state.dispatch(&key(Key::Escape)).unwrap();
    assert!(app.state.path_picker.is_none());
    assert_eq!(app.state.focus.focused, focus);
    app.state.dispatch(&browse()).unwrap();
    until(&mut app, |app| app.state.path_picker.is_some()).await;
    assert!(
        app.state
            .path_picker
            .as_ref()
            .unwrap()
            .picker
            .field
            .value
            .ends_with("original draft.json")
    );
    app.state.dispatch(&key(Key::Escape)).unwrap();
    app.state.dispatch(&browse()).unwrap();
    // Close the owner before its queued preparation can publish.
    app.state.dispatch(&key(Key::Escape)).unwrap();
    assert!(!app.state.rest.sidebar.has_modal());
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
    assert!(app.state.path_picker.is_none());
    assert!(!root.path().join("discarded.json").exists());
}

#[tokio::test]
async fn sql_save_uses_chosen_new_filename_and_keeps_external_change_protection() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join(if cfg!(unix) {
        "query 界.sql "
    } else {
        "query 界.sql"
    });
    let mut app = app(root.path());
    let definition=serde_json::from_value(json!({"id":"fixture","name":"Fixture","driver":{"id":"h2","driverClass":"org.h2.Driver","coordinates":"com.h2database:h2:2.2.224"},"jdbcUrl":"jdbc:h2:mem:picker"})).unwrap();
    app.state.database.tree.replace_sources(vec![definition]);
    app.state
        .open_database_console("fixture".into(), String::new())
        .unwrap();
    app.state.dispatch(&ctrl('a')).unwrap();
    app.state
        .dispatch(&InputEvent::Paste("select 'chosen 界';".into()))
        .unwrap();
    app.state.dispatch(&ctrl('s')).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_some()).await;
    choose(&mut app, &target).await;
    assert!(!target.exists());
    assert_eq!(
        app.state.sql.dialog.as_ref().unwrap().dialog.field.value,
        target.to_str().unwrap()
    );
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_none()).await;
    assert_eq!(fs::read_to_string(&target).unwrap(), "select 'chosen 界';");
    app.state
        .dispatch(&InputEvent::Paste("--edit".into()))
        .unwrap();
    fs::write(&target, "external SQL").unwrap();
    app.state.dispatch(&ctrl('s')).unwrap();
    until(&mut app, |app| app.state.sql.dialog.is_some()).await;
    choose(&mut app, &target).await;
    app.state.dispatch(&key(Key::Enter)).unwrap();
    until(&mut app, |app| {
        app.state
            .sql
            .dialog
            .as_ref()
            .is_some_and(|dialog| !dialog.busy && !dialog.dialog.finished)
    })
    .await;
    assert_eq!(fs::read_to_string(&target).unwrap(), "external SQL");
    app.state.dispatch(&key(Key::Escape)).unwrap();
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
