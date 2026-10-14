use crate::{
    editor::{
        display::display_text,
        input::{EditorState, HookContext},
    },
    input::terminal::*,
    theme::Theme,
};
use crossterm::event::MouseButton;
use kode_core::{ByteOffset, intelligence::NavigationTarget};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Modifier,
    widgets::{Clear, Widget},
};

#[derive(Debug, Clone)]
pub struct UsageEntry {
    pub target: NavigationTarget,
    pub label: String,
}
#[derive(Debug, Clone, Copy)]
pub struct UsageGeometry {
    pub area: Rect,
    pub rows: Rect,
    pub track: Rect,
}
#[derive(Debug)]
pub enum UsageAction {
    Consumed,
    Navigate(NavigationTarget),
    Dismissed,
}
pub struct UsagePopup {
    pub entries: Vec<UsageEntry>,
    pub anchor: ByteOffset,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub offset: usize,
    drag: Option<(u16, usize)>,
}
impl UsagePopup {
    pub fn new(anchor: ByteOffset, entries: Vec<UsageEntry>) -> Self {
        Self {
            anchor,
            entries,
            selected: 0,
            hovered: None,
            offset: 0,
            drag: None,
        }
    }
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
    pub fn geometry(&self, context: HookContext<'_>, state: &EditorState) -> Option<UsageGeometry> {
        let body = context.geometry.text;
        if self.entries.is_empty() || body.width == 0 || body.height < 3 {
            return None;
        }
        let (row, column) = context.layout.position_to_cell(self.anchor).ok()?;
        let height = (self.entries.len() + 2).min(10).min(body.height as usize) as u16;
        let scrollbar = self.entries.len() > (height - 2) as usize;
        let width = (self
            .entries
            .iter()
            .map(|entry| display_text(&entry.label).chars().count())
            .max()
            .unwrap_or(0)
            + 2
            + usize::from(scrollbar))
        .min(body.width as usize) as u16;
        let x = (body.x + column.min(body.width as usize - 1) as u16).min(body.right() - width);
        let screen_y = body.y
            + row
                .saturating_sub(state.scroll_rows)
                .min(body.height as usize - 1) as u16;
        let y = if screen_y + height <= body.bottom() {
            screen_y
        } else {
            screen_y.saturating_sub(height).max(body.y)
        }
        .min(body.bottom() - height);
        let area = Rect::new(x, y, width, height);
        let track_width = u16::from(scrollbar).min(width);
        Some(UsageGeometry {
            area,
            rows: Rect::new(x, y + 1, width, height - 2),
            track: Rect::new(area.right() - track_width, y + 1, track_width, height - 2),
        })
    }
    fn clamp(&mut self, visible: usize, follow: bool) {
        if self.entries.is_empty() {
            self.offset = 0;
            self.selected = 0;
            return;
        }
        let visible = visible.max(1);
        let max = self.entries.len().saturating_sub(visible);
        self.offset = self.offset.min(max);
        self.selected = self.selected.min(self.entries.len() - 1);
        if follow {
            if self.selected < self.offset {
                self.offset = self.selected;
            } else if self.selected >= self.offset + visible {
                self.offset = (self.selected - visible + 1).min(max);
            }
        } else {
            self.selected = self.selected.clamp(
                self.offset,
                (self.offset + visible - 1).min(self.entries.len() - 1),
            );
        }
    }
    fn link_bounds(&self, geometry: UsageGeometry, index: usize) -> Rect {
        let selected = self.selected == index;
        let width = geometry
            .area
            .width
            .saturating_sub(if geometry.track.is_empty() { 2 } else { 3 })
            .saturating_sub(u16::from(selected));
        Rect::new(
            geometry.area.x + (if selected { 2 } else { 1 }).min(geometry.area.width),
            geometry.rows.y + (index - self.offset) as u16,
            width.min(
                display_text(&self.entries[index].label)
                    .chars()
                    .count()
                    .min(u16::MAX as usize) as u16,
            ),
            1,
        )
    }
    pub fn render(
        &mut self,
        context: HookContext<'_>,
        state: &EditorState,
        buffer: &mut Buffer,
        theme: &Theme,
    ) {
        let Some(geometry) = self.geometry(context, state) else {
            return;
        };
        self.clamp(geometry.rows.height as usize, true);
        let area = geometry.area.intersection(buffer.area);
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-search-bar"));
        if !area.is_empty() {
            buffer.set_stringn(
                area.x,
                area.y,
                "usage sites",
                area.width as usize,
                theme.style("definition-title"),
            );
        }
        for index in
            self.offset..(self.offset + geometry.rows.height as usize).min(self.entries.len())
        {
            let rect = self.link_bounds(geometry, index).intersection(buffer.area);
            if !rect.is_empty() {
                let mut style = theme.style("code-search-bar");
                if self.hovered == Some(index) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                buffer.set_stringn(
                    rect.x,
                    rect.y,
                    display_text(&self.entries[index].label),
                    rect.width as usize,
                    style,
                );
            }
            let marker = Position::new(
                geometry.area.x + 1,
                geometry.rows.y + (index - self.offset) as u16,
            );
            if self.selected == index && geometry.area.width > 1 && buffer.area.contains(marker) {
                buffer[marker].set_symbol(">");
            }
        }
        if !geometry.track.is_empty() {
            let indicator = self.offset * (geometry.track.height as usize - 1)
                / self
                    .entries
                    .len()
                    .saturating_sub(geometry.rows.height as usize)
                    .max(1);
            for row in 0..geometry.track.height {
                let point = Position::new(geometry.track.x, geometry.track.y + row);
                if buffer.area.contains(point) {
                    buffer[point]
                        .set_symbol(if row as usize == indicator {
                            "█"
                        } else {
                            "│"
                        })
                        .set_style(theme.style("code-popup-scrollbar"));
                }
            }
        }
    }
    fn selected_action(&self) -> UsageAction {
        self.entries
            .get(self.selected)
            .filter(|entry| {
                !entry.target.path.as_os_str().is_empty() || entry.target.document.is_some()
            })
            .map_or(UsageAction::Consumed, |entry| {
                UsageAction::Navigate(entry.target.clone())
            })
    }
    pub fn dispatch(
        &mut self,
        event: &InputEvent,
        context: HookContext<'_>,
        state: &EditorState,
    ) -> UsageAction {
        let geometry = self.geometry(context, state);
        let visible = geometry.map_or(8, |geometry| geometry.rows.height as usize);
        self.clamp(visible, false);
        match event {
            InputEvent::Key(input) => {
                self.hovered = None;
                match input.key {
                    Key::Escape => return UsageAction::Dismissed,
                    Key::Up => self.selected = self.selected.saturating_sub(1),
                    Key::Down => {
                        self.selected =
                            (self.selected + 1).min(self.entries.len().saturating_sub(1))
                    }
                    Key::PageUp => self.selected = self.selected.saturating_sub(visible),
                    Key::PageDown => {
                        self.selected =
                            (self.selected + visible).min(self.entries.len().saturating_sub(1))
                    }
                    Key::Home => self.selected = 0,
                    Key::End => self.selected = self.entries.len().saturating_sub(1),
                    Key::Enter => return self.selected_action(),
                    _ => {}
                }
                self.clamp(visible, true);
            }
            InputEvent::Pointer(pointer) => {
                let point = Position::new(pointer.x, pointer.y);
                match pointer.action {
                    PointerAction::Scroll { vertical, .. } => {
                        self.offset = self
                            .offset
                            .saturating_add_signed(vertical as isize)
                            .min(self.entries.len().saturating_sub(visible));
                        self.hovered = None;
                        self.clamp(visible, false);
                    }
                    PointerAction::Up(MouseButton::Left) => self.drag = None,
                    PointerAction::Drag(MouseButton::Left) | PointerAction::Move
                        if self.drag.is_some() =>
                    {
                        let (start, offset) = self.drag.expect("drag checked");
                        let max = self.entries.len().saturating_sub(visible);
                        let delta = (pointer.y as i128 - start as i128) * max as i128
                            / (visible.saturating_sub(1).max(1)) as i128;
                        self.offset = (offset as i128 + delta).clamp(0, max as i128) as usize;
                        self.hovered = None;
                        self.clamp(visible, false);
                    }
                    PointerAction::Down(_) => {
                        let Some(geometry) = geometry else {
                            return UsageAction::Dismissed;
                        };
                        if !geometry.area.contains(point) {
                            return UsageAction::Dismissed;
                        }
                        if pointer.action == PointerAction::Down(MouseButton::Left)
                            && geometry.track.contains(point)
                        {
                            self.offset = (pointer.y - geometry.track.y) as usize
                                * self.entries.len().saturating_sub(visible)
                                / visible.saturating_sub(1).max(1);
                            self.clamp(visible, false);
                            self.hovered = None;
                            self.drag = Some((pointer.y, self.offset));
                        } else if pointer.action == PointerAction::Down(MouseButton::Left)
                            && geometry.rows.contains(point)
                        {
                            self.selected = self.offset + (pointer.y - geometry.rows.y) as usize;
                            return self.selected_action();
                        }
                    }
                    PointerAction::Move => {
                        self.hovered = geometry.and_then(|geometry| {
                            if !geometry.rows.contains(point) {
                                return None;
                            }
                            let index = self.offset + (pointer.y - geometry.rows.y) as usize;
                            self.entries
                                .get(index)
                                .filter(|entry| !entry.target.path.as_os_str().is_empty())?;
                            self.link_bounds(geometry, index)
                                .contains(point)
                                .then_some(index)
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        UsageAction::Consumed
    }
}

pub fn compact_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.into();
    }
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() < 2 {
        return path
            .chars()
            .take(max.saturating_sub(1).max(1))
            .collect::<String>()
            + "…";
    }
    let (folders, name) = parts.split_at(parts.len() - 1);
    let name = name[0];
    let mut compact = vec![folders[0].to_string()];
    compact.extend(
        folders
            .iter()
            .skip(1)
            .take(folders.len().saturating_sub(2))
            .map(|folder| folder.chars().next().expect("nonempty folder").to_string()),
    );
    if folders.len() > 1 {
        compact.push(folders.last().expect("folder").to_string());
    }
    compact.push(name.into());
    let compact = compact.join("/");
    if compact.chars().count() <= max {
        return compact;
    }
    let anchor = if folders.len() == 1 {
        format!("{}/", folders[0])
    } else {
        format!("{}/…/{}/", folders[0], folders.last().expect("folder"))
    };
    let available = max.saturating_sub(anchor.chars().count());
    if available > 1 {
        anchor + &name.chars().take(available - 1).collect::<String>() + "…"
    } else {
        path.chars()
            .take(max.saturating_sub(1).max(1))
            .collect::<String>()
            + "…"
    }
}
