use super::display::Glyph;
use kode_core::document::TextSnapshot;
use kode_core::{ByteOffset, DocumentId, DomainError, LineIndex, Result, Revision, TextRange};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollapsedFold {
    pub start: LineIndex,
    pub end: LineIndex,
}
#[derive(Debug)]
pub struct GlyphRun {
    pub source: TextRange,
    pub glyph: Glyph,
    pub visual_start: usize,
}
#[derive(Debug)]
pub struct LineLayout {
    pub source: TextRange,
    pub runs: Vec<GlyphRun>,
    pub visual_width: usize,
    pub first_row: usize,
    pub row_count: usize,
}
#[derive(Debug)]
pub struct VisualRow {
    pub line: LineIndex,
    pub source: TextRange,
    pub run_start: usize,
    pub run_end: usize,
    pub visual_start: usize,
    pub visual_width: usize,
    pub fold_end: Option<LineIndex>,
    pub summary: Option<FoldSummary>,
}
#[derive(Debug)]
pub struct SummaryCell {
    pub symbol: char,
    pub source: Option<TextRange>,
    pub position: ByteOffset,
}
#[derive(Debug)]
pub struct FoldSummary {
    pub cells: Vec<SummaryCell>,
    pub marker: std::ops::Range<usize>,
}
fn display_cells(line: &LineLayout, start: usize, end: usize) -> Vec<SummaryCell> {
    let mut cells = vec![];
    let first = line
        .runs
        .partition_point(|run| run.visual_start + run.glyph.width() <= start);
    for run in &line.runs[first..] {
        if run.visual_start >= end {
            break;
        }
        for (index, symbol) in run.glyph.text().chars().enumerate() {
            if (start..end).contains(&(run.visual_start + index)) {
                cells.push(SummaryCell {
                    symbol,
                    source: Some(run.source),
                    position: if index == 0 {
                        run.source.start
                    } else {
                        run.source.end
                    },
                });
            }
        }
    }
    cells
}
fn fold_summary(first: &LineLayout, last: &LineLayout, width: usize) -> FoldSummary {
    let tail_budget = last.visual_width.min((width / 3).max(1));
    let head_budget = width.saturating_sub(5 + tail_budget).max(1);
    let mut head = display_cells(first, 0, head_budget.min(width));
    let marker = head.len()..(head.len() + 5).min(width);
    head.extend(" ... ".chars().map(|symbol| SummaryCell {
        symbol,
        source: None,
        position: first.source.end,
    }));
    head.extend(display_cells(
        last,
        last.visual_width.saturating_sub(tail_budget),
        last.visual_width,
    ));
    head.truncate(width);
    FoldSummary {
        cells: head,
        marker,
    }
}
#[derive(Debug)]
pub struct VisualLayout {
    pub snapshot: TextSnapshot,
    pub width: usize,
    pub lines: Vec<LineLayout>,
    pub rows: Vec<VisualRow>,
}

