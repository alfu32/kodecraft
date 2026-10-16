use crossterm::event::MouseButton;
use kode_core::{DomainError, JobTag, RequestId, WorkspaceGeneration};
use kode_lsp::{
    catalog::{Catalog, Platform},
    discovery::DiscoveryOptions,
};
use kode_platform::jobs::Progress;
use kode_tui::{focus::FocusState, input::terminal::*, panels::settings::lsp::*, theme::Theme};
use ratatui::{buffer::Buffer, layout::Rect};
use std::collections::BTreeMap;
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn report(root: &std::path::Path) -> SettingsReport {
    let options = DiscoveryOptions {
        platform: Platform::detect(),
        install_root: root.into(),
        search_path: vec![],
        configured_commands: BTreeMap::new(),
    };
    SettingsReport {
        statuses: options.statuses(&Catalog::compiled().unwrap(), |_| false),
        catalog_path: None,
        install_root: root.into(),
        message: "Catalog refreshed".into(),
        operation_error: None,
    }
}
fn install_report(panel: &mut LspSettings, report: SettingsReport) {
    let job = panel
        .begin(WorkspaceGeneration(1), SettingsAction::Refresh)
        .unwrap()
        .unwrap();
    assert!(panel.complete(job.tag, Ok(report)));
}
#[test]
fn settings_keyboard_mouse_selection_and_worker_acknowledgement_keep_installation_and_running_separate()
 {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("never-created");
    let mut panel = LspSettings::new().unwrap();
    install_report(&mut panel, report(&root));
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    let area = Rect::new(13, 7, 48, 25);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 50));
    panel.render(area, &mut buffer, &Theme::compiled(), true);
    panel.dispatch(&key(Key::Up), &mut focus);
    assert_eq!(panel.selected, panel.statuses.len() - 1);
    assert!(panel.scroll > 0);
    panel.dispatch(&key(Key::Down), &mut focus);
    assert_eq!(panel.selected, 0);
    panel.dispatch(&key(Key::Character('S')), &mut focus);
    assert_eq!(
        panel.take_work(),
        vec![SettingsAction::Lifecycle(
            "rust-analyzer".into(),
            LifecycleAction::Start
        )]
    );
    assert!(!panel.statuses[0].running);
    panel.dispatch(&key(Key::Enter), &mut focus);
    let action = panel.take_work().pop().unwrap();
    let job = panel
        .begin(WorkspaceGeneration(1), action)
        .unwrap()
        .unwrap();
    panel.dispatch(&key(Key::Enter), &mut focus);
    assert!(panel.take_work().is_empty());
    let stale = JobTag {
        request: RequestId::fresh().unwrap(),
        ..job.tag
    };
    assert!(!panel.complete(stale, Err(DomainError::Cancelled)));
    assert!(panel.busy.is_some());
    panel.progress(&Progress {
        tag: job.tag,
        message: "Downloading 64 bytes".into(),
        completed: 64,
        total: Some(128),
        at_millis: 0,
    });
    assert_eq!(panel.message, "Downloading 64 bytes");
    panel.dispatch(&key(Key::Escape), &mut focus);
    assert!(job.cancel.is_cancelled());
    assert!(panel.busy.is_some());
    assert!(panel.complete(job.tag, Err(DomainError::Cancelled)));
    assert!(panel.busy.is_none());
    panel.render(area, &mut buffer, &Theme::compiled(), true);
    let row = area.y + 5 + 4;
    let symbols: String = (area.x..area.right())
        .map(|x| buffer[(x, row)].symbol())
        .collect();
    let x = area.x + symbols.find("y:remove").unwrap() as u16;
    panel.dispatch(
        &InputEvent::Pointer(PointerInput {
            x,
            y: row,
            action: PointerAction::Down(MouseButton::Left),
            modifiers: Modifiers::default(),
        }),
        &mut focus,
    );
    assert_eq!(
        panel.take_work(),
        vec![SettingsAction::Uninstall("rust-analyzer".into())]
    );
    assert!(
        !root.exists(),
        "render/dispatch must not create the install root"
    );
}
#[test]
fn settings_refresh_preserves_selected_server_identity_and_tiny_surfaces_do_not_draw_outside() {
    let directory = tempfile::tempdir().unwrap();
    let mut panel = LspSettings::new().unwrap();
    install_report(&mut panel, report(directory.path()));
    let selected = panel.statuses[0].entry.id.clone();
    let mut reversed = report(directory.path());
    reversed.statuses.reverse();
    install_report(&mut panel, reversed);
    assert_eq!(panel.statuses[panel.selected].entry.id, selected);
    for (width, height) in [(0, 0), (1, 1), (2, 3), (19, 8), (48, 25)] {
        let area = Rect::new(12, 6, width, height);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 50));
        for cell in &mut buffer.content {
            cell.set_symbol("Z");
        }
        panel.render(area, &mut buffer, &Theme::compiled(), true);
        assert_eq!(buffer[(0, 0)].symbol(), "Z");
        assert_eq!(buffer[(89, 49)].symbol(), "Z");
        assert!(!panel.dispatch(&key(Key::Enter), &mut FocusState::default()));
        assert!(panel.take_work().is_empty());
    }
}

#[test]
fn failure_details_keep_full_text_and_operation_identity_and_remain_available_while_busy() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("not-created");
    let mut panel = LspSettings::new().unwrap();
    install_report(&mut panel, report(&root));
    let source = panel.statuses[0].entry.id.clone();
    let job = panel
        .begin(
            WorkspaceGeneration(1),
            SettingsAction::Install(source.clone()),
        )
        .unwrap()
        .unwrap();
    let message = format!(
        "download failed\n{}\nACTIONABLE-END",
        "path segment ".repeat(1500)
    );
    panel.complete(job.tag, Err(DomainError::Unsupported(message.clone())));
    let mut focus = FocusState {
        focused: Some(panel.id),
        ..Default::default()
    };
    panel.dispatch(&key(Key::Function(4)), &mut focus);
    let snapshot = panel.take_work().pop().unwrap();
    let SettingsAction::Details { title, text } = snapshot else {
        panic!("details action expected");
    };
    assert!(title.contains(&source) && title.contains(&job.tag.request.0.to_string()));
    assert!(text.ends_with("ACTIONABLE-END") && text.len() > 16000);
    let busy = panel
        .begin(WorkspaceGeneration(1), SettingsAction::Refresh)
        .unwrap()
        .unwrap();
    for (width, height) in [(48, 25), (19, 8), (1, 1)] {
        let area = Rect::new(12, 6, width, height);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 90, 50));
        panel.render(area, &mut buffer, &Theme::compiled(), true);
        panel.dispatch(
            &InputEvent::Pointer(PointerInput {
                x: area.right() - 1,
                y: area.y + area.height.saturating_sub(1).min(3),
                action: PointerAction::Down(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            &mut focus,
        );
        assert!(
            matches!(panel.take_work().pop(),Some(SettingsAction::Details {text,..}) if text.ends_with("ACTIONABLE-END"))
        );
        assert_eq!(panel.busy, Some(busy.tag));
    }
    panel.complete(busy.tag, Ok(report(&root)));
    assert!(
        text.ends_with("ACTIONABLE-END"),
        "the captured snapshot remains independent of new results"
    );
    assert!(!root.exists());
}
