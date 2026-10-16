use super::*;
use crate::{input::terminal::*, panels::rest::sidebar::Action, widgets::file_picker::Work};
use kode_platform::jobs::CancellationToken;

#[tokio::test]
async fn slow_path_validation_keeps_its_scheduler_owner_until_publication() {
    let root = tempfile::tempdir().unwrap();
    let prepared = workspace::prepare_workspace(
        root.path(),
        kode_core::WorkspaceGeneration(1),
        None,
        &CancellationToken::new(),
    )
    .unwrap();
    let mut app = AppRuntime::with_services(
        prepared,
        Arc::new(kode_platform::clipboard::ClipboardService::internal()),
        Arc::new(MonotonicClock::default()),
    )
    .unwrap();
    app.state
        .rest
        .sidebar
        .perform(
            Action::Export,
            &mut app.state.rest.model,
            &mut app.state.focus,
        )
        .unwrap();
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Character('p'),
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            repeat: false,
        }))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while app.state.path_picker.is_none() {
            app.pump().unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    app.state.path_picker.as_mut().unwrap().picker.focus = 4;
    app.state
        .dispatch(&InputEvent::Key(KeyInput {
            key: Key::Enter,
            modifiers: Modifiers::default(),
            repeat: false,
        }))
        .unwrap();
    let mut validation = None;
    for work in app.state.take_work() {
        match work {
            AppWork::PathWork {
                request,
                work: work @ Work::Validate { .. },
            } => validation = Some((request, work)),
            work => app.accept(work).unwrap(),
        }
    }
    let (request, work) = validation.unwrap();
    let tag = work.tag();
    let (entered, started) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    app.blocking(
        tag,
        OwnerId(request.0),
        "path-validate",
        false,
        Context::PathWork {
            request,
            tree: false,
        },
        Box::new(move |cancel| {
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(3)).unwrap();
            work.run(&cancel).map(Output::PathReport)
        }),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            app.pump().unwrap();
            if started.try_recv().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    // Several real event-loop passes while a worker is blocked used to prune this owner.
    for _ in 0..3 {
        app.pump().unwrap();
        tokio::task::yield_now().await;
    }
    assert!(app.contexts.contains_key(&tag));
    assert!(app.state.path_picker.is_some());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while app.state.path_picker.is_some() {
            app.pump().unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert!(app.state.rest.sidebar.has_modal());
    assert!(
        !root
            .path()
            .join("collection.postman_collection.json")
            .exists()
    );
    app.flush_before_leave().await.unwrap();
    app.quiesce().await.unwrap();
}
