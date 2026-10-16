//! Bounded viewport over immutable streamed row chunks. Result navigation never
//! changes an editor selection or takes focus for a background completion.
use crate::{
    editor::display::Glyph,
    focus::{FocusId, FocusState},
    input::terminal::{InputEvent, Key, PointerAction},
    theme::Theme,
    widgets::controls::draw_text,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::event::MouseButton;
use kode_core::{RequestId, Result};
use kode_database::{
    bridge::wire::{Cell, SqlError},
    execution::{Phase, Snapshot},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};

pub struct ResultsPanel {
    pub id: FocusId,
    pub tab: usize,
    pub row: usize,
    pub column: usize,
    pub area: Rect,
    snapshot: Snapshot,
    messages: Vec<String>,
    wrapped: Vec<String>,
    wrap_width: Option<u16>,
    details_hit: Rect,
    details_requested: bool,
    widths: Vec<Vec<usize>>,
    samples: Vec<usize>,
    tabs: Vec<(Rect, usize)>,
}
/// Stop before allocating a full representation of a large cell. Terminal
/// controls and zero/wide scalars use the editor's established safe escapes.
pub fn clipped(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let glyph = Glyph::for_scalar(ch);
        let size = glyph.width();
        if used + size > width {
            if used < width {
                out.push('…');
            }
            break;
        }
        out.push_str(&glyph.text());
        used += size;
    }
    out
}
fn value(cell: &Cell, width: usize) -> String {
    match cell {
        Cell::Null => clipped("<null>", width),
        Cell::Text { text, truncated } => clipped(
            &format!(
                "{}{}",
                clipped(text, width.saturating_sub(usize::from(*truncated))),
                if *truncated { "…" } else { "" }
            ),
            width,
        ),
        Cell::Binary { base64, truncated } => {
            let length = (base64.len() / 4 * 3)
                .saturating_sub(base64.bytes().rev().take_while(|ch| *ch == b'=').count());
            let prefix = base64.len().min(width.div_ceil(2).div_ceil(3) * 4);
            let Some(bytes) = base64
                .get(..prefix)
                .and_then(|prefix| STANDARD.decode(prefix).ok())
            else {
                return clipped("<invalid binary>", width);
            };
            let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
            clipped(
                &format!(
                    "0x{hex}{} ({length} bytes{})",
                    if *truncated || bytes.len() < length {
                        "…"
                    } else {
                        ""
                    },
                    if *truncated { ", truncated" } else { "" }
                ),
                width,
            )
        }
    }
}
fn errors(out: &mut Vec<String>, label: &str, error: &SqlError) {
    out.push(format!(
        "{label}: {} [SQLSTATE {} / {}]",
        error.message,
        error.sql_state.as_deref().unwrap_or("-"),
        error.vendor_code
    ));
    for next in &error.chained {
        errors(out, "Caused by", next);
    }
}
pub fn sql_error_details(error: &SqlError) -> String {
    let mut out = vec![];
    errors(&mut out, "Error", error);
    out.join("\n")
}
impl ResultsPanel {
    pub fn details(&self) -> String {
        format!(
            "Execution: {}\n\n{}",
            self.snapshot
                .execution_id
                .as_deref()
                .unwrap_or("not started"),
            self.messages.join("\n")
        )
    }
    pub fn take_details_request(&mut self) -> bool {
        std::mem::take(&mut self.details_requested)
    }
    fn wrap_messages(&mut self) {
        if self.wrap_width != Some(self.area.width) {
            self.wrapped = self
                .messages
                .iter()
                .flat_map(|message| message.lines())
                .flat_map(|line| crate::widgets::text_panel::wrap_line(line, self.area.width))
                .collect();
            self.wrap_width = Some(self.area.width);
        }
    }

