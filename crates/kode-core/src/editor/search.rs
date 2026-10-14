use super::{edit::TextEdit, selection::CaretState};
use crate::{
    ByteOffset, DocumentId, DomainError, Result, Revision, Selection, TextRange,
    commands::EditKind,
    document::{Document, TextSnapshot},
};
use std::sync::Arc;

pub const MAX_MATCHES: usize = 100_000;
pub const MAX_PATTERN_BYTES: usize = 65_536;
#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub range: TextRange,
    pub captures: Vec<Option<TextRange>>,
}
#[derive(Debug, Clone)]
pub struct SearchReport {
    pub document: DocumentId,
    pub revision: Revision,
    pub query: String,
    pub error: Option<String>,
    pub matches: Vec<SearchMatch>,
    pub text: Arc<str>,
}
impl SearchReport {
    pub fn is_current(&self, snapshot: &TextSnapshot) -> bool {
        self.document == snapshot.id && self.revision == snapshot.revision
    }
}

/// Pure worker operation. Empty/zero-width matches are excluded consistently
/// from counts, selection and replacement, as the source's visible search does.
pub fn scan(
    snapshot: &TextSnapshot,
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Result<SearchReport> {
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let text = snapshot.to_service_snapshot().text;
    let mut report = SearchReport {
        document: snapshot.id,
        revision: snapshot.revision,
        query: query.into(),
        error: None,
        matches: vec![],
        text,
    };
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    if query.is_empty() {
        return Ok(report);
    }
    let run = || -> std::result::Result<Vec<SearchMatch>, String> {
        if query.len() > MAX_PATTERN_BYTES {
            return Err(format!("Regex exceeds {MAX_PATTERN_BYTES} byte limit"));
        }
        let pattern = super::search_pattern::translate(query)?;
        let mut matches = vec![];
        let mut push = |captures: Vec<Option<TextRange>>| -> std::result::Result<(), String> {
            if cancelled() {
                return Err("cancelled".into());
            }
            let range = captures[0].expect("complete match capture is always present");
            if !range.is_empty() {
                if matches.len() >= MAX_MATCHES {
                    return Err(format!(
                        "Search exceeds {MAX_MATCHES} matches; narrow the pattern"
                    ));
                }
                matches.push(SearchMatch { range, captures });
            }
            Ok(())
        };
        match regex::RegexBuilder::new(&pattern)
            .size_limit(8 * 1024 * 1024)
            .build()
        {
            Ok(regex) => {
                for captures in regex.captures_iter(report.text.as_ref()) {
                    push(
                        captures
                            .iter()
                            .map(|capture| {
                                capture.map(|capture| TextRange {
                                    start: ByteOffset(capture.start()),
                                    end: ByteOffset(capture.end()),
                                })
                            })
                            .collect(),
                    )?;
                }
            }
            Err(_) => {
                let regex = fancy_regex::RegexBuilder::new(&pattern)
                    .backtrack_limit(100_000)
                    .delegate_size_limit(8 * 1024 * 1024)
                    .build()
                    .map_err(|error| format!("Invalid or unsupported Java regex: {error}"))?;
                for captures in regex.captures_iter(report.text.as_ref()) {
                    let captures =
                        captures.map_err(|error| format!("Regex execution failed: {error}"))?;
                    push(
                        (0..captures.len())
                            .map(|index| {
                                captures.get(index).map(|capture| TextRange {
                                    start: ByteOffset(capture.start()),
                                    end: ByteOffset(capture.end()),
                                })
                            })
                            .collect(),
                    )?;
                }
            }
        }
        Ok(matches)
    };
    match run() {
        Ok(matches) => report.matches = matches,
        Err(_) if cancelled() => return Err(DomainError::Cancelled),
        Err(error) => report.error = Some(error),
    }
    Ok(report)
}

/// Kode's custom replacement: greedily consume numeric $n references, drop
/// missing/unmatched groups, and keep other dollars/backslashes literally.
pub fn expand_replacement(replacement: &str, matched: &SearchMatch, text: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$'
            && chars
                .peek()
                .is_some_and(|ch| replacement_digit(*ch).is_some())
        {
            let mut index = Some(0usize);
            while let Some(digit) = chars.next_if(|ch| replacement_digit(*ch).is_some()) {
                index = index.and_then(|value| {
                    value
                        .checked_mul(10)?
                        .checked_add(replacement_digit(digit).expect("validated digit"))
                        .filter(|value| *value <= i32::MAX as usize)
                });
            }
            if let Some(range) = index
                .and_then(|index| matched.captures.get(index))
                .and_then(|range| *range)
            {
                out.push_str(&text[range.start.0..range.end.0]);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn replacement_digit(ch: char) -> Option<usize> {
    use unicode_general_category::{GeneralCategory::DecimalNumber, get_general_category};
    if ch.is_ascii_digit() {
        return Some(ch as usize - '0' as usize);
    }
    // Kotlin's Char-based parser uses Java 21 Character.digit: BMP decimal
    // digits are references, supplementary digits remain literal surrogate pairs.
    if ch as u32 > 0xFFFF || get_general_category(ch) != DecimalNumber {
        return None;
    }
    let mut previous = ch as u32;
    let mut digit = 0;
    while previous > 0
        && char::from_u32(previous - 1).is_some_and(|ch| get_general_category(ch) == DecimalNumber)
    {
        previous -= 1;
        digit += 1;
    }
    Some(digit % 10)
}

#[derive(Debug, Default)]
pub struct SearchResults {
    pub report: Option<SearchReport>,
    pub active: Option<usize>,
}
impl SearchResults {
    pub fn publish(&mut self, report: SearchReport, snapshot: &TextSnapshot, query: &str) -> bool {
        if !report.is_current(snapshot) || report.query != query {
            return false;
        }
        self.active = self.active.filter(|index| *index < report.matches.len());
        self.report = Some(report);
        true
    }
    pub fn invalidate(&mut self) {
        self.report = None;
        self.active = None;
    }
    pub fn next(
        &mut self,
        snapshot: &TextSnapshot,
        caret: &mut CaretState,
    ) -> Result<Option<TextRange>> {
        let Some(report) = &self.report else {
            return Ok(None);
        };
        if !report.is_current(snapshot) || report.matches.is_empty() {
            return Ok(None);
        }
        let index = self
            .active
            .map_or(0, |index| (index + 1) % report.matches.len());
        let range = report.matches[index].range;
        caret.set_selection(
            snapshot,
            Selection {
                anchor: range.start,
                head: range.end,
            },
        )?;
        self.active = Some(index);
        Ok(Some(range))
    }
    pub fn replace(
        &mut self,
        document: &mut Document,
        caret: &mut CaretState,
        replacement: &str,
        all: bool,
    ) -> Result<usize> {
        if document.is_read_only() {
            return Err(DomainError::ReadOnly);
        }
        let Some(report) = &self.report else {
            return Ok(0);
        };
        if !report.is_current(&document.snapshot()) {
            return Err(DomainError::StaleRevision {
                expected: report.revision,
                current: document.revision(),
            });
        }
        if report.matches.is_empty() {
            return Ok(0);
        }
        let index = self.active.unwrap_or(0).min(report.matches.len() - 1);
        let selected: Vec<_> = report
            .matches
            .iter()
            .enumerate()
            .filter(|(current, _)| all || *current == index)
            .map(|(_, matched)| TextEdit {
                range: matched.range,
                inserted: expand_replacement(replacement, matched, &report.text),
            })
            .collect();
        let count = selected.len();
        let outcome = document.apply_transaction(
            document.revision(),
            selected,
            if all {
                EditKind::ReplaceAll
            } else {
                EditKind::Replace
            },
            caret.selection,
            None,
        )?;
        if let Some(outcome) = outcome {
            caret.selection = outcome.selection;
            self.report = None;
            // Preserve source's current index when refreshing after Replace One.
            self.active = if all { None } else { Some(index) };
        }
        Ok(count)
    }
}
