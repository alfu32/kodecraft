use crate::{
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
};
use crossterm::event::MouseButton;
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
};

pub fn draw_text(area: Rect, buffer: &mut Buffer, text: &str, style: Style) {
    let area = area.intersection(buffer.area);
    if !area.is_empty() {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)].reset();
            }
        }
        buffer.set_style(area, style);
        buffer.set_stringn(area.x, area.y, text, area.width as usize, style);
    }
}
pub fn button(event: &InputEvent, area: Rect, focused: bool) -> bool {
    match event {
        InputEvent::Key(input) => {
            focused
                && matches!(input.key, Key::Enter | Key::Character(' '))
                && !input.modifiers.ctrl
                && !input.modifiers.alt
        }
        InputEvent::Pointer(pointer) => {
            pointer.action == PointerAction::Down(MouseButton::Left)
                && area.contains(Position::new(pointer.x, pointer.y))
        }
        _ => false,
    }
}

#[derive(Debug, Default, Clone)]
pub struct Scrollbar {
    pub offset: usize,
    pub viewport: usize,
    pub total: usize,
    drag_anchor: Option<i64>,
}
impl Scrollbar {
    pub fn new(total: usize, viewport: usize) -> Self {
        Self {
            total,
            viewport,
            ..Self::default()
        }
    }
    pub fn clamp(&mut self) {
        self.offset = self.offset.min(self.total.saturating_sub(self.viewport));
    }
    pub fn thumb(&self, track: usize) -> (usize, usize) {
        if track == 0 {
            return (0, 0);
        }
        let length = ((self.viewport.min(self.total) as u128 * track as u128)
            / self.total.max(1) as u128)
            .max(1)
            .min(track as u128) as usize;
        let max_offset = self.total.saturating_sub(self.viewport);
        let start = (self.offset.min(max_offset) as u128 * (track - length) as u128
            / max_offset.max(1) as u128) as usize;
        (start, length)
    }
    fn position(&mut self, area: Rect, y: i64) {
        let (_, length) = self.thumb(area.height as usize);
        let max_start = (area.height as usize).saturating_sub(length);
        let start = (y - area.y as i64).clamp(0, max_start as i64) as usize;
        self.offset = (start as u128 * self.total.saturating_sub(self.viewport) as u128
            / max_start.max(1) as u128) as usize;
        self.clamp();
    }
    pub fn handle(&mut self, event: &InputEvent, area: Rect) -> bool {
        let InputEvent::Pointer(pointer) = event else {
            return false;
        };
        match pointer.action {
            PointerAction::Down(MouseButton::Left)
                if !area.is_empty() && area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                let (start, length) = self.thumb(area.height as usize);
                let local = (pointer.y - area.y) as usize;
                if local < start || local >= start + length {
                    self.position(area, pointer.y as i64 - length as i64 / 2);
                }
                let (start, _) = self.thumb(area.height as usize);
                self.drag_anchor = Some(pointer.y as i64 - area.y as i64 - start as i64);
                true
            }
            PointerAction::Drag(MouseButton::Left) if self.drag_anchor.is_some() => {
                self.position(area, pointer.y as i64 - self.drag_anchor.unwrap());
                true
            }
            PointerAction::Up(MouseButton::Left) if self.drag_anchor.is_some() => {
                self.drag_anchor = None;
                true
            }
            PointerAction::Scroll { vertical, .. }
                if area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                self.offset = self.offset.saturating_add_signed(vertical as isize * 3);
                self.clamp();
                true
            }
            _ => false,
        }
    }
    pub fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        let (start, length) = self.thumb(area.height as usize);
        if area.is_empty() {
            return;
        }
        for row in 0..area.height {
            buffer[(area.x, area.y + row)]
                .set_symbol(
                    if (row as usize) >= start && (row as usize) < start + length {
                        "█"
                    } else {
                        "│"
                    },
                )
                .set_style(theme.style("scrollbar"));
        }
    }
    pub fn captured(&self) -> bool {
        self.drag_anchor.is_some()
    }
}

#[derive(Debug)]
pub struct Slider {
    pub min: f64,
    pub max: f64,
    pub value: f64,
    captured: bool,
}
impl Slider {
    pub fn new(min: f64, max: f64, value: f64) -> kode_core::Result<Self> {
        if !min.is_finite() || !max.is_finite() || !value.is_finite() || min > max {
            return Err(kode_core::DomainError::Unsupported(
                "invalid slider range".into(),
            ));
        }
        Ok(Self {
            min,
            max,
            value: value.clamp(min, max),
            captured: false,
        })
    }
    fn point(&mut self, area: Rect, x: u16) {
        let point = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
        self.value = self.min
            + (self.max - self.min) * point as f64 / area.width.saturating_sub(1).max(1) as f64;
    }
    pub fn handle(&mut self, event: &InputEvent, area: Rect, focused: bool) -> bool {
        match event {
            InputEvent::Pointer(pointer) => match pointer.action {
                PointerAction::Down(MouseButton::Left)
                    if !area.is_empty() && area.contains(Position::new(pointer.x, pointer.y)) =>
                {
                    self.captured = true;
                    self.point(area, pointer.x);
                    true
                }
                PointerAction::Drag(MouseButton::Left) if self.captured => {
                    self.point(area, pointer.x);
                    true
                }
                PointerAction::Up(MouseButton::Left) if self.captured => {
                    self.captured = false;
                    true
                }
                _ => false,
            },
            InputEvent::Key(input) if focused && matches!(input.key, Key::Left | Key::Right) => {
                let sign = if input.key == Key::Left { -1.0 } else { 1.0 };
                self.value =
                    (self.value + sign * (self.max - self.min) / 100.0).clamp(self.min, self.max);
                true
            }
            _ => false,
        }
    }
    pub fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        let ratio = if self.max == self.min {
            0.0
        } else {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        };
        let selected = (ratio * area.width.saturating_sub(1) as f64) as u16;
        for col in 0..area.width {
            buffer[(area.x + col, area.y)]
                .set_symbol(if col == selected { "█" } else { "─" })
                .set_style(theme.style(if col == selected {
                    "slider-indicator"
                } else {
                    "slider-track"
                }));
        }
    }
    pub fn captured(&self) -> bool {
        self.captured
    }
}
