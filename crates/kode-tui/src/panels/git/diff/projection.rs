//! Worker-prepared presentation of immutable hunks. Row/hunk identity is kept
//! alongside the text; actions never infer a new chunk from a line number.
use crate::editor::display::Glyph;
use kode_core::{ByteOffset, DomainError, Result, TextRange};
use kode_git::diff::DiffPlan;
use kode_platform::jobs::CancellationToken;
#[derive(Debug, Clone)]
pub struct Paint {
    pub range: TextRange,
    pub class: &'static str,
}
#[derive(Debug, Clone)]
pub struct Row {
    pub hunk: Option<usize>,
    pub marker: Option<std::ops::Range<usize>>,
}
#[derive(Debug)]
pub struct Projection {
    pub text: String,
    pub paint: Vec<Paint>,
    pub rows: Vec<Row>,
    segments: Vec<Segment>,
}
#[derive(Debug, Clone, Copy)]
pub struct SourcePoint {
    left: bool,
    line: usize,
    column: usize,
}
#[derive(Debug)]
struct Segment {
    range: std::ops::Range<usize>,
    source: SourcePoint,
}
fn label(line: Option<usize>, width: usize) -> String {
    let text = line.map_or(String::new(), |line| (line + 1).to_string());
    if text.len() > width {
        "*".repeat(width)
    } else {
        text
    }
}
fn limit() -> DomainError {
    DomainError::Unsupported(
        "diff presentation exceeds 100,000 rows/16 MiB; use a narrower comparison".into(),
    )
}
fn wrap(text: &str, width: usize, cancel: &CancellationToken) -> Result<Vec<String>> {
    let mut result = vec![];
    let mut line = String::new();
    let mut cells = 0;
    let mut bytes = 0usize;
    for ch in text.chars() {
        let glyph = Glyph::for_scalar(ch);
        for ch in glyph.text().chars() {
            if cells == width {
                if cancel.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                bytes += line.len();
                if result.len() >= 100_000 || bytes > 16 * 1024 * 1024 {
                    return Err(limit());
                }
                result.push(std::mem::take(&mut line));
                cells = 0;
            }
            line.push(ch);
            cells += 1;
        }
    }
    result.push(line);
    Ok(result)
}
fn text(plan: &DiffPlan, left: bool, line: Option<usize>) -> String {
    let (source, lines) = if left {
        (&plan.contents().old.bytes, plan.old_lines())
    } else {
        (&plan.contents().new.bytes, plan.new_lines())
    };
    let Some(range) = line.and_then(|line| lines.get(line)) else {
        return String::new();
    };
    let bytes = &source[range.clone()];
    // Keep CR visible, and make an absent final LF explicit in both preview modes.
    let source = std::str::from_utf8(bytes.strip_suffix(b"\n").unwrap_or(bytes)).unwrap_or("");
    if bytes.ends_with(b"\n") {
        source.into()
    } else {
        format!("{source} [no newline]")
    }
}
impl Projection {
    pub fn build(
        plan: &DiffPlan,
        width: u16,
        context: bool,
        unified: bool,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let mut output = Self {
            text: String::new(),
            paint: vec![],
            rows: vec![],
            segments: vec![],
        };
        if let Some(notice) = plan.notice() {
            output.text = notice.into();
            return Ok(output);
        }
        let width = usize::from(width.max(1));
        let mut source_rows = vec![];
        let (mut left, mut right) = (0, 0);
        for (id, hunk) in plan.hunks().iter().enumerate() {
            if context {
                while left < hunk.old_lines.start && right < hunk.new_lines.start {
                    source_rows.push((Some(left), Some(right), None));
                    left += 1;
                    right += 1;
                }
            } else if id > 0 {
                source_rows.push((None, None, None));
            }
            let length = hunk.old_lines.len().max(hunk.new_lines.len());
            for offset in 0..length {
                source_rows.push((
                    (offset < hunk.old_lines.len()).then_some(hunk.old_lines.start + offset),
                    (offset < hunk.new_lines.len()).then_some(hunk.new_lines.start + offset),
                    Some(id),
                ));
            }
            left = hunk.old_lines.end;
            right = hunk.new_lines.end;
        }
        if context {
            while left < plan.old_lines().len() || right < plan.new_lines().len() {
                source_rows.push((
                    (left < plan.old_lines().len()).then_some(left),
                    (right < plan.new_lines().len()).then_some(right),
                    None,
                ));
                left += 1;
                right += 1;
            }
        }
        for (left, right, hunk) in source_rows {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            if output.rows.len() >= 100_000 || output.text.len() >= 16 * 1024 * 1024 {
                return Err(DomainError::Unsupported(
                    "diff presentation exceeds 100,000 rows/16 MiB; use a narrower comparison"
                        .into(),
                ));
            }
            let class = match (hunk, left, right) {
                (None, _, _) => "diff-context",
                (Some(_), None, _) => "diff-added",
                (Some(_), _, None) => "diff-removed",
                _ => "diff-modified",
            };
            if unified || width < 17 {
                if hunk.is_none() {
                    let line = format!(
                        " {} {}",
                        left.map_or(String::new(), |line| (line + 1).to_string()),
                        text(plan, true, left)
                    );
                    let prefix = format!(
                        " {} ",
                        left.map_or(String::new(), |line| (line + 1).to_string())
                    )
                    .len();
                    for (index, line) in wrap(&line, width, cancel)?.into_iter().enumerate() {
                        let start = output.text.len();
                        output.push(&line, class, hunk, None)?;
                        if let Some(source_line) = left {
                            output.map_wrapped(
                                start,
                                &line,
                                prefix,
                                index * width,
                                true,
                                source_line,
                            );
                        }
                    }
                } else {
                    for (is_left, line, prefix, style) in [
                        (true, left, '-', "diff-removed"),
                        (false, right, '+', "diff-added"),
                    ] {
                        if let Some(line) = line {
                            let source_line = line;
                            let prefix = format!("{prefix}{} ", line + 1);
                            let text = format!("{prefix}{}", text(plan, is_left, Some(line)));
                            for (index, line) in wrap(&text, width, cancel)?.into_iter().enumerate()
                            {
                                let start = output.text.len();
                                output.push(&line, style, hunk, None)?;
                                output.map_wrapped(
                                    start,
                                    &line,
                                    prefix.len(),
                                    index * width,
                                    is_left,
                                    source_line,
                                );
                            }
                        }
                    }
                }
            } else {
                let number = plan
                    .old_lines()
                    .len()
                    .max(plan.new_lines().len())
                    .max(1)
                    .to_string()
                    .len()
                    .max(2);
                let side = (width - 3) / 2;
                let number = number.min(side.saturating_sub(2));
                let left_width = side.saturating_sub(number + 1).max(1);
                let right_width = width.saturating_sub(side + 3 + number + 1).max(1);
                let l = wrap(&text(plan, true, left), left_width, cancel)?;
                let r = wrap(&text(plan, false, right), right_width, cancel)?;
                for index in 0..l.len().max(r.len()) {
                    let start = output.text.len();
                    let lnum = if index == 0 {
                        label(left, number)
                    } else {
                        String::new()
                    };
                    let rnum = if index == 0 {
                        label(right, number)
                    } else {
                        String::new()
                    };
                    let ltext = l.get(index).map_or("", String::as_str);
                    let rtext = r.get(index).map_or("", String::as_str);
                    let marker = if hunk.is_some() && index == 0 {
                        ">>"
                    } else {
                        " |"
                    };
                    let mut line = format!("{lnum:>number$} {ltext}");
                    line.push_str(&" ".repeat(side.saturating_sub(line.chars().count())));
                    let marker_start = line.chars().count();
                    let right_source_start = line.len() + 3 + number + 1;
                    line.push_str(&format!("{marker} {rnum:>number$} {rtext}"));
                    line.push_str(&" ".repeat(width.saturating_sub(line.chars().count())));
                    output.push(
                        &line,
                        class,
                        hunk,
                        (hunk.is_some() && index == 0).then_some(marker_start..marker_start + 2),
                    )?;
                    if let Some(source_line) = left
                        && !ltext.is_empty()
                    {
                        output.segments.push(Segment {
                            range: start + number + 1..start + number + 1 + ltext.len(),
                            source: SourcePoint {
                                left: true,
                                line: source_line,
                                column: index * left_width,
                            },
                        });
                    }
                    if let Some(source_line) = right
                        && !rtext.is_empty()
                    {
                        output.segments.push(Segment {
                            range: start + right_source_start
                                ..start + right_source_start + rtext.len(),
                            source: SourcePoint {
                                left: false,
                                line: source_line,
                                column: index * right_width,
                            },
                        });
                    }
                    for range in [
                        start..start + number + 1,
                        start + right_source_start - number - 1..start + right_source_start,
                    ] {
                        output.paint.push(Paint {
                            range: TextRange {
                                start: ByteOffset(range.start),
                                end: ByteOffset(range.end),
                            },
                            class: "diff-gutter",
                        });
                    }
                    // Insertion/removal use the source's per-side color policy.
                    if hunk.is_some() && (left.is_none() || right.is_none()) {
                        let split = start
                            + output.text[start..]
                                .char_indices()
                                .nth(side)
                                .map_or(line.len(), |(byte, _)| byte);
                        if let Some(paint) = output.paint.last_mut() {
                            paint.range.end = ByteOffset(split);
                            paint.class = "diff-removed";
                        }
                        output.paint.push(Paint {
                            range: TextRange {
                                start: ByteOffset(split),
                                end: ByteOffset(output.text.len() - 1),
                            },
                            class: if left.is_none() {
                                "diff-added"
                            } else {
                                "diff-context"
                            },
                        });
                    }
                }
            }
        }
        if output.text.is_empty() {
            output.text = "No changes".into();
        }
        Ok(output)
    }
    fn map_wrapped(
        &mut self,
        start: usize,
        text: &str,
        prefix: usize,
        column: usize,
        left: bool,
        line: usize,
    ) {
        let skip = prefix.saturating_sub(column).min(text.chars().count());
        let byte = text
            .char_indices()
            .nth(skip)
            .map_or(text.len(), |(byte, _)| byte);
        if byte < text.len() {
            self.segments.push(Segment {
                range: start + byte..start + text.len(),
                source: SourcePoint {
                    left,
                    line,
                    column: column.saturating_add(skip).saturating_sub(prefix),
                },
            });
        }
    }
    pub fn source_point(&self, offset: ByteOffset) -> Option<SourcePoint> {
        let segment = self
            .segments
            .iter()
            .find(|segment| segment.range.start <= offset.0 && offset.0 <= segment.range.end)
            .or_else(|| {
                self.segments
                    .iter()
                    .find(|segment| segment.range.start >= offset.0)
            })?;
        let end = offset.0.clamp(segment.range.start, segment.range.end);
        let column = self.text.get(segment.range.start..end)?.chars().count();
        Some(SourcePoint {
            column: segment.source.column + column,
            ..segment.source
        })
    }
    pub fn projected_offset(&self, point: SourcePoint) -> Option<ByteOffset> {
        let segment = self.segments.iter().find(|segment| {
            segment.source.left == point.left
                && segment.source.line == point.line
                && segment.source.column <= point.column
                && segment.source.column + self.text[segment.range.clone()].chars().count()
                    >= point.column
        })?;
        let relative = point.column - segment.source.column;
        let byte = self.text[segment.range.clone()]
            .char_indices()
            .nth(relative)
            .map_or(segment.range.len(), |(byte, _)| byte);
        Some(ByteOffset(segment.range.start + byte))
    }
    fn push(
        &mut self,
        text: &str,
        class: &'static str,
        hunk: Option<usize>,
        marker: Option<std::ops::Range<usize>>,
    ) -> Result<()> {
        if self.rows.len() >= 100_000
            || self.text.len().saturating_add(text.len() + 1) > 16 * 1024 * 1024
        {
            return Err(limit());
        }
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        self.text.push('\n');
        self.paint.push(Paint {
            range: TextRange {
                start: ByteOffset(start),
                end: ByteOffset(end),
            },
            class,
        });
        self.rows.push(Row { hunk, marker });
        Ok(())
    }
}