impl VisualLayout {
    pub fn build(snapshot: &TextSnapshot, width: usize, folds: &[CollapsedFold]) -> Result<Self> {
        let width = width.max(1);
        let mut lines = vec![];
        let mut byte = 0;
        for raw in snapshot.lines() {
            let mut end = raw.len_chars();
            if end > 0 && raw.char(end - 1) == '\n' {
                end -= 1;
                if end > 0 && raw.char(end - 1) == '\r' {
                    end -= 1;
                }
            }
            let content = raw.slice(..end);
            let source_start = byte;
            let mut visual = 0;
            let mut runs = vec![];
            for ch in content.chars() {
                let glyph = Glyph::for_scalar(ch);
                let start = byte;
                byte += ch.len_utf8();
                runs.push(GlyphRun {
                    source: TextRange {
                        start: ByteOffset(start),
                        end: ByteOffset(byte),
                    },
                    glyph,
                    visual_start: visual,
                });
                visual += glyph.width();
            }
            lines.push(LineLayout {
                source: TextRange {
                    start: ByteOffset(source_start),
                    end: ByteOffset(byte),
                },
                runs,
                visual_width: visual,
                first_row: 0,
                row_count: 0,
            });
            byte = source_start + raw.len_bytes();
        }
        let mut collapsed: BTreeMap<usize, usize> = BTreeMap::new();
        for fold in folds {
            if fold.end < fold.start || fold.end.0 >= lines.len() {
                return Err(DomainError::InvalidCoordinate {
                    unit: "fold line",
                    index: fold.end.0,
                    limit: lines.len().saturating_sub(1),
                });
            }
            collapsed
                .entry(fold.start.0)
                .and_modify(|end| *end = (*end).max(fold.end.0))
                .or_insert(fold.end.0);
        }
        let mut rows = vec![];
        let mut line = 0;
        while line < lines.len() {
            let first_row = rows.len();
            lines[line].first_row = first_row;
            if let Some(&end) = collapsed.get(&line) {
                rows.push(VisualRow {
                    line: LineIndex(line),
                    source: TextRange {
                        start: lines[line].source.start,
                        end: lines[end].source.end,
                    },
                    run_start: 0,
                    run_end: lines[line].runs.len(),
                    visual_start: 0,
                    visual_width: lines[line].visual_width,
                    fold_end: Some(LineIndex(end)),
                    summary: Some(fold_summary(&lines[line], &lines[end], width)),
                });
                for hidden in &mut lines[line..=end] {
                    hidden.first_row = first_row;
                    hidden.row_count = 1;
                }
                line = end + 1;
                continue;
            }
            if lines[line].runs.is_empty() {
                rows.push(VisualRow {
                    line: LineIndex(line),
                    source: lines[line].source,
                    run_start: 0,
                    run_end: 0,
                    visual_start: 0,
                    visual_width: 0,
                    fold_end: None,
                    summary: None,
                });
            } else {
                let mut start = 0;
                let mut used = 0;
                for index in 0..lines[line].runs.len() {
                    let glyph_width = lines[line].runs[index].glyph.width();
                    if index > start && used + glyph_width > width {
                        rows.push(row(&lines[line], line, start, index));
                        start = index;
                        used = 0;
                    }
                    used += glyph_width;
                }
                rows.push(row(&lines[line], line, start, lines[line].runs.len()));
            }
            lines[line].row_count = rows.len() - first_row;
            line += 1;
        }
        Ok(Self {
            snapshot: snapshot.clone(),
            width,
            lines,
            rows,
        })
    }
    pub fn position_to_cell(&self, byte: ByteOffset) -> Result<(usize, usize)> {
        let position = self.snapshot.byte_to_position(byte)?;
        let line = &self.lines[position.line.0];
        let mut low = line.first_row;
        let mut high = low + line.row_count;
        while low + 1 < high {
            let middle = (low + high) / 2;
            if self.rows[middle].source.start <= byte {
                low = middle;
            } else {
                high = middle;
            }
        }
        let row = &self.rows[low];
        if let Some(summary) = &row.summary {
            let column = summary
                .cells
                .iter()
                .position(|cell| {
                    cell.source.is_some_and(|range| range.start == byte) && cell.position == byte
                })
                .unwrap_or_else(|| {
                    if byte == row.source.end {
                        summary.cells.len()
                    } else {
                        0
                    }
                });
            return Ok((low, column));
        }
        if row.line != position.line {
            return Ok((low, 0));
        }
        let run = line.runs.partition_point(|run| run.source.start < byte);
        let visual = line
            .runs
            .get(run)
            .map_or(line.visual_width, |run| run.visual_start);
        Ok((low, visual.saturating_sub(row.visual_start)))
    }
    /// Matches the source's ceiling inverse: the first cell hits a scalar's
    /// beginning, subsequent cells of its tab/escape hit its valid end boundary.
    pub fn cell_to_position(&self, row: usize, column: usize) -> ByteOffset {
        let row = &self.rows[row.min(self.rows.len() - 1)];
        if let Some(summary) = &row.summary {
            return summary
                .cells
                .get(column)
                .map_or(row.source.end, |cell| cell.position);
        }
        let line = &self.lines[row.line.0];
        let visual = row.visual_start.saturating_add(column);
        let index = line.runs.partition_point(|run| run.visual_start < visual);
        line.runs
            .get(index)
            .map_or(line.source.end, |run| run.source.start)
    }
    pub fn visible_rows(
        &self,
        scroll: usize,
        height: usize,
    ) -> impl Iterator<Item = (usize, &VisualRow)> {
        self.rows.iter().enumerate().skip(scroll).take(height)
    }
}
fn row(line: &LineLayout, index: usize, start: usize, end: usize) -> VisualRow {
    let first = &line.runs[start];
    let last = &line.runs[end - 1];
    VisualRow {
        line: LineIndex(index),
        source: TextRange {
            start: first.source.start,
            end: last.source.end,
        },
        run_start: start,
        run_end: end,
        visual_start: first.visual_start,
        visual_width: last.visual_start + last.glyph.width() - first.visual_start,
        fold_end: None,
        summary: None,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CacheKey {
    document: DocumentId,
    revision: Revision,
    width: usize,
    folds: Vec<CollapsedFold>,
}
#[derive(Default)]
pub struct LayoutCache {
    key: Option<CacheKey>,
    layout: Option<Arc<VisualLayout>>,
}
impl LayoutCache {
    pub fn get(
        &mut self,
        snapshot: &TextSnapshot,
        width: usize,
        folds: &[CollapsedFold],
    ) -> Result<Arc<VisualLayout>> {
        let key = CacheKey {
            document: snapshot.id,
            revision: snapshot.revision,
            width: width.max(1),
            folds: folds.to_vec(),
        };
        if self.key.as_ref() == Some(&key) {
            return Ok(self
                .layout
                .as_ref()
                .expect("cache key implies layout")
                .clone());
        }
        let layout = Arc::new(VisualLayout::build(snapshot, width, folds)?);
        self.key = Some(key);
        self.layout = Some(layout.clone());
        Ok(layout)
    }
}
