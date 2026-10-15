//! kotlin-sg can discard enclosing declarations around a dangling member dot.
//! Mask only incomplete access operators in parser input, preserving every raw
//! byte position. Snapshots, source text, lexical slices and saved bytes stay raw.
use super::ParseDiagnostic;
use kode_core::{ByteOffset, DomainError, Result, TextRange};
use kode_platform::jobs::CancellationToken;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
pub(super) fn prepare(
    source: Arc<str>,
    cancel: &CancellationToken,
) -> Result<(Arc<str>, Vec<ParseDiagnostic>)> {
    let bytes = source.as_bytes();
    let mut input: Option<Vec<u8>> = None;
    let mut diagnostics = vec![];
    let mut cursor = 0usize;
    let started = Instant::now();
    while cursor < bytes.len() {
        if cursor.is_multiple_of(4096) {
            check(cancel, started)?;
        }
        if bytes[cursor..].starts_with(b"//") {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if bytes[cursor..].starts_with(b"/*") {
            cursor = comment(bytes, cursor, cancel, started)?;
            continue;
        }
        if bytes[cursor..].starts_with(b"\"\"\"") {
            cursor += 3;
            while cursor < bytes.len() && !bytes[cursor..].starts_with(b"\"\"\"") {
                if cursor.is_multiple_of(4096) {
                    check(cancel, started)?;
                }
                cursor += 1;
            }
            cursor = (cursor + 3).min(bytes.len());
            continue;
        }
        if matches!(bytes[cursor], b'\"' | b'\'' | b'`') {
            let quote = bytes[cursor];
            cursor += 1;
            while cursor < bytes.len() {
                if cursor.is_multiple_of(4096) {
                    check(cancel, started)?;
                }
                if bytes[cursor] == quote {
                    cursor += 1;
                    break;
                }
                if bytes[cursor] == b'\\' && quote != b'`' {
                    cursor = (cursor + 2).min(bytes.len());
                } else {
                    cursor += 1;
                }
            }
            continue;
        }
        if bytes[cursor] == b'.'
            && (cursor == 0 || bytes[cursor - 1] != b'.')
            && bytes.get(cursor + 1) != Some(&b'.')
        {
            let next = significant(&source, cursor + 1, cancel, started)?;
            let dangling = next == bytes.len()
                || matches!(bytes.get(next), Some(b'}' | b')' | b']' | b';' | b','))
                || hard_keyword(&source[next..]);
            if dangling {
                let input = input.get_or_insert_with(|| bytes.to_vec());
                input[cursor] = b' ';
                let previous = source[..cursor]
                    .char_indices()
                    .rev()
                    .find(|(_, ch)| !ch.is_whitespace());
                if let Some((index, '?')) = previous {
                    input[index] = b' ';
                }
                if diagnostics.len() < 256 {
                    diagnostics.push(ParseDiagnostic {
                        range: TextRange::new(ByteOffset(cursor), ByteOffset(cursor + 1))?,
                        kind: "incomplete_member_access".into(),
                        missing: false,
                    });
                }
            }
        }
        cursor += 1;
    }
    let input = input.map_or_else(
        || source.clone(),
        |bytes| Arc::from(String::from_utf8(bytes).expect("ASCII masking preserves UTF-8")),
    );
    Ok((input, diagnostics))
}
fn check(cancel: &CancellationToken, started: Instant) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    if started.elapsed() > Duration::from_secs(2) {
        return Err(DomainError::Unsupported(
            "Kotlin recovery preparation budget exceeded".into(),
        ));
    }
    Ok(())
}
fn comment(
    bytes: &[u8],
    mut cursor: usize,
    cancel: &CancellationToken,
    started: Instant,
) -> Result<usize> {
    let mut depth = 1usize;
    cursor += 2;
    while cursor < bytes.len() && depth > 0 {
        if cursor.is_multiple_of(4096) {
            check(cancel, started)?;
        }
        if bytes[cursor..].starts_with(b"/*") {
            depth += 1;
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"*/") {
            depth -= 1;
            cursor += 2;
        } else {
            cursor += 1;
        }
    }
    Ok(cursor)
}
fn significant(
    source: &str,
    mut cursor: usize,
    cancel: &CancellationToken,
    started: Instant,
) -> Result<usize> {
    while cursor < source.len() {
        check(cancel, started)?;
        let bytes = source.as_bytes();
        if bytes[cursor..].starts_with(b"/*") {
            cursor = comment(bytes, cursor, cancel, started)?;
            continue;
        }
        if bytes[cursor..].starts_with(b"//") {
            cursor += 2;
            while cursor < bytes.len() && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        let ch = source[cursor..].chars().next().expect("cursor before EOF");
        if !ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    Ok(cursor)
}
fn hard_keyword(text: &str) -> bool {
    let word = text
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .next()
        .unwrap_or_default();
    matches!(
        word,
        "class"
            | "fun"
            | "val"
            | "var"
            | "object"
            | "interface"
            | "package"
            | "import"
            | "return"
            | "throw"
            | "if"
            | "else"
            | "when"
            | "while"
            | "for"
            | "try"
            | "catch"
            | "finally"
    )
}
