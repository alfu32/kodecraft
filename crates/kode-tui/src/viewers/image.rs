//! Image controls own interactive state; decoding and cell sampling stay in workers.
use crate::{
    editor::display::display_text,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::{Slider, draw_text},
};
use crossterm::event::MouseButton;
use kode_core::{JobTag, RequestId, Result, WorkspaceGeneration};
use kode_platform::images::{
    DecodedImage,
    cells::{CellImage, CellMode, CellOptions},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Style},
    widgets::{Clear, Widget},
};
use std::path::PathBuf;
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageSettings {
    pub width: u16,
    pub gray: f64,
    pub scatter: f64,
    pub mode: CellMode,
}
impl Default for ImageSettings {
    fn default() -> Self {
        Self {
            width: 60,
            gray: 0.2,
            scatter: 1600.0,
            mode: CellMode::Quart,
        }
    }
}
pub struct ImageJob {
    pub tag: JobTag,
    pub image: DecodedImage,
    pub options: CellOptions,
    pub full_height: u32,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct RenderKey {
    settings: ImageSettings,
    width: u16,
    rows: u16,
}
pub struct ImageView {
    pub id: FocusId,
    pub path: PathBuf,
    pub mime: String,
    pub settings: ImageSettings,
    pub error: Option<String>,
    pub cells: Option<CellImage>,
    pub pending: Option<JobTag>,
    generation: WorkspaceGeneration,
    image: DecodedImage,
    key: Option<RenderKey>,
    sliders: [Slider; 3],
    tracks: [Rect; 3],
    toggle: Rect,
    more: Rect,
    advanced: bool,
    window_focused: bool,
    area: Rect,
    work: Vec<ImageJob>,
}
impl ImageView {
    pub fn new(
        path: PathBuf,
        mime: String,
        image: DecodedImage,
        generation: WorkspaceGeneration,
        settings: ImageSettings,
    ) -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            path,
            mime,
            settings,
            error: None,
            cells: None,
            pending: None,
            generation,
            image,
            key: None,
            sliders: [
                Slider::new(8.0, 200.0, settings.width.into())?,
                Slider::new(0.0, 1.0, settings.gray)?,
                Slider::new(0.0, 8000.0, settings.scatter)?,
            ],
            tracks: [Rect::default(); 3],
            toggle: Rect::default(),
            more: Rect::default(),
            advanced: false,
            window_focused: true,
            area: Rect::default(),
            work: vec![],
        })
    }
    pub fn take_work(&mut self) -> Vec<ImageJob> {
        std::mem::take(&mut self.work)
    }
    pub fn is_dragging(&self, focus: &FocusState) -> bool {
        focus.captured == Some(self.id) && self.sliders.iter().any(Slider::captured)
    }
    pub fn detach(&mut self, focus: &mut FocusState) {
        if focus.captured == Some(self.id) {
            focus.captured = None;
        }
        self.sliders = [
            Slider::new(8.0, 200.0, self.settings.width.into()).expect("width range"),
            Slider::new(0.0, 1.0, self.settings.gray).expect("gray range"),
            Slider::new(0.0, 8000.0, self.settings.scatter).expect("contrast range"),
        ];
    }
    pub fn complete(&mut self, tag: JobTag, result: Result<CellImage>) -> bool {
        if self.pending != Some(tag) || tag.workspace != self.generation {
            return false;
        }
        self.pending = None;
        match result {
            Ok(cells) => {
                self.cells = Some(cells);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        true
    }
    fn controls(&self) -> usize {
        if self.advanced { 3 } else { 1 }
    }
    fn request(&mut self, body_rows: u16) -> Result<()> {
        let width = self.settings.width.min(self.area.width.max(8));
        let rows = body_rows.saturating_sub(self.controls() as u16);
        let key = RenderKey {
            settings: self.settings,
            width,
            rows,
        };
        if rows == 0 {
            self.pending = None;
            self.key = None;
            self.work.clear();
            return Ok(());
        }
        if self.key == Some(key) {
            return Ok(());
        }
        self.key = Some(key);
        let tag = JobTag {
            workspace: self.generation,
            request: RequestId::fresh()?,
            document: None,
        };
        self.pending = Some(tag);
        self.error = None;
        // Source height truncates aspect/2, with a minimum four cell rows.
        let height = (f64::from(self.image.height()) * f64::from(width)
            / f64::from(self.image.width())
            / 2.0) as u32;
        let height = height.max(4);
        self.work.clear();
        self.work.push(ImageJob {
            tag,
            image: self.image.clone(),
            options: CellOptions {
                width,
                height: u32::from(rows).min(height) as u16,
                mode: self.settings.mode,
                gray: self.settings.gray,
                scatter: self.settings.scatter,
            },
            full_height: height,
        });
        Ok(())
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        if let InputEvent::Focus(active) = event {
            self.window_focused = *active;
            if !active {
                self.detach(focus);
            }
            return true;
        }
        if !self.window_focused {
            return false;
        }
        if let InputEvent::Key(input) = event {
            if focus.focused != Some(self.id)
                || input.modifiers.ctrl
                || input.modifiers.alt
                || input.modifiers.meta
            {
                return false;
            }
            match input.key {
                Key::Left => self.settings.width = self.settings.width.saturating_sub(4).max(8),
                Key::Right => {
                    self.settings.width = self
                        .settings
                        .width
                        .saturating_add(4)
                        .min(self.area.width.max(8))
                        .min(200)
                }
                Key::Enter => self.settings.mode = self.settings.mode.next(),
                Key::Character('m' | 'M') => self.advanced = !self.advanced,
                _ => return false,
            }
            self.sliders[0].value = self.settings.width.into();
            return true;
        }
        if let InputEvent::Pointer(pointer) = event {
            if pointer.action == PointerAction::Down(MouseButton::Left) {
                if self.toggle.contains(Position::new(pointer.x, pointer.y)) {
                    focus.focused = Some(self.id);
                    self.settings.mode = self.settings.mode.next();
                    return true;
                }
                if self.more.contains(Position::new(pointer.x, pointer.y)) {
                    focus.focused = Some(self.id);
                    self.advanced = !self.advanced;
                    return true;
                }
            }
            for i in 0..self.controls() {
                let captured = self.sliders[i].captured();
                if self.sliders[i].handle(event, self.tracks[i], false) {
                    focus.focused = Some(self.id);
                    if self.sliders[i].captured() {
                        focus.captured = Some(self.id);
                    }
                    if captured && !self.sliders[i].captured() {
                        focus.captured = None;
                        self.settings.width = (self.sliders[0].value as u16).clamp(8, 200);
                        self.settings.gray = self.sliders[1].value;
                        self.settings.scatter = self.sliders[2].value;
                    }
                    return true;
                }
            }
        }
        false
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) -> Result<()> {
        self.area = area.intersection(buffer.area);
        let area = self.area;
        self.toggle = Rect::default();
        self.more = Rect::default();
        self.tracks = [Rect::default(); 3];
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("image-body"));
        if area.is_empty() {
            self.pending = None;
            self.key = None;
            self.work.clear();
            return Ok(());
        }
        let header = Rect::new(area.x, area.y, area.width, 1);
        let controls = "[mode] [more]";
        draw_text(
            header,
            buffer,
            &format!(
                "{} [{}] [{}]",
                display_text(&self.path.to_string_lossy()),
                display_text(&self.mime),
                self.settings.mode.label()
            ),
            theme.style("image-header"),
        );
        if area.width >= controls.len() as u16 {
            let x = area.right() - controls.len() as u16;
            self.toggle = Rect::new(x, area.y, 6, 1);
            self.more = Rect::new(x + 7, area.y, 6, 1).intersection(area);
            draw_text(
                Rect::new(x, area.y, controls.len() as u16, 1),
                buffer,
                controls,
                theme.style("button"),
            );
        }
        for (i, label) in ["Width", "Gray", "Contrast"]
            .into_iter()
            .take(self.controls())
            .enumerate()
        {
            if area.height <= i as u16 + 1 {
                continue;
            }
            let row = area.y + i as u16 + 1;
            draw_text(
                Rect::new(area.x, row, area.width.min(10), 1),
                buffer,
                label,
                theme.style("image-body"),
            );
            let track = Rect::new(
                area.x + area.width.min(10),
                row,
                area.width.saturating_sub(10),
                1,
            );
            self.tracks[i] = track;
            self.sliders[i].render(track, buffer, theme);
        }
        self.request(area.height.saturating_sub(1))?;
        let y = area.y.saturating_add(1 + self.controls() as u16);
        let body = Rect::new(
            area.x,
            y.min(area.bottom()),
            area.width,
            area.bottom().saturating_sub(y),
        );
        if let Some(error) = &self.error {
            draw_text(
                body,
                buffer,
                &display_text(error),
                theme.style("image-body"),
            );
        } else if let Some(cells) = &self.cells {
            for y in 0..body.height.min(cells.height) {
                for x in 0..body.width.min(cells.width) {
                    let cell = &cells.cells[y as usize * cells.width as usize + x as usize];
                    let [r, g, b] = cell.foreground;
                    let [br, bg, bb] = cell.background;
                    buffer[(body.x + x, body.y + y)]
                        .set_char(cell.glyph)
                        .set_style(
                            Style::default()
                                .fg(Color::Rgb(r, g, b))
                                .bg(Color::Rgb(br, bg, bb)),
                        );
                }
            }
        } else {
            draw_text(body, buffer, "Rendering image…", theme.style("image-body"));
        }
        Ok(())
    }
}
