//! SQL targets use UTF-8 byte ranges from the actual editor selection. The
//! scanner borrows text and never expands a cursor miss into the whole script.
use kode_core::{DomainError, Result};
use std::{borrow::Cow, ops::Range};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Standard,
    MySql,
    Oracle,
}
impl Dialect {
    pub fn from_driver_id(driver_id: &str) -> Self {
        match driver_id {
            "mysql" | "mariadb" => Self::MySql,
            "oracle" => Self::Oracle,
            _ => Self::Standard,
        }
    }
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

/// Splits an explicit script into driver-ready statements. Oracle PL/SQL units
/// stay intact even though their internal statements contain semicolons.
pub fn script_statements(text: &str, dialect: Dialect) -> Vec<Located<'_>> {
    if dialect != Dialect::Oracle {
        return statements(text, dialect).collect();
    }

    let segments: Vec<_> = Scanner::new(text, Dialect::Standard).collect();
    let mut output = Vec::new();
    let mut block: Option<(usize, OracleBlock)> = None;

    for (index, segment) in segments.iter().enumerate() {
        if !segment.code {
            continue;
        }
        let mut located = segment.located.clone();
        if let Some(range) = without_oracle_slash_line(text, located.range.clone()) {
            located = trimmed_located(text, range);
            if located.sql.is_empty() {
                continue;
            }
        }

        if let Some((start, state)) = &mut block {
            let ended = state.observe(&oracle_words(located.sql));
            let next_words = segments[index + 1..]
                .iter()
                .find(|next| next.code)
                .map(|next| oracle_words(next.located.sql));
            let package_continues = state.package
                && next_words.as_ref().is_some_and(|words| {
                    matches!(
                        words.first().map(String::as_str),
                        Some("END" | "FUNCTION" | "PROCEDURE")
                    )
                });
            if ended && !package_continues {
                let end = located.range.end
                    + usize::from(text.as_bytes().get(located.range.end) == Some(&b';'));
                output.push(trimmed_located(text, *start..end));
                block = None;
            }
            continue;
        }

        if plsql(located.sql) {
            let start = located.range.start;
            let mut state = OracleBlock::new(located.sql);
            let ended = state.observe(&oracle_words(located.sql));
            if ended {
                let next_words = segments[index + 1..]
                    .iter()
                    .find(|next| next.code)
                    .map(|next| oracle_words(next.located.sql));
                let package_continues = state.package
                    && next_words.as_ref().is_some_and(|words| {
                        matches!(
                            words.first().map(String::as_str),
                            Some("END" | "FUNCTION" | "PROCEDURE")
                        )
                    });
                if !package_continues {
                    let end = located.range.end
                        + usize::from(text.as_bytes().get(located.range.end) == Some(&b';'));
                    output.push(trimmed_located(text, start..end));
                    continue;
                }
            }
            block = Some((start, state));
            continue;
        }

        output.push(located);
    }

    if let Some((start, _)) = block {
        output.push(trimmed_located(text, start..text.len()));
    }
    output
}

fn without_oracle_slash_line(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    let raw = &text[range.clone()];
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        offset += line.len();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim() == "/" {
            return Some(range.start + offset..range.end);
        }
        return None;
    }
    None
}

fn trimmed_located(text: &str, range: Range<usize>) -> Located<'_> {
    let raw = &text[range.clone()];
    let sql = raw.trim();
    let start = range.start + raw.len() - raw.trim_start().len();
    Located {
        sql,
        range: start..start + sql.len(),
    }
}

#[derive(Default)]
struct OracleBlock {
    package: bool,
    begin: usize,
    conditional: usize,
    loop_depth: usize,
    case_depth: usize,
}

impl OracleBlock {
    fn new(first: &str) -> Self {
        Self {
            package: first.to_ascii_uppercase().contains("PACKAGE"),
            ..Self::default()
        }
    }

