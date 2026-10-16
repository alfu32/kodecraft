//! Bounded byte patterns and cancellable linear-time search; never decodes data.
use kode_core::{DomainError, RequestId, Result, document::bytes::ByteBuffer};
use kode_platform::jobs::CancellationToken;
use std::ops::Range;

pub const MAX_PATTERN_BYTES: usize = 4096;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Text,
    Hex,
}
impl SearchMode {
    pub fn pattern(self, input: &str) -> std::result::Result<Vec<u8>, &'static str> {
        let bytes = match self {
            Self::Text => input.as_bytes().to_vec(),
            Self::Hex => {
                let digits: Vec<_> = input
                    .bytes()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect();
                if digits.iter().any(|byte| !byte.is_ascii_hexdigit()) {
                    return Err("Use hex bytes: 00 FF");
                }
                if digits.len() % 2 != 0 {
                    return Err("Odd hex digit count");
                }
                digits
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| {
                        let digit = |byte: u8| {
                            if byte.is_ascii_digit() {
                                byte - b'0'
                            } else {
                                byte.to_ascii_lowercase() - b'a' + 10
                            }
                        };
                        digit(pair[0]) * 16 + digit(pair[1])
                    })
                    .collect()
            }
        };
        if bytes.is_empty() {
            return Err("Enter a pattern");
        }
        if bytes.len() > MAX_PATTERN_BYTES {
            return Err("Pattern too long");
        }
        Ok(bytes)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Next,
    Previous,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReport {
    pub range: Option<Range<usize>>,
    pub wrapped: bool,
}
pub struct SearchJob {
    pub request: RequestId,
    pub source: ByteBuffer,
    pub pattern: Vec<u8>,
    pub from: usize,
    pub direction: Direction,
    pub cancel: CancellationToken,
}
impl SearchJob {
    pub fn run(self, scheduler: &CancellationToken) -> Result<SearchReport> {
        find(
            self.source.bytes(),
            &self.pattern,
            self.from,
            self.direction,
            || self.cancel.is_cancelled() || scheduler.is_cancelled(),
        )
    }
}
/// Return one next/previous match, including overlapping matches and wraparound.
/// KMP bounds work even for repetitive data/patterns; memory depends only on the
/// pattern size. Cancellation is checked before work and every 4096 input bytes.
pub fn find(
    bytes: &[u8],
    pattern: &[u8],
    from: usize,
    direction: Direction,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SearchReport> {
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_BYTES {
        return Err(DomainError::Unsupported(
            "Invalid binary search pattern length".into(),
        ));
    }
    let mut prefix = vec![0; pattern.len()];
    for index in 1..pattern.len() {
        let mut length = prefix[index - 1];
        while length > 0 && pattern[index] != pattern[length] {
            length = prefix[length - 1];
        }
        if pattern[index] == pattern[length] {
            length += 1;
        }
        prefix[index] = length;
    }
    let from = from.min(bytes.len());
    let (mut first, mut previous, mut last) = (None, None, None);
    let mut length = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if index % 4096 == 0 && cancelled() {
            return Err(DomainError::Cancelled);
        }
        while length > 0 && *byte != pattern[length] {
            length = prefix[length - 1];
        }
        if *byte == pattern[length] {
            length += 1;
        }
        if length == pattern.len() {
            let start = index + 1 - length;
            if direction == Direction::Next && start >= from {
                return Ok(SearchReport {
                    range: Some(start..start + length),
                    wrapped: false,
                });
            }
            first.get_or_insert(start);
            last = Some(start);
            if start < from {
                previous = Some(start);
            }
            length = prefix[length - 1];
        }
    }
    if cancelled() {
        return Err(DomainError::Cancelled);
    }
    let (start, wrapped) = match direction {
        Direction::Next => (first, first.is_some()),
        Direction::Previous => (previous.or(last), previous.is_none() && last.is_some()),
    };
    Ok(SearchReport {
        range: start.map(|start| start..start + pattern.len()),
        wrapped,
    })
}
