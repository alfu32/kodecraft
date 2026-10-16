use kode_core::{JobTag, RequestId, WorkspaceGeneration};
use kode_rest::{
    model::NodePath,
    runner::{Item, Outcome, Summary},
};
use kode_tui::{
    focus::FocusState,
    input::terminal::{InputEvent, Key, KeyInput, Modifiers, PointerAction, PointerInput},
    panels::rest::{
        run::RunState,
        runner::{Intent, Runner},
    },
    theme::Theme,
};
use ratatui::{buffer::Buffer, layout::Rect};
use std::time::Duration;
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    })
}
fn click(area: Rect) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x: area.x,
        y: area.y,
        action: PointerAction::Down(crossterm::event::MouseButton::Left),
        modifiers: Modifiers::default(),
    })
}
fn setup(count: usize) -> (Runner, RunState, FocusState) {
    let mut runner = Runner::new().unwrap();
    runner.visible = true;
    let outcomes: Vec<_> = (0..count)
        .map(|index| Outcome {
            item: Item {
                id: format!("runtime-{index}"),
                path: NodePath(vec![index]),
                name: format!("Item {index}"),
            },
            status: Some(200),
            error: None,
            validation_error: None,
            duration: Duration::from_millis(3),
            received_bytes: 42,
        })
        .collect();
    let run = RunState {
        summary: Some(Summary {
            outcomes: outcomes.clone(),
            cancelled: false,
        }),
        outcomes,
        ..Default::default()
    };
    let focus = FocusState {
        focused: Some(runner.id),
        ..Default::default()
    };
    (runner, run, focus)
}
fn draw(runner: &mut Runner, run: &RunState, width: u16, height: u16) -> Buffer {
    let area = Rect::new(2, 3, width, height);
    let mut buffer = Buffer::empty(area);
    runner.render(area, &mut buffer, &Theme::compiled(), run, true);
    buffer
}
#[test]
fn keyboard_follow_wheel_and_clicked_scrolled_rows_open_the_captured_runtime_identity() {
    let (mut runner, run, mut focus) = setup(40);
    draw(&mut runner, &run, 80, 12);
    assert!(runner.dispatch(&key(Key::End), &run, &mut focus));
    let buffer = draw(&mut runner, &run, 80, 12);
    assert!(runner.scroll > 0);
    assert!(
        buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains("Item 39")
    );
    runner.dispatch(&key(Key::PageUp), &run, &mut focus);
    draw(&mut runner, &run, 80, 12);
    assert!(
        runner.selected >= runner.scroll
            && runner.selected < runner.scroll + runner.list.height as usize
    );
    let selection = runner.selected;
    runner.dispatch(
        &InputEvent::Pointer(PointerInput {
            x: runner.list.x,
            y: runner.list.y,
            action: PointerAction::Scroll {
                horizontal: 0,
                vertical: 1,
            },
            modifiers: Modifiers::default(),
        }),
        &run,
        &mut focus,
    );
    draw(&mut runner, &run, 80, 12);
    assert_eq!(runner.selected, selection);
    let expected = format!("runtime-{}", runner.scroll);
    runner.dispatch(&click(runner.list), &run, &mut focus);
    assert!(matches!(runner.take_intents().as_slice(),[Intent::Open(key)] if key==&expected));
}
#[test]
fn stop_rerun_and_close_have_separate_mouse_targets_and_keep_outcome_data_immutable() {
    let (mut runner, mut run, mut focus) = setup(2);
    run.pending = Some(JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId::fresh().unwrap(),
        document: None,
    });
    draw(&mut runner, &run, 80, 12);
    runner.dispatch(&click(runner.button), &run, &mut focus);
    assert!(matches!(runner.take_intents().as_slice(), [Intent::Stop]));
    run.pending = None;
    draw(&mut runner, &run, 80, 12);
    runner.dispatch(&click(runner.button), &run, &mut focus);
    assert!(matches!(runner.take_intents().as_slice(), [Intent::Rerun]));
    runner.dispatch(&click(runner.close), &run, &mut focus);
    assert!(matches!(
        runner.take_intents().as_slice(),
        [Intent::Dismiss]
    ));
    assert_eq!(run.outcomes.len(), 2);
}
#[test]
fn tiny_and_empty_runner_views_keep_keyboard_actions_bounded() {
    let (mut runner, run, mut focus) = setup(2);
    for (width, height) in [(20, 8), (8, 4), (1, 1), (0, 0)] {
        draw(&mut runner, &run, width, height);
        runner.dispatch(&key(Key::End), &run, &mut focus);
        runner.dispatch(&key(Key::Enter), &run, &mut focus);
        assert!(matches!(runner.take_intents().as_slice(),[Intent::Open(key)] if key=="runtime-1"));
        runner.dispatch(&key(Key::Escape), &run, &mut focus);
        assert!(matches!(
            runner.take_intents().as_slice(),
            [Intent::Dismiss]
        ));
    }
}
