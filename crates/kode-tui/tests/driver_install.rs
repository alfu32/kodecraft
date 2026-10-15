use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_database::{
    bridge::client::{BridgeError, Failure},
    drivers::{Installed, Phase, Progress},
};
use kode_tui::{
    focus::EventTarget,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers},
    panels::database::driver_install::{Action, DriverInstallDialog},
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use tokio::sync::watch;
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn tag(workspace: u64) -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(workspace),
        request: RequestId::fresh().unwrap(),
        document: None,
    }
}
fn installed() -> Installed {
    Installed {
        provider_id: "h2".into(),
        resolved_version: "2.2.224".into(),
        classpath_digest: "0".repeat(64),
        source: "local-classpath".into(),
        manifest_path: "/synthetic/install-v1.json".into(),
        classpath: vec!["/synthetic/h2.jar".into()],
        reused: false,
        cleanup_warning: false,
    }
}
#[test]
fn cancellation_is_one_action_and_only_the_matching_final_reply_can_finish_the_dialog() {
    let job = tag(1);
    let (send, receive) = watch::channel(Progress::default());
    let mut dialog = DriverInstallDialog::new(job, "H2".into(), receive);
    dialog.dispatch(&key(Key::Escape));
    assert_eq!(dialog.take_action(), Some(Action::Cancel));
    assert!(dialog.cancel_requested());
    assert!(!dialog.done);
    dialog.dispatch(&key(Key::Escape));
    dialog.dispatch(&key(Key::Enter));
    assert!(dialog.take_action().is_none());
    send.send_modify(|progress| progress.phase = Phase::Ready);
    assert!(
        !dialog.done,
        "progress/cancel acknowledgements cannot finish a job"
    );
    assert!(!dialog.complete(tag(2), Ok(installed())));
    assert!(!dialog.done);
    assert!(dialog.complete(job, Ok(installed())));
    assert!(dialog.done && dialog.success);
    assert!(dialog.message.contains("Driver installed"));
    assert!(!dialog.complete(
        job,
        Err(BridgeError::new(Failure::Transport, "stale error"))
    ));
    assert!(dialog.success);
    dialog.dispatch(&key(Key::Enter));
    assert_eq!(dialog.take_action(), Some(Action::Dismiss));
}
#[test]
fn pending_unknown_totals_small_layouts_and_unknown_outcomes_never_report_false_completion() {
    let job = tag(1);
    let (send, receive) = watch::channel(Progress {
        phase: Phase::Downloading,
        completed_bytes: 3 * 1024 * 1024,
        total_bytes: None,
    });
    let mut dialog = DriverInstallDialog::new(job, "H2\n界".into(), receive);
    let theme = Theme::parse("");
    for (width, height) in [(82, 12), (42, 9), (1, 1), (0, 0)] {
        let area = Rect::new(2, 3, width, height);
        let mut buffer = Buffer::empty(area);
        dialog.render(area, &mut buffer, &theme);
        assert!(!dialog.done);
        if width == 82 {
            let text = buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("3.0 MiB received"));
            assert!(!text.contains("100%"));
        }
    }
    let mut error = BridgeError::new(Failure::Transport, "helper stopped");
    error.unknown_outcome = true;
    assert!(dialog.complete(job, Err(error)));
    assert!(!dialog.success);
    assert!(dialog.message.contains("refresh driver status"));
    send.send_modify(|progress| progress.phase = Phase::Ready);
    let area = Rect::new(0, 0, 82, 12);
    dialog.render(area, &mut Buffer::empty(area), &theme);
    assert!(!dialog.success);
}
