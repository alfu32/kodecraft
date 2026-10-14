use ratatui::layout::{Position, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panes {
    pub first: Rect,
    pub splitter: Rect,
    pub second: Rect,
}

pub fn split(area: Rect, axis: Axis, first_size: u16) -> Panes {
    let extent = match axis {
        Axis::Horizontal => area.width,
        Axis::Vertical => area.height,
    };
    if extent < 3 {
        return Panes {
            first: area,
            splitter: Rect::default(),
            second: Rect::default(),
        };
    }
    let first_size = first_size.clamp(1, extent - 2);
    match axis {
        Axis::Horizontal => Panes {
            first: Rect::new(area.x, area.y, first_size, area.height),
            splitter: Rect::new(area.x + first_size, area.y, 1, area.height),
            second: Rect::new(
                area.x + first_size + 1,
                area.y,
                extent - first_size - 1,
                area.height,
            ),
        },
        Axis::Vertical => Panes {
            first: Rect::new(area.x, area.y, area.width, first_size),
            splitter: Rect::new(area.x, area.y + first_size, area.width, 1),
            second: Rect::new(
                area.x,
                area.y + first_size + 1,
                area.width,
                extent - first_size - 1,
            ),
        },
    }
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
pub fn local_position(area: Rect, x: u16, y: u16) -> Option<Position> {
    area.contains(Position::new(x, y))
        .then(|| Position::new(x - area.x, y - area.y))
}

#[derive(Debug)]
pub struct Splitter {
    pub axis: Axis,
    pub size: u16,
    captured: bool,
}
impl Splitter {
    pub fn new(axis: Axis, size: u16) -> Self {
        Self {
            axis,
            size,
            captured: false,
        }
    }
    pub fn handle(&mut self, event: &crate::input::terminal::InputEvent, area: Rect) -> bool {
        use crate::input::terminal::{InputEvent, PointerAction};
        use crossterm::event::MouseButton;
        let InputEvent::Pointer(pointer) = event else {
            return false;
        };
        match pointer.action {
            PointerAction::Down(MouseButton::Left)
                if split(area, self.axis, self.size)
                    .splitter
                    .contains(Position::new(pointer.x, pointer.y)) =>
            {
                self.captured = true;
                true
            }
            PointerAction::Drag(MouseButton::Left) if self.captured => {
                let (point, origin, extent) = match self.axis {
                    Axis::Horizontal => (pointer.x, area.x, area.width),
                    Axis::Vertical => (pointer.y, area.y, area.height),
                };
                self.size = point
                    .saturating_sub(origin)
                    .clamp(1, extent.saturating_sub(2).max(1));
                true
            }
            PointerAction::Up(MouseButton::Left) if self.captured => {
                self.captured = false;
                true
            }
            _ => false,
        }
    }
    pub fn captured(&self) -> bool {
        self.captured
    }
    pub fn render(
        &self,
        area: Rect,
        buffer: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) {
        let separator = split(area, self.axis, self.size)
            .splitter
            .intersection(buffer.area);
        for y in separator.y..separator.bottom() {
            for x in separator.x..separator.right() {
                buffer[(x, y)]
                    .set_symbol(if self.axis == Axis::Horizontal {
                        "│"
                    } else {
                        "─"
                    })
                    .set_style(theme.style("splitter"));
            }
        }
    }
}
