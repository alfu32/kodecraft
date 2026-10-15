//! A compiled Java-compatible pattern reusable across project files/lines.
use crate::{ByteOffset, DomainError, Result, TextRange};
enum Engine {
    Linear(regex::Regex),
    Backtracking(fancy_regex::Regex),
}
pub struct SearchRegex(Engine);
fn error(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "search regex",
        message: message.into(),
    }
}
impl SearchRegex {
    pub fn capture_count(&self) -> usize {
        match &self.0 {
            Engine::Linear(pattern) => pattern.captures_len(),
            Engine::Backtracking(pattern) => pattern.captures_len(),
        }
    }
    /// Numbered capture ranges for configurable definition extractors. The
    /// callback owns emission/cancellation limits, as for named captures.
    pub fn for_each_capture(
        &self,
        text: &str,
        group: usize,
        mut found: impl FnMut(TextRange) -> Result<()>,
    ) -> Result<()> {
        if group >= self.capture_count() {
            return Err(error("capture group does not exist"));
        }
        let span = |start, end| TextRange {
            start: ByteOffset(start),
            end: ByteOffset(end),
        };
        match &self.0 {
            Engine::Linear(pattern) => {
                for captures in pattern.captures_iter(text) {
                    if let Some(value) = captures.get(group) {
                        found(span(value.start(), value.end()))?;
                    }
                }
            }
            Engine::Backtracking(pattern) => {
                for captures in pattern.captures_iter(text) {
                    let captures = captures.map_err(|failure| error(failure.to_string()))?;
                    if let Some(value) = captures.get(group) {
                        found(span(value.start(), value.end()))?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn compile(query: &str) -> Result<Self> {
        if query.len() > super::search::MAX_PATTERN_BYTES {
            return Err(error("pattern exceeds 65536 bytes"));
        }
        let pattern = super::search_pattern::translate(query).map_err(error)?;
        let engine = match regex::RegexBuilder::new(&pattern)
            .size_limit(8 * 1024 * 1024)
            .build()
        {
            Ok(pattern) => Engine::Linear(pattern),
            Err(_) => Engine::Backtracking(
                fancy_regex::RegexBuilder::new(&super::search_pattern::backtracking(&pattern))
                    .backtrack_limit(100_000)
                    .delegate_size_limit(8 * 1024 * 1024)
                    .build()
                    .map_err(|failure| error(failure.to_string()))?,
            ),
        };
        Ok(Self(engine))
    }
    /// Source-compatible named capture ranges for lexical grammar consumers.
    /// Coordinates remain byte offsets; callers bound emitted matches themselves.
    pub fn for_each_named(
        &self,
        text: &str,
        groups: &[&str],
        mut found: impl FnMut(TextRange, &[Option<TextRange>]) -> Result<()>,
    ) -> Result<()> {
        let span = |start, end| TextRange {
            start: ByteOffset(start),
            end: ByteOffset(end),
        };
        match &self.0 {
            Engine::Linear(pattern) => {
                for captures in pattern.captures_iter(text) {
                    let matched = captures.get(0).expect("whole regex match");
                    let ranges: Vec<_> = groups
                        .iter()
                        .map(|name| {
                            captures
                                .name(name)
                                .map(|matched| span(matched.start(), matched.end()))
                        })
                        .collect();
                    found(span(matched.start(), matched.end()), &ranges)?;
                }
            }
            Engine::Backtracking(pattern) => {
                for captures in pattern.captures_iter(text) {
                    let captures = captures.map_err(|failure| error(failure.to_string()))?;
                    let matched = captures.get(0).expect("whole regex match");
                    let ranges: Vec<_> = groups
                        .iter()
                        .map(|name| {
                            captures
                                .name(name)
                                .map(|matched| span(matched.start(), matched.end()))
                        })
                        .collect();
                    found(span(matched.start(), matched.end()), &ranges)?;
                }
            }
        }
        Ok(())
    }
    pub fn is_match(&self, text: &str) -> Result<bool> {
        match &self.0 {
            Engine::Linear(pattern) => Ok(pattern.is_match(text)),
            Engine::Backtracking(pattern) => pattern
                .is_match(text)
                .map_err(|failure| error(failure.to_string())),
        }
    }
    /// Retains zero-width matches for the original project search's line results.
    /// The caller supplies an explicit total match limit and cancellation checks.
    pub fn for_each(
        &self,
        text: &str,
        mut found: impl FnMut(TextRange) -> Result<()>,
    ) -> Result<()> {
        match &self.0 {
            Engine::Linear(pattern) => {
                for matched in pattern.find_iter(text) {
                    found(TextRange {
                        start: ByteOffset(matched.start()),
                        end: ByteOffset(matched.end()),
                    })?;
                }
            }
            Engine::Backtracking(pattern) => {
                for matched in pattern.find_iter(text) {
                    let matched = matched.map_err(|failure| error(failure.to_string()))?;
                    found(TextRange {
                        start: ByteOffset(matched.start()),
                        end: ByteOffset(matched.end()),
                    })?;
                }
            }
        }
        Ok(())
    }
}