    pub fn new() -> Result<Self> {
        Ok(Self {
            id: FocusId(RequestId::fresh()?.0),
            tab: 0,
            row: 0,
            column: 0,
            area: Rect::default(),
            snapshot: Snapshot::default(),
            messages: vec![],
            wrapped: vec![],
            wrap_width: None,
            details_hit: Rect::default(),
            details_requested: false,
            widths: vec![],
            samples: vec![],
            tabs: vec![],
        })
    }
    pub fn publish(&mut self, snapshot: Snapshot) {
        let messages_selected = self.tab == self.snapshot.preview.result_sets.len() && self.tab > 0;
        self.wrap_width = None;
        self.messages.clear();
        for count in &snapshot.preview.update_counts {
            self.messages.push(format!("Update count: {count}"));
        }
        for warning in &snapshot.preview.warnings {
            errors(&mut self.messages, "Warning", warning);
        }
        if let Some(final_result) = &snapshot.completion {
            self.messages.push(format!(
                "{:?} in {} ms",
                final_result.outcome, final_result.elapsed_ms
            ));
            if let Some(error) = &final_result.error {
                errors(&mut self.messages, "Error", error);
            }
        }
        if let Some(error) = &snapshot.error {
            self.messages.push(error.to_string());
        }
        if snapshot.unknown_outcome {
            self.messages.push(
                "Outcome unknown. Check the database before retrying; no SQL was replayed.".into(),
            );
        }
        if let Some(status) = snapshot.cancel_status {
            self.messages.push(format!(
                "Cancel request: {status:?}; completion is reported separately."
            ));
        }
        if let Some(error) = &snapshot.cancel_error {
            self.messages
                .push(format!("Cancel request failed: {error}"));
        }
        if snapshot.preview.incomplete || snapshot.preview.retention_limited {
            self.messages
                .push("Incomplete preview; retained rows are only part of the results.".into());
        }
        // Column widths are bounded and computed once when each result arrives.
        while self.widths.len() < snapshot.preview.result_sets.len() {
            let set = &snapshot.preview.result_sets[self.widths.len()];
            let widths = set
                .columns
                .iter()
                .map(|column| {
                    clipped(&column.label, 30)
                        .len()
                        .max(clipped(&column.type_name, 30).len())
                        .clamp(8, 32)
                })
                .collect();
            self.widths.push(widths);
            self.samples.push(0);
        }
        for (index, set) in snapshot.preview.result_sets.iter().enumerate() {
            let sample = set.received_rows.min(100);
            if sample > self.samples[index] {
                for row in set.chunks.iter().flat_map(|chunk| chunk.iter()).take(100) {
                    for (column, cell) in row.iter().enumerate() {
                        self.widths[index][column] = self.widths[index][column]
                            .max(value(cell, 30).chars().count() + 2)
                            .min(32);
                    }
                }
                self.samples[index] = sample;
            }
        }
        self.snapshot = snapshot;
        if messages_selected {
            self.tab = self.snapshot.preview.result_sets.len();
        }
        self.tab = self.tab.min(self.snapshot.preview.result_sets.len());
        self.clamp();
    }
    pub fn reset(&mut self) {
        self.widths.clear();
        self.samples.clear();
        self.row = 0;
        self.column = 0;
        self.tab = 0;
        self.publish(Snapshot::default());
    }
    fn rows(&self) -> usize {
        self.snapshot
            .preview
            .result_sets
            .get(self.tab)
            .map_or(self.wrapped.len(), |set| set.received_rows)
    }
    fn clamp(&mut self) {
        let viewport = if self.snapshot.preview.result_sets.get(self.tab).is_some() {
            self.area.height.saturating_sub(5)
        } else {
            self.area.height.saturating_sub(2)
        }
        .max(1) as usize;
        self.row = self.row.min(self.rows().saturating_sub(viewport));
        self.column = self.column.min(
            self.snapshot
                .preview
                .result_sets
                .get(self.tab)
                .map_or(0, |set| set.columns.len().saturating_sub(1)),
        );
    }
    pub fn dispatch(&mut self, event: &InputEvent, focus: &mut FocusState) -> bool {
        if crate::widgets::controls::button(event, self.details_hit, false) {
            self.details_requested = true;
            return true;
        }
        match event {
            InputEvent::Key(input)
                if !input.modifiers.ctrl
                    && !input.modifiers.meta
                    && (input.modifiers.alt || focus.focused == Some(self.id)) =>
            {
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::SqlResults,
                    *input,
                ) else {
                    return false;
                };
                match input.key {
                    Key::Up => self.row = self.row.saturating_sub(1),
                    Key::Down => self.row = self.row.saturating_add(1),
                    Key::Left => self.column = self.column.saturating_sub(1),
                    Key::Right => self.column = self.column.saturating_add(1),
                    Key::PageUp => {
                        self.row = self
                            .row
                            .saturating_sub(self.area.height.saturating_sub(4).max(1) as usize)
                    }
                    Key::PageDown => {
                        self.row = self
                            .row
                            .saturating_add(self.area.height.saturating_sub(4).max(1) as usize)
                    }
                    Key::Home => {
                        self.row = 0;
                        self.column = 0
                    }
                    Key::End => self.row = self.rows(),
                    Key::Tab => {
                        let count = self.snapshot.preview.result_sets.len() + 1;
                        self.tab =
                            (self.tab + if input.modifiers.shift { count - 1 } else { 1 }) % count;
                        self.row = 0;
                        self.column = 0
                    }
                    _ => return false,
                }
            }
            InputEvent::Pointer(pointer)
                if self.area.contains(Position::new(pointer.x, pointer.y)) =>
            {
                match pointer.action {
                    PointerAction::Down(MouseButton::Left) => {
                        focus.focused = Some(self.id);
                        if let Some((_, tab)) = self
                            .tabs
                            .iter()
                            .find(|(rect, _)| rect.contains(Position::new(pointer.x, pointer.y)))
                        {
                            self.tab = *tab;
                            self.row = 0;
                            self.column = 0;
                        }
                    }
                    PointerAction::Scroll {
                        vertical,
                        horizontal,
                    } => {
                        self.row = self.row.saturating_add_signed(vertical as isize * 3);
                        self.column = self.column.saturating_add_signed(horizontal as isize)
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        self.clamp();
        true
    }
    pub fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, focused: bool) {
        self.area = area;
        self.wrap_messages();
        self.clamp();
        self.tabs.clear();
        self.details_hit = Rect::default();
        if area.is_empty() {
            return;
        }
        let style = theme.style("content");
        draw_text(area, buffer, "", style);
        let phase = if self.snapshot.phase == Phase::Finished {
            self.snapshot
                .completion
                .as_ref()
                .map_or("Failed".to_string(), |result| {
                    format!("{:?} / {} ms", result.outcome, result.elapsed_ms)
                })
        } else {
            format!("{:?}", self.snapshot.phase)
        };
        let summary = format!(
            "Results — {phase}{}{}",
            if self.snapshot.unknown_outcome {
                " / OUTCOME UNKNOWN"
            } else {
                ""
            },
            if self.snapshot.preview.incomplete {
                " / incomplete"
            } else {
                ""
            }
        );
        draw_text(
            Rect::new(area.x, area.y, area.width, 1),
            buffer,
            &summary,
            theme.style(if focused {
                "tab-button:focus"
            } else {
                "tab-strip"
            }),
        );
        if area.height < 2 {
            return;
        }
        let details_width = area.width.min(12);
        self.details_hit = Rect::new(area.right() - details_width, area.y + 1, details_width, 1);
        let tabs_right = self.details_hit.x;
        let mut x = area.x;
        let start = self
            .tab
            .saturating_sub((area.width as usize / 14).max(1) - 1);
        for tab in start..=self.snapshot.preview.result_sets.len() {
            if x >= tabs_right {
                break;
            }
            let label = if tab == self.snapshot.preview.result_sets.len() {
                " Messages ".into()
            } else {
                format!(" Result {} ", tab + 1)
            };
            let rect = Rect::new(x, area.y + 1, (label.len() as u16).min(tabs_right - x), 1);
            draw_text(
                rect,
                buffer,
                &label,
                theme.style(if self.tab == tab {
                    "tab-button:focus"
                } else {
                    "tab-button"
                }),
            );
            self.tabs.push((rect, tab));
            x = rect.right();
        }
        draw_text(
            self.details_hit,
            buffer,
            "[F4 Details]",
            theme.style("lsp-button"),
        );
        if area.height < 3 {
            return;
        }
        if let Some(set) = self.snapshot.preview.result_sets.get(self.tab) {
            let mut columns = vec![];
            let mut x = area.x;
            for (index, column) in set.columns.iter().enumerate().skip(self.column) {
                if x >= area.right() {
                    break;
                }
                let width = (self.widths[self.tab][index] as u16).min(area.right() - x);
                columns.push((index, x, width));
                draw_text(
                    Rect::new(x, area.y + 2, width, 1),
                    buffer,
                    &clipped(&column.label, width.saturating_sub(1) as usize),
                    theme.style("tab-strip"),
                );
                if area.height > 3 {
                    draw_text(
                        Rect::new(x, area.y + 3, width, 1),
                        buffer,
                        &clipped(&column.type_name, width.saturating_sub(1) as usize),
                        theme.style("semantic.comment"),
                    );
                }
                x = x.saturating_add(width);
            }
            let mut skip = self.row;
            let mut y = area.y + 4;
            'chunks: for chunk in &set.chunks {
                if skip >= chunk.len() {
                    skip -= chunk.len();
                    continue;
                }
                for row in chunk.iter().skip(std::mem::take(&mut skip)) {
                    if y >= area.bottom().saturating_sub(1) {
                        break 'chunks;
                    }
                    for &(index, x, width) in &columns {
                        if let Some(cell) = row.get(index) {
                            draw_text(
                                Rect::new(x, y, width, 1),
                                buffer,
                                &value(cell, width.saturating_sub(1) as usize),
                                theme.style(if matches!(cell, Cell::Null) {
                                    "semantic.comment"
                                } else {
                                    "content"
                                }),
                            );
                        }
                    }
                    y += 1;
                }
            }
            if area.height > 4 {
                draw_text(
                    Rect::new(area.x, area.bottom() - 1, area.width, 1),
                    buffer,
                    &format!(
                        "{} rows{}{} / row {} / column {}",
                        set.received_rows,
                        if set.limited { " (limited)" } else { "" },
                        if set.incomplete { " (incomplete)" } else { "" },
                        self.row + 1,
                        self.column + 1
                    ),
                    theme.style("semantic.comment"),
                );
            }
        } else {
            for (offset, message) in self
                .wrapped
                .iter()
                .skip(self.row)
                .take(area.height.saturating_sub(2) as usize)
                .enumerate()
            {
                draw_text(
                    Rect::new(area.x, area.y + 2 + offset as u16, area.width, 1),
                    buffer,
                    message,
                    style,
                );
            }
        }
    }
}
