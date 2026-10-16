//! SQL targets use UTF-8 byte ranges from the actual editor selection. The
//! scanner borrows text and never expands a cursor miss into the whole script.
use kode_core::{DomainError, Result};
use std::ops::Range;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Standard,
    MySql,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Located<'a> {
    pub sql: &'a str,
    pub range: Range<usize>,
}
impl std::fmt::Debug for Located<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocatedSql")
            .field("range", &self.range)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy)]
enum State {
    Normal,
    Single(bool),
    Double,
    Backtick,
    Bracket,
    Line,
    Block(usize),
    Dollar(usize, usize),
}
struct Segment<'a> {
    located: Located<'a>,
    code: bool,
}
struct Scanner<'a> {
    text: &'a str,
    dialect: Dialect,
    state: State,
    start: usize,
    at: usize,
    code: bool,
    ended: bool,
}
impl<'a> Scanner<'a> {
    fn new(text: &'a str, dialect: Dialect) -> Self {
        Self {
            text,
            dialect,
            state: State::Normal,
            start: 0,
            at: 0,
            code: false,
            ended: false,
        }
    }
    fn dollar(&self) -> Option<usize> {
        if self.dialect == Dialect::MySql
            || self.text[..self.at]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
        {
            return None;
        }
        let rest = &self.text[self.at + 1..];
        if rest.starts_with('$') {
            return Some(self.at + 2);
        }
        let mut first = true;
        for (offset, ch) in rest.char_indices() {
            if ch == '$' && !first {
                return Some(self.at + offset + 2);
            }
            if !(ch == '_'
                || if first {
                    ch.is_alphabetic()
                } else {
                    ch.is_alphanumeric()
                })
            {
                return None;
            }
            first = false;
        }
        None
    }
}
impl<'a> Iterator for Scanner<'a> {
    type Item = Segment<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.ended {
            return None;
        }
        let bytes = self.text.as_bytes();
        loop {
            let mut boundary = None;
            while self.at < bytes.len() {
                let i = self.at;
                let ch = bytes[i];
                let next = bytes.get(i + 1).copied();
                self.at += 1;
                match self.state {
                    State::Line => {
                        if matches!(ch, b'\r' | b'\n') {
                            self.state = State::Normal;
                        }
                    }
                    State::Block(depth) => {
                        if ch == b'/' && next == Some(b'*') && self.dialect != Dialect::MySql {
                            self.state = State::Block(depth + 1);
                            self.at += 1;
                        } else if ch == b'*' && next == Some(b'/') {
                            self.state = if depth == 1 {
                                State::Normal
                            } else {
                                State::Block(depth - 1)
                            };
                            self.at += 1;
                        }
                    }
                    State::Single(escape) => {
                        if escape && ch == b'\\' && next.is_some() {
                            self.at += 1;
                        } else if ch == b'\'' {
                            if next == Some(b'\'') {
                                self.at += 1;
                            } else {
                                self.state = State::Normal;
                            }
                        }
                    }
                    State::Double => {
                        if self.dialect == Dialect::MySql && ch == b'\\' && next.is_some() {
                            self.at += 1;
                        } else if ch == b'"' {
                            if next == Some(b'"') {
                                self.at += 1;
                            } else {
                                self.state = State::Normal;
                            }
                        }
                    }
                    State::Backtick => {
                        if self.dialect == Dialect::MySql && ch == b'\\' && next.is_some() {
                            self.at += 1;
                        } else if ch == b'`' {
                            if next == Some(b'`') {
                                self.at += 1;
                            } else {
                                self.state = State::Normal;
                            }
                        }
                    }
                    State::Bracket => {
                        if ch == b']' {
                            if next == Some(b']') {
                                self.at += 1;
                            } else {
                                self.state = State::Normal;
                            }
                        }
                    }
                    State::Dollar(start, end) => {
                        if ch == b'$' && bytes[i..].starts_with(&bytes[start..end]) {
                            self.at = i + end - start;
                            self.state = State::Normal;
                        }
                    }
                    State::Normal => match (ch, next) {
                        (b'-', Some(b'-'))
                            if self.dialect != Dialect::MySql
                                || bytes.get(i + 2).is_some_and(|ch| {
                                    ch.is_ascii_whitespace() || ch.is_ascii_control()
                                }) =>
                        {
                            self.state = State::Line;
                            self.at += 1;
                        }
                        (b'/', Some(b'*')) => {
                            if self.dialect == Dialect::MySql && bytes.get(i + 2) == Some(&b'!') {
                                self.code = true;
                            }
                            self.state = State::Block(1);
                            self.at += 1;
                        }
                        (b'#', _) if self.dialect == Dialect::MySql => {
                            self.state = State::Line;
                        }
                        (b'\'', _) => {
                            let prefixed = i > 0
                                && matches!(bytes[i - 1], b'e' | b'E')
                                && !self.text[..i - 1].chars().next_back().is_some_and(|ch| {
                                    ch.is_alphanumeric() || matches!(ch, '_' | '$')
                                });
                            self.state = State::Single(self.dialect == Dialect::MySql || prefixed);
                            self.code = true;
                        }
                        (b'"', _) => {
                            self.state = State::Double;
                            self.code = true;
                        }
                        (b'`', _) => {
                            self.state = State::Backtick;
                            self.code = true;
                        }
                        (b'[', _) => {
                            self.state = State::Bracket;
                            self.code = true;
                        }
                        (b'$', _) => {
                            self.at = i;
                            if let Some(end) = self.dollar() {
                                self.state = State::Dollar(i, end);
                                self.at = end;
                            } else {
                                self.at = i + 1;
                            }
                            self.code = true;
                        }
                        (b';', _) => {
                            boundary = Some(i);
                            break;
                        }
                        _ => {
                            let scalar = self.text[i..].chars().next().expect("SQL scalar");
                            self.at = i + scalar.len_utf8();
                            if !scalar.is_whitespace() {
                                self.code = true;
                            }
                        }
                    },
                }
            }
            let end = boundary.unwrap_or(bytes.len());
            let range = self.start..end;
            self.start = self.at;
            let code = std::mem::take(&mut self.code);
            if boundary.is_none() {
                self.ended = true;
            }
            let sql = self.text[range.clone()].trim();
            if !sql.is_empty() {
                return Some(Segment {
                    located: Located { sql, range },
                    code,
                });
            }
            if self.ended {
                return None;
            }
        }
    }
}
pub fn statements(text: &str, dialect: Dialect) -> impl Iterator<Item = Located<'_>> {
    Scanner::new(text, dialect)
        .filter(|segment| segment.code)
        .map(|segment| segment.located)
}
pub fn script(text: &str, dialect: Dialect) -> Option<Located<'_>> {
    statements(text, dialect).next()?;
    Some(Located {
        sql: text.trim(),
        range: 0..text.len(),
    })
}
pub fn locate(
    text: &str,
    cursor: usize,
    selection: Option<Range<usize>>,
    dialect: Dialect,
) -> Result<Option<Located<'_>>> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return Err(DomainError::Unsupported(
            "SQL cursor must be a UTF-8 byte boundary".into(),
        ));
    }
    if let Some(range) = selection {
        let selected = text.get(range.clone()).ok_or_else(|| {
            DomainError::Unsupported("SQL selection must use valid UTF-8 byte boundaries".into())
        })?;
        if !selected.trim().is_empty() {
            return Ok(script(selected, dialect).map(|_| Located {
                sql: selected.trim(),
                range,
            }));
        }
    }
    let mut last = None;
    for segment in Scanner::new(text, dialect) {
        if cursor <= segment.located.range.end {
            return Ok(segment.code.then_some(segment.located));
        }
        last = Some(segment);
    }
    Ok(last
        .filter(|segment| segment.code)
        .map(|segment| segment.located))
}
