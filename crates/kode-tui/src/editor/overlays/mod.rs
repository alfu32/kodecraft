//! Prepared paint only. Order: lexical < semantic < warning background < hover
//! < search < active search < transient < tab marker < selection < cursor.
mod cache;
pub mod remap;
pub mod worker;
use super::render::OverlaySpan;
use crate::theme::Theme;
pub use cache::OverlayCache;
use kode_core::TextRange;
use ratatui::style::{Color, Style};
use std::collections::{BTreeMap, BTreeSet};
pub use worker::{OverlayJob, OverlayReport, OverlayWorker};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaintSpan {
    pub range: TextRange,
    pub class: String,
    pub color: Option<[u8; 3]>,
    pub priority: i32,
}
pub fn rule(theme: &Theme, class: &str) -> Style {
    theme
        .rules
        .get(class)
        .map_or(Style::default(), |rule| rule.style)
}
/// Sweep endpoints to produce disjoint segments. A source scalar is drawn once
/// by the layout renderer, regardless of overlapping parser/recovery captures.
pub fn compose(spans: &[PaintSpan], theme: &Theme) -> Vec<OverlaySpan> {
    let spans: Vec<_> = spans
        .iter()
        .map(|span| {
            let mut style = rule(theme, &span.class);
            if let Some([r, g, b]) = span.color {
                style = style.fg(Color::Rgb(r, g, b));
            }
            OverlaySpan {
                range: span.range,
                priority: span.priority,
                style,
            }
        })
        .collect();
    compose_styles(&spans)
}
pub fn compose_styles(spans: &[OverlaySpan]) -> Vec<OverlaySpan> {
    let mut events: BTreeMap<_, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (id, span) in spans
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.range.is_empty())
    {
        events.entry(span.range.start).or_default().0.push(id);
        events.entry(span.range.end).or_default().1.push(id);
    }
    let mut active: BTreeSet<(i32, std::cmp::Reverse<usize>, kode_core::ByteOffset, usize)> =
        BTreeSet::new();
    let mut previous = None;
    let mut output: Vec<OverlaySpan> = vec![];
    for (offset, (starts, ends)) in events {
        if let Some(start) = previous
            && start < offset
            && !active.is_empty()
        {
            let mut style = Style::default();
            for (_, _, _, id) in &active {
                style = style.patch(spans[*id].style);
            }
            if let Some(last) = output.last_mut()
                && last.range.end == start
                && last.style == style
            {
                last.range.end = offset;
            } else {
                output.push(OverlaySpan {
                    range: TextRange { start, end: offset },
                    style,
                    priority: 20,
                });
            }
        }
        let key = |id: usize| {
            (
                spans[id].priority,
                std::cmp::Reverse(spans[id].range.len()),
                spans[id].range.start,
                id,
            )
        };
        for id in ends {
            active.remove(&key(id));
        }
        for id in starts {
            active.insert(key(id));
        }
        previous = Some(offset);
    }
    output
}
