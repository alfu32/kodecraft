//! Theme/clipping adapters for Ratatui's standard lists and tables.
use crate::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{List, ListState, StatefulWidget, Table, TableState},
};

pub fn list(
    widget: List<'_>,
    state: &mut ListState,
    area: Rect,
    buffer: &mut Buffer,
    theme: &Theme,
) {
    let area = area.intersection(buffer.area);
    widget
        .style(theme.style("content"))
        .highlight_style(theme.style("file-entry::selection:focus"))
        .render(area, buffer, state);
}
pub fn table(
    widget: Table<'_>,
    state: &mut TableState,
    area: Rect,
    buffer: &mut Buffer,
    theme: &Theme,
) {
    let area = area.intersection(buffer.area);
    widget
        .style(theme.style("content"))
        .row_highlight_style(theme.style("file-entry::selection:focus"))
        .render(area, buffer, state);
}
