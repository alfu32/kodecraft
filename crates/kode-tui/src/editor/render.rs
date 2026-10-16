use super::{display::Glyph, layout::VisualLayout};
use crate::theme::Theme;
use kode_core::document::Document;
use kode_core::{ByteOffset, DocumentOrigin, LineIndex, Selection, TextRange};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    widgets::{Clear, Widget},
};

#[derive(Debug, Clone, Copy)]
pub struct EditorGeometry {
    pub header: Rect,
    pub search: Rect,
    pub gutter: Rect,
    pub text: Rect,
    pub minimap: Rect,
}
impl EditorGeometry {
    pub fn new(area: Rect, lines: usize, search_height: u16) -> Self {
        let preview = if area.width < 40 {
            0
        } else {
            (area.width / 3).clamp(8, 20)
        };
        let main = area.width - preview;
        let header_height = area.height.min(1);
        let search_height = search_height.min(area.height - header_height);
        let body_y = area.y + header_height + search_height;
        let body_height = area.height - header_height - search_height;
        let gutter_width = (lines.max(1).to_string().len() as u16 + 2)
            .min(12)
            .min(main);
        Self {
            header: Rect::new(area.x, area.y, area.width, header_height),
            search: Rect::new(area.x, area.y + header_height, main, search_height),
            gutter: Rect::new(area.x, body_y, gutter_width, body_height),
            text: Rect::new(
                area.x + gutter_width,
                body_y,
                main - gutter_width,
                body_height,
            ),
            minimap: Rect::new(area.x + main, body_y, preview, body_height),
        }
    }
    pub fn hit_test(&self, layout: &VisualLayout, scroll: usize, x: u16, y: u16) -> EditorHit {
        let point = Position::new(x, y);
        if self.header.contains(point) {
            return EditorHit::Header;
        }
        if self.search.contains(point) {
            return EditorHit::Search;
        }
        if self.minimap.contains(point) {
            return EditorHit::Minimap {
                row: y - self.minimap.y,
            };
        }
        if self.gutter.contains(point) {
            let row = (scroll + (y - self.gutter.y) as usize).min(layout.rows.len() - 1);
            return EditorHit::Gutter {
                line: layout.rows[row].line,
            };
        }
        if self.text.contains(point) {
            let row = (scroll + (y - self.text.y) as usize).min(layout.rows.len() - 1);
            if layout.rows[row]
                .summary
                .as_ref()
                .is_some_and(|summary| summary.marker.contains(&((x - self.text.x) as usize)))
            {
                return EditorHit::FoldToggle {
                    line: layout.rows[row].line,
                };
            }
            return EditorHit::Text(layout.cell_to_position(
                scroll + (y - self.text.y) as usize,
                (x - self.text.x) as usize,
            ));
        }
        EditorHit::Outside
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorHit {
    Header,
    Search,
    Gutter { line: LineIndex },
    FoldToggle { line: LineIndex },
    Text(ByteOffset),
    Minimap { row: u16 },
    Outside,
}

pub struct EditorHeader {
    pub label: String,
    pub mime: Option<String>,
    pub language: Option<String>,
    pub grammar: String,
    pub dirty: bool,
}
impl EditorHeader {
    pub fn for_document(document: &Document, mime: Option<String>, grammar: String) -> Self {
        let label = match document.origin() {
            DocumentOrigin::File(path) => path.display().to_string(),
            DocumentOrigin::Virtual { label, .. } => label.clone(),
        };
        Self {
            label,
            mime,
            language: document.snapshot().language,
            grammar,
            dirty: document.is_dirty(),
        }
    }
    pub fn text(&self) -> String {
        format!(
            "{}{}{} [{}] {}",
            if self.dirty { "*" } else { " " },
            if self.grammar.is_empty() {
                String::new()
            } else {
                format!("[{}] ", self.grammar)
            },
            self.label,
            self.mime.as_deref().unwrap_or("unknown"),
            self.language
                .as_ref()
                .map_or(String::new(), |language| format!("· {language}")),
        )
    }
}
#[derive(Debug, Default, Clone)]
pub struct RenderState {
    pub selection: Selection,
    pub secondary: Vec<Selection>,
    pub scroll_rows: usize,
    pub focused: bool,
}
pub struct OverlaySpan {
    pub range: TextRange,
    pub style: Style,
    pub priority: i32,
}
pub struct RenderOptions<'a> {
    pub geometry: EditorGeometry,
    pub header: &'a EditorHeader,
    pub state: &'a RenderState,
    pub overlays: &'a [OverlaySpan],
    pub warning_lines: &'a [LineIndex],
    pub fold_starts: &'a [LineIndex],
}

/// Draw only prepared, visible state. No I/O, parsing, service calls or raw terminal writes.
pub fn render_editor(
    layout: &VisualLayout,
    buffer: &mut Buffer,
    theme: &Theme,
    options: RenderOptions<'_>,
) -> Option<Position> {
    let geometry = options.geometry;
    let header = geometry.header.intersection(buffer.area);
    if !header.is_empty() {
        Clear.render(header, buffer);
        buffer.set_style(header, theme.style("code-header"));
        buffer.set_stringn(
            header.x,
            header.y,
            super::display::display_text(&options.header.text()),
            header.width as usize,
            theme.style("code-header"),
        );
    }
    let text_area = geometry.text.intersection(buffer.area);
    let gutter = geometry.gutter.intersection(buffer.area);
    Clear.render(text_area, buffer);
    Clear.render(gutter, buffer);
    buffer.set_style(text_area, theme.style("code-body"));
    buffer.set_style(gutter, theme.style("code-gutter"));
    let preview_search = Rect::new(
        geometry.minimap.x,
        geometry.search.y,
        geometry.minimap.width,
        geometry.search.height,
    )
    .intersection(buffer.area);
    Clear.render(preview_search, buffer);
    buffer.set_style(preview_search, theme.style("code-body"));
    if text_area.height == 0 {
        return None;
    }
    let all =
        std::iter::once(options.state.selection).chain(options.state.secondary.iter().copied());
    let mut ranges: Vec<_> = all
        .clone()
        .map(|selection| selection.range())
        .filter(|range| !range.is_empty())
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut selected: Vec<TextRange> = vec![];
    for range in ranges {
        if let Some(last) = selected.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            selected.push(range);
        }
    }
    let is_selected = |range: TextRange| {
        let index = selected.partition_point(|selected| selected.end <= range.start);
        selected
            .get(index)
            .is_some_and(|selected| selected.start < range.end)
    };
    let mut heads: Vec<_> = all.map(|selection| selection.head).collect();
    heads.sort_unstable();
    heads.dedup();
    let visible: Vec<_> = layout
        .visible_rows(options.state.scroll_rows, text_area.height as usize)
        .collect();
    let mut overlays: Vec<_> = options
        .overlays
        .iter()
        .filter(|span| {
            visible.iter().any(|(_, row)| {
                span.range.start < row.source.end && row.source.start < span.range.end
            })
        })
        .map(|span| OverlaySpan {
            range: span.range,
            style: span.style,
            priority: span.priority,
        })
        .collect();
    for line in options.warning_lines {
        if let Some(line) = layout.lines.get(line.0) {
            overlays.push(OverlaySpan {
                range: line.source,
                priority: 25,
                style: Style::default().bg(theme
                    .style("code-search-active")
                    .bg
                    .unwrap_or(ratatui::style::Color::Reset)),
            });
        }
    }
    let overlays = super::overlays::compose_styles(&overlays);
    let source_style = |range: TextRange| {
        let mut style = theme.style("code-body");
        let first = overlays.partition_point(|span| span.range.end <= range.start);
        for overlay in overlays[first..]
            .iter()
            .take_while(|span| span.range.start < range.end)
        {
            style = style.patch(overlay.style);
        }
        style
    };
    for (relative, (_, row)) in visible.iter().enumerate() {
        let y = text_area.y + relative as u16;
        let line = &layout.lines[row.line.0];
        if !gutter.is_empty() {
            let digits = gutter.width.saturating_sub(2).max(1) as usize;
            let number = if row.source.start == line.source.start {
                format!("{:>digits$}", row.line.0 + 1)
            } else {
                " ".repeat(digits)
            };
            buffer.set_stringn(
                gutter.x,
                y,
                format!("{number} "),
                gutter.width as usize,
                theme.style("code-gutter"),
            );
            if row.fold_end.is_some() || options.fold_starts.contains(&row.line) {
                buffer[(gutter.right() - 1, y)]
                    .set_symbol(if row.fold_end.is_some() { "+" } else { "-" })
                    .set_style(theme.style("code-fold-marker"));
            }
        }
        if text_area.width == 0 {
            continue;
        }
        if let Some(summary) = &row.summary {
            for (column, cell) in summary
                .cells
                .iter()
                .take(text_area.width as usize)
                .enumerate()
            {
                let mut style = theme.style(if cell.source.is_none() {
                    "code-fold-marker"
                } else {
                    "code-body"
                });
                if let Some(source) = cell.source {
                    style = source_style(source);
                    let source_line = layout
                        .lines
                        .partition_point(|line| line.source.start <= source.start)
                        .saturating_sub(1);
                    let source_line = &layout.lines[source_line];
                    let run = source_line
                        .runs
                        .partition_point(|run| run.source.end <= source.start);
                    if source_line
                        .runs
                        .get(run)
                        .is_some_and(|run| run.glyph == Glyph::Tab)
                    {
                        style = style.patch(theme.style("code-tab"));
                    }
                    if is_selected(source) {
                        style = style.patch(theme.style("code-selection"));
                    }
                    if options.state.focused && heads.binary_search(&source.start).is_ok() {
                        style = style.patch(theme.style("code-cursor"));
                    }
                } else if is_selected(TextRange {
                    start: line.source.end,
                    end: layout.lines[row.fold_end.expect("summary has closing line").0]
                        .source
                        .start,
                }) {
                    style = style.patch(theme.style("code-selection"));
                }
                buffer[(text_area.x + column as u16, y)]
                    .set_symbol(&cell.symbol.to_string())
                    .set_style(style);
            }
            continue;
        }
        let mut column = 0;
        for run in &line.runs[row.run_start..row.run_end] {
            if column >= text_area.width as usize {
                break;
            }
            let mut style = source_style(run.source);
            if run.glyph == Glyph::Tab {
                style = style.patch(theme.style("code-tab"));
            }
            if is_selected(run.source) {
                style = style.patch(theme.style("code-selection"));
            }
            if options.state.focused && heads.binary_search(&run.source.start).is_ok() {
                style = style.patch(theme.style("code-cursor"));
            }
            let remaining = text_area.width as usize - column;
            buffer.set_stringn(
                text_area.x + column as u16,
                y,
                run.glyph.text(),
                remaining,
                style,
            );
            column += run.glyph.width();
        }
    }
    if !options.state.focused || text_area.is_empty() {
        return None;
    }
    // Include empty lines, line endings and exact wrap/EOF edges for secondary
    // carets as well. Only the primary supplies the real terminal cursor.
    let first = visible.first()?.1.source.start;
    let last = visible.last()?.1.source.end;
    let low = heads.partition_point(|head| *head < first);
    for head in heads[low..].iter().take_while(|head| **head <= last) {
        let Ok((row, column)) = layout.position_to_cell(*head) else {
            continue;
        };
        if row >= options.state.scroll_rows
            && row < options.state.scroll_rows + text_area.height as usize
        {
            let position = Position::new(
                text_area.x + column.min(text_area.width as usize - 1) as u16,
                text_area.y + (row - options.state.scroll_rows) as u16,
            );
            buffer[position].set_style(theme.style("code-cursor"));
        }
    }
    let (row, column) = layout.position_to_cell(options.state.selection.head).ok()?;
    if row < options.state.scroll_rows
        || row >= options.state.scroll_rows + text_area.height as usize
    {
        return None;
    }
    let position = Position::new(
        text_area.x + column.min(text_area.width as usize - 1) as u16,
        text_area.y + (row - options.state.scroll_rows) as u16,
    );
    // At an exact wrap/EOF edge preserve the last source glyph rather than
    // replacing punctuation with a space merely to represent the caret.
    buffer[position].set_style(theme.style("code-cursor"));
    Some(position)
}
