use super::{
    input::{EditorInputHooks, EditorState, HookContext, InputTarget},
    layout::CollapsedFold,
    render::EditorHit,
};
use crate::input::terminal::{InputEvent, PointerAction};
use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DomainError, LineIndex, Result, Revision,
    document::{Document, TextSnapshot},
};
use std::collections::{BTreeMap, BTreeSet};

pub fn schedule_discovery(
    scheduler: &mut dyn kode_platform::jobs::WorkerScheduler<FoldReport>,
    owner: kode_platform::jobs::OwnerId,
    tag: kode_core::JobTag,
    snapshot: TextSnapshot,
) -> Result<()> {
    scheduler.schedule_blocking(
        owner,
        tag,
        kode_platform::jobs::Delivery::Cancellable,
        Box::new(move |cancel| discover(&snapshot, || cancel.is_cancelled())),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FoldRegion {
    pub start: LineIndex,
    pub end: LineIndex,
}
#[derive(Debug)]
pub struct FoldReport {
    pub document: DocumentId,
    pub revision: Revision,
    pub regions: Vec<FoldRegion>,
}

/// Pure analysis for a worker. Brace nesting excludes comments/quoted strings;
/// indentation folds follow the source's language and block-start rules.
pub fn discover(snapshot: &TextSnapshot, cancelled: impl Fn() -> bool) -> Result<FoldReport> {
    let language = snapshot
        .language
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let indentation = matches!(
        language.as_str(),
        "python" | "yaml" | "yml" | "bash" | "powershell" | "ruby"
    );
    let mut regions = BTreeSet::new();
    let mut stack = vec![];
    let mut block_comment = 0usize;
    let mut multiline_quote: Option<(char, usize)> = None;
    let mut indent_lines = vec![];
    for (line, raw) in snapshot.lines().enumerate() {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        let chars: Vec<char> = raw.chars().collect();
        let mut quote = multiline_quote;
        let mut escaped = false;
        let mut index = 0;
        while index < chars.len() {
            if index % 4096 == 0 && cancelled() {
                return Err(DomainError::Cancelled);
            }
            let ch = chars[index];
            if block_comment > 0 {
                if ch == '*' && chars.get(index + 1) == Some(&'/') {
                    block_comment -= 1;
                    index += 2;
                } else if language == "kotlin" && ch == '/' && chars.get(index + 1) == Some(&'*') {
                    block_comment += 1;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }
            if let Some((delimiter, count)) = quote {
                if escaped {
                    escaped = false;
                    index += 1;
                } else if ch == '\\' && !(count == 3 && language == "kotlin") {
                    escaped = true;
                    index += 1;
                } else if ch == delimiter
                    && (0..count).all(|n| chars.get(index + n) == Some(&delimiter))
                {
                    quote = None;
                    index += count;
                } else {
                    index += 1;
                }
                continue;
            }
            if ch == '/' && chars.get(index + 1) == Some(&'/') || ch == '#' && indentation {
                break;
            }
            if ch == '/' && chars.get(index + 1) == Some(&'*') {
                block_comment = 1;
                index += 2;
                continue;
            }
            if matches!(ch, '"' | '\'' | '`') {
                let count = if ch != '`'
                    && chars.get(index + 1) == Some(&ch)
                    && chars.get(index + 2) == Some(&ch)
                {
                    3
                } else {
                    1
                };
                quote = Some((ch, count));
                index += count;
                continue;
            }
            match ch {
                '{' => stack.push(line),
                '}' => {
                    if let Some(start) = stack.pop()
                        && line > start
                    {
                        regions.insert(FoldRegion {
                            start: LineIndex(start),
                            end: LineIndex(line),
                        });
                    }
                }
                _ => {}
            }
            index += 1;
        }
        multiline_quote = quote.filter(|(ch, count)| *ch == '`' || *count == 3);
        if indentation {
            let text = raw.to_string();
            let trimmed = text.trim();
            let width = text
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .map(|ch| if ch == '\t' { 4 } else { 1 })
                .sum::<usize>();
            let ruby_start = language == "ruby"
                && [
                    "class", "module", "def", "if", "unless", "case", "begin", "while", "until",
                    "for", "do",
                ]
                .iter()
                .any(|word| {
                    trimmed.strip_prefix(word).is_some_and(|tail| {
                        tail.chars()
                            .next()
                            .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
                    })
                });
            indent_lines.push((
                width,
                trimmed.is_empty(),
                trimmed.ends_with(':') || ruby_start,
            ));
        }
    }
    // An index stack avoids the source's quadratic scan for every nested header.
    let mut headers: Vec<(usize, usize)> = vec![];
    for (line, (width, blank, starts)) in indent_lines.iter().enumerate() {
        if cancelled() {
            return Err(DomainError::Cancelled);
        }
        if !blank {
            while headers.last().is_some_and(|(_, indent)| indent >= width) {
                let (start, _) = headers.pop().expect("header checked");
                if line > start + 1 {
                    regions.insert(FoldRegion {
                        start: LineIndex(start),
                        end: LineIndex(line - 1),
                    });
                }
            }
        }
        if *starts && !blank {
            headers.push((line, *width));
        }
    }
    for (start, _) in headers {
        let end = snapshot.len_lines() - 1;
        if end > start {
            regions.insert(FoldRegion {
                start: LineIndex(start),
                end: LineIndex(end),
            });
        }
    }
    let mut regions: Vec<_> = regions.into_iter().collect();
    regions.sort_by_key(|region| (region.start, std::cmp::Reverse(region.end)));
    Ok(FoldReport {
        document: snapshot.id,
        revision: snapshot.revision,
        regions,
    })
}

#[derive(Debug, Default)]
pub struct FoldState {
    current: Option<(DocumentId, Revision)>,
    regions: BTreeMap<LineIndex, FoldRegion>,
    collapsed: BTreeSet<LineIndex>,
}
impl FoldState {
    pub fn publish(&mut self, report: FoldReport, snapshot: &TextSnapshot) -> bool {
        if (report.document, report.revision) != (snapshot.id, snapshot.revision) {
            return false;
        }
        let stamp = Some((snapshot.id, snapshot.revision));
        if self.current != stamp {
            self.collapsed.clear();
        }
        self.current = stamp;
        self.regions.clear();
        for region in report.regions {
            self.regions
                .entry(region.start)
                .and_modify(|current| {
                    if region.end > current.end {
                        *current = region;
                    }
                })
                .or_insert(region);
        }
        self.collapsed
            .retain(|start| self.regions.contains_key(start));
        true
    }
    pub fn invalidate(&mut self) {
        self.current = None;
        self.regions.clear();
        self.collapsed.clear();
    }
    pub fn regions(&self) -> impl Iterator<Item = &FoldRegion> {
        self.regions.values()
    }
    pub fn toggle(&mut self, line: LineIndex) -> bool {
        if !self.regions.contains_key(&line) {
            return false;
        }
        if !self.collapsed.insert(line) {
            self.collapsed.remove(&line);
        }
        true
    }
    pub fn expand_all(&mut self) {
        self.collapsed.clear();
    }
    pub fn reveal(&mut self, line: LineIndex) -> bool {
        let before = self.collapsed.len();
        self.collapsed.retain(|start| {
            self.regions
                .get(start)
                .is_none_or(|region| !(region.start < line && line <= region.end))
        });
        before != self.collapsed.len()
    }
    pub fn collapsed(&self, snapshot: &TextSnapshot) -> Vec<CollapsedFold> {
        if self.current != Some((snapshot.id, snapshot.revision)) {
            return vec![];
        }
        self.collapsed
            .iter()
            .filter_map(|start| self.regions.get(start))
            .map(|region| CollapsedFold {
                start: region.start,
                end: region.end,
            })
            .collect()
    }
}
impl EditorInputHooks for FoldState {
    fn collapsed_folds(&self, snapshot: &TextSnapshot) -> Option<Vec<CollapsedFold>> {
        Some(self.collapsed(snapshot))
    }
    fn reveal_caret(
        &mut self,
        snapshot: &TextSnapshot,
        position: kode_core::ByteOffset,
    ) -> Result<bool> {
        if self.current != Some((snapshot.id, snapshot.revision)) {
            self.invalidate();
            return Ok(false);
        }
        Ok(self.reveal(snapshot.byte_to_position(position)?.line))
    }
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        if target != InputTarget::Gutter
            || self.current != Some((document.id(), document.revision()))
        {
            return Ok(false);
        }
        let InputEvent::Pointer(pointer) = event else {
            return Ok(false);
        };
        if pointer.action != PointerAction::Down(MouseButton::Left) {
            return Ok(false);
        }
        match context
            .geometry
            .hit_test(context.layout, state.scroll_rows, pointer.x, pointer.y)
        {
            EditorHit::Gutter { line } | EditorHit::FoldToggle { line } => Ok(self.toggle(line)),
            _ => Ok(false),
        }
    }
}

#[derive(Default)]
pub struct EditorVisuals {
    pub folds: FoldState,
}
impl EditorInputHooks for EditorVisuals {
    fn collapsed_folds(&self, snapshot: &TextSnapshot) -> Option<Vec<CollapsedFold>> {
        self.folds.collapsed_folds(snapshot)
    }
    fn reveal_caret(
        &mut self,
        snapshot: &TextSnapshot,
        position: kode_core::ByteOffset,
    ) -> Result<bool> {
        self.folds.reveal_caret(snapshot, position)
    }
    fn dispatch(
        &mut self,
        target: InputTarget,
        event: &InputEvent,
        document: &mut Document,
        state: &mut EditorState,
        context: HookContext<'_>,
    ) -> Result<bool> {
        if target != InputTarget::Minimap {
            return self.folds.dispatch(target, event, document, state, context);
        }
        let InputEvent::Pointer(pointer) = event else {
            return Ok(false);
        };
        if pointer.action != PointerAction::Down(MouseButton::Left) {
            return Ok(false);
        }
        let snapshot = document.snapshot();
        let area = context.geometry.minimap;
        let preview = super::minimap::Minimap::prepare(
            &snapshot,
            area.width as usize,
            area.height as usize,
            state.caret.cursor(),
        )?;
        let Some((position, scroll)) = preview.hit(area, pointer.x, pointer.y, context.layout)?
        else {
            return Ok(false);
        };
        state.caret.move_to(&snapshot, position, false)?;
        let changed = self.folds.reveal(snapshot.byte_to_position(position)?.line);
        state.scroll_rows = if changed {
            let layout = super::layout::VisualLayout::build(
                &snapshot,
                context.geometry.text.width as usize,
                &self.folds.collapsed(&snapshot),
            )?;
            layout
                .position_to_cell(position)?
                .0
                .min(layout.rows.len().saturating_sub(area.height as usize))
        } else {
            scroll
        };
        Ok(true)
    }
}
