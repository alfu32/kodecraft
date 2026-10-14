use super::layout::VisualLayout;
use crate::theme::Theme;
use kode_core::{ByteOffset, LineIndex, Result, document::TextSnapshot};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{Clear, Widget},
};

pub const DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
#[derive(Debug)]
pub struct MinimapRow {
    pub first_line: LineIndex,
    pub cells: String,
    pub highlighted: bool,
}
#[derive(Debug)]
pub struct Minimap {
    pub rows: Vec<MinimapRow>,
    pub block_offset: usize,
}
impl Minimap {
    /// Four raw lines and two UTF-16 columns per Braille cell, matching the source
    /// shape rather than its escaped editor presentation. Only visible blocks
    /// and the first width*2 columns are visited, even for a single huge line.
    pub fn prepare(
        snapshot: &TextSnapshot,
        width: usize,
        height: usize,
        caret: ByteOffset,
    ) -> Result<Self> {
        let caret_line = snapshot.byte_to_position(caret)?.line.0;
        let total = snapshot.len_lines().div_ceil(4);
        let offset = (caret_line / 4)
            .saturating_sub(height / 2)
            .min(total.saturating_sub(height));
        let mut rows = vec![];
        if width == 0 {
            return Ok(Self {
                rows,
                block_offset: offset,
            });
        }
        for block in offset..(offset + height).min(total) {
            let mut masks = vec![0u8; width];
            for (dy, dots) in DOTS.iter().enumerate() {
                let line = block * 4 + dy;
                if line >= snapshot.len_lines() {
                    break;
                }
                let mut column = 0;
                'source: for scalar in snapshot.line_content(LineIndex(line))?.chars() {
                    let mut units = [0; 2];
                    for unit in scalar.encode_utf16(&mut units) {
                        if column >= width * 2 {
                            break 'source;
                        }
                        if *unit != ' ' as u16 {
                            masks[column / 2] |= dots[column % 2];
                        }
                        column += 1;
                    }
                }
            }
            rows.push(MinimapRow {
                first_line: LineIndex(block * 4),
                cells: masks
                    .into_iter()
                    .map(|mask| char::from_u32(0x2800 + mask as u32).expect("Braille code point"))
                    .collect(),
                highlighted: (block * 4..block * 4 + 4).contains(&caret_line),
            });
        }
        Ok(Self {
            rows,
            block_offset: offset,
        })
    }
    pub fn render(&self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let area = area.intersection(buffer.area);
        Clear.render(area, buffer);
        buffer.set_style(area, theme.style("code-body"));
        if area.is_empty() {
            return;
        }
        for (index, row) in self.rows.iter().take(area.height as usize).enumerate() {
            let style = theme.style(if row.highlighted {
                "code-selection"
            } else {
                "code-body"
            });
            buffer.set_stringn(
                area.x,
                area.y + index as u16,
                &row.cells,
                area.width as usize,
                style,
            );
        }
    }
    pub fn hit(
        &self,
        area: Rect,
        x: u16,
        y: u16,
        layout: &VisualLayout,
    ) -> Result<Option<(ByteOffset, usize)>> {
        if !area.contains(Position::new(x, y)) {
            return Ok(None);
        }
        let Some(row) = self.rows.get((y - area.y) as usize) else {
            return Ok(None);
        };
        let position = layout.snapshot.line_start(row.first_line)?;
        let visual_row = layout.position_to_cell(position)?.0;
        Ok(Some((
            position,
            visual_row.min(layout.rows.len().saturating_sub(area.height as usize)),
        )))
    }
}