    fn observe(&mut self, words: &[String]) -> bool {
        let mut bare_end = false;
        let mut index = 0;
        while index < words.len() {
            match words[index].as_str() {
                "BEGIN" => self.begin += 1,
                "IF" => self.conditional += 1,
                "LOOP" => self.loop_depth += 1,
                "CASE" => self.case_depth += 1,
                "END" => match words.get(index + 1).map(String::as_str) {
                    Some("IF") => self.conditional = self.conditional.saturating_sub(1),
                    Some("LOOP") => self.loop_depth = self.loop_depth.saturating_sub(1),
                    Some("CASE") => self.case_depth = self.case_depth.saturating_sub(1),
                    _ => {
                        if self.begin > 0 {
                            self.begin -= 1;
                            bare_end = self.begin == 0;
                        } else {
                            bare_end = true;
                        }
                    }
                },
                _ => {}
            }
            if words[index] == "END"
                && matches!(
                    words.get(index + 1).map(String::as_str),
                    Some("IF" | "LOOP" | "CASE")
                )
            {
                index += 1;
            }
            index += 1;
        }
        bare_end
            && self.begin == 0
            && self.conditional == 0
            && self.loop_depth == 0
            && self.case_depth == 0
    }
}

fn oracle_words(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let mut words = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match (bytes[index], bytes.get(index + 1).copied()) {
            (b'-', Some(b'-')) => {
                index += 2;
                while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                    index += 1;
                }
            }
            (b'/', Some(b'*')) => {
                index += 2;
                let mut depth = 1usize;
                while index < bytes.len() && depth > 0 {
                    match (bytes[index], bytes.get(index + 1).copied()) {
                        (b'/', Some(b'*')) => {
                            depth += 1;
                            index += 2;
                        }
                        (b'*', Some(b'/')) => {
                            depth -= 1;
                            index += 2;
                        }
                        _ => index += 1,
                    }
                }
            }
            (b'\'' | b'"', _) => {
                let quote = bytes[index];
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == quote {
                        index += 1;
                        if bytes.get(index) == Some(&quote) {
                            index += 1;
                        } else {
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
            }
            (ch, _) if ch.is_ascii_alphabetic() || ch == b'_' => {
                let start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                words.push(sql[start..index].to_ascii_uppercase());
            }
            _ => index += sql[index..].chars().next().expect("SQL scalar").len_utf8(),
        }
    }
    words
}

/// Removes an Oracle SQL statement terminator from the driver payload while
/// leaving the editor buffer and PL/SQL block terminators untouched.
pub fn for_driver(sql: &str, dialect: Dialect) -> Cow<'_, str> {
    if dialect != Dialect::Oracle || plsql(sql) {
        return Cow::Borrowed(sql);
    }
    let Some(last) = Scanner::new(sql, Dialect::Standard)
        .filter(|segment| segment.code)
        .last()
    else {
        return Cow::Borrowed(sql);
    };
    if sql.as_bytes().get(last.located.range.end) != Some(&b';') {
        return Cow::Borrowed(sql);
    }
    let mut driver_sql = sql.to_owned();
    driver_sql.remove(last.located.range.end);
    Cow::Owned(driver_sql)
}

fn plsql(sql: &str) -> bool {
    let Some(mut source) = Scanner::new(sql, Dialect::Standard)
        .find(|segment| segment.code)
        .map(|segment| segment.located.sql.trim_start())
    else {
        return false;
    };
    loop {
        if let Some(rest) = source.strip_prefix("--") {
            source = rest
                .find(['\r', '\n'])
                .map_or("", |end| &rest[end + 1..])
                .trim_start();
            continue;
        }
        if let Some(rest) = source.strip_prefix("/*") {
            let Some(end) = rest.find("*/") else {
                return false;
            };
            source = rest[end + 2..].trim_start();
            continue;
        }
        break;
    }
    let words: Vec<String> = source
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .take(8)
        .map(str::to_ascii_uppercase)
        .collect();
    match words.first().map(String::as_str) {
        Some("BEGIN" | "DECLARE") => true,
        Some("CREATE") => {
            let mut index = 1;
            if words.get(index).is_some_and(|word| word == "OR")
                && words.get(index + 1).is_some_and(|word| word == "REPLACE")
            {
                index += 2;
            }
            if words.get(index).is_some_and(|word| {
                matches!(word.as_str(), "EDITIONABLE" | "NONEDITIONABLE" | "FORCE")
            }) {
                index += 1;
            }
            matches!(
                words.get(index).map(String::as_str),
                Some("PACKAGE" | "PROCEDURE" | "FUNCTION" | "TRIGGER")
            ) || (words.get(index).is_some_and(|word| word == "TYPE")
                && words.get(index + 1).is_some_and(|word| word == "BODY"))
        }
        _ => false,
    }
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
