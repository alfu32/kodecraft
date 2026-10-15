use crossterm::event::MouseButton;
use kode_core::{DomainError, WorkspaceGeneration};
use kode_platform::images::{
    DecodedImage,
    cells::{CellMode, render_cells},
};
use kode_tui::{
    focus::FocusState,
    input::terminal::*,
    theme::Theme,
    viewers::image::{ImageSettings, ImageView},
};
use ratatui::{buffer::Buffer, layout::Rect};
fn key(key: Key) -> InputEvent {
    InputEvent::Key(KeyInput {
        key,
        modifiers: Default::default(),
        repeat: false,
    })
}
fn pointer(x: u16, y: u16, action: PointerAction) -> InputEvent {
    InputEvent::Pointer(PointerInput {
        x,
        y,
        action,
        modifiers: Default::default(),
    })
}
fn view() -> ImageView {
    ImageView::new(
        "picture.png".into(),
        "image/png".into(),
        DecodedImage::new(2, 2, vec![255; 16]).unwrap(),
        WorkspaceGeneration(4),
        ImageSettings::default(),
    )
    .unwrap()
}
fn paint(view: &mut ImageView, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 40));
    view.render(area, &mut buffer, &Theme::compiled()).unwrap();
    buffer
}
#[test]
fn mode_width_and_resize_jobs_reject_old_completions_and_paint_real_colors() {
    let mut view = view();
    let mut focus = FocusState {
        focused: Some(view.id),
        ..Default::default()
    };
    let area = Rect::new(5, 4, 90, 20);
    paint(&mut view, area);
    let first = view.take_work().pop().unwrap();
    assert!(view.dispatch(&key(Key::Right), &mut focus));
    assert_eq!(view.settings.width, 64);
    paint(&mut view, area);
    let next = view.take_work().pop().unwrap();
    let old = render_cells(&first.image, first.options, first.full_height, || false).unwrap();
    assert!(!view.complete(first.tag, Ok(old)));
    let cells = render_cells(&next.image, next.options, next.full_height, || false).unwrap();
    assert!(view.complete(next.tag, Ok(cells)));
    let buffer = paint(&mut view, area);
    assert_eq!(buffer[(5, 6)].symbol(), "█");
    assert_eq!(buffer[(5, 6)].fg, ratatui::style::Color::Rgb(255, 255, 255));
    for expected in [
        CellMode::Demi,
        CellMode::Braille,
        CellMode::Bixel,
        CellMode::Quart,
    ] {
        assert!(view.dispatch(&key(Key::Enter), &mut focus));
        assert_eq!(view.settings.mode, expected);
    }
    view.dispatch(&key(Key::Left), &mut focus);
    paint(&mut view, area);
    let stale = view.take_work().pop().unwrap();
    paint(&mut view, Rect::default());
    assert!(!view.complete(stale.tag, Err(DomainError::Cancelled)));
}
#[test]
fn width_gray_contrast_drag_commit_on_release_and_focus_loss_cancels_capture() {
    let mut view = view();
    let mut focus = FocusState {
        focused: Some(view.id),
        ..Default::default()
    };
    let area = Rect::new(5, 4, 90, 20);
    paint(&mut view, area);
    view.take_work();
    view.dispatch(&key(Key::Character('m')), &mut focus);
    paint(&mut view, area);
    view.take_work();
    view.dispatch(
        &pointer(30, 6, PointerAction::Down(MouseButton::Left)),
        &mut focus,
    );
    view.dispatch(
        &pointer(119, 6, PointerAction::Drag(MouseButton::Left)),
        &mut focus,
    );
    assert!(view.is_dragging(&focus));
    assert_eq!(view.settings.gray, 0.2);
    paint(&mut view, area);
    assert!(view.take_work().is_empty());
    assert!(view.dispatch(
        &pointer(0, 0, PointerAction::Up(MouseButton::Left)),
        &mut focus
    ));
    assert_eq!(view.settings.gray, 1.0);
    assert!(focus.captured.is_none());
    paint(&mut view, area);
    assert_eq!(view.take_work().len(), 1);
    view.dispatch(
        &pointer(15, 7, PointerAction::Down(MouseButton::Left)),
        &mut focus,
    );
    view.dispatch(
        &pointer(15, 7, PointerAction::Up(MouseButton::Left)),
        &mut focus,
    );
    assert_eq!(view.settings.scatter, 0.0);
    view.dispatch(
        &pointer(94, 5, PointerAction::Down(MouseButton::Left)),
        &mut focus,
    );
    assert!(view.is_dragging(&focus));
    view.dispatch(&InputEvent::Focus(false), &mut focus);
    assert!(!view.is_dragging(&focus));
    assert!(!view.dispatch(&key(Key::Right), &mut focus));
    assert_eq!(view.settings.width, 60);
}
#[test]
fn tiny_surfaces_error_feedback_and_foreign_generation_stay_clipped() {
    for width in [0, 1, 5, 12, 13, 30] {
        for height in [0, 1, 2, 3, 8] {
            let mut view = view();
            let area = Rect::new(2, 3, width, height);
            let buffer = paint(&mut view, area);
            assert_eq!(buffer[(0, 0)].symbol(), " ");
            if let Some(job) = view.take_work().pop() {
                let mut foreign = job.tag;
                foreign.workspace = WorkspaceGeneration(99);
                assert!(!view.complete(foreign, Err(DomainError::Cancelled)));
                assert!(view.complete(
                    job.tag,
                    Err(DomainError::Unsupported("invalid image data".into()))
                ));
                paint(&mut view, area);
                assert!(view.error.as_ref().unwrap().contains("invalid image data"));
            }
        }
    }
}
