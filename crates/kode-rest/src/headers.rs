//! Source HTTP field catalog and header-editor diagnostics/completion.
use std::{
    collections::{BTreeSet, HashMap},
    ops::Range,
};

pub const NAMES: &[&str] = &[
    "Accept",
    "Accept-Charset",
    "Accept-Encoding",
    "Accept-Language",
    "Accept-Ranges",
    "Access-Control-Allow-Credentials",
    "Access-Control-Allow-Headers",
    "Access-Control-Allow-Methods",
    "Access-Control-Allow-Origin",
    "Access-Control-Expose-Headers",
    "Access-Control-Max-Age",
    "Access-Control-Request-Headers",
    "Access-Control-Request-Method",
    "Age",
    "Allow",
    "Alt-Svc",
    "Authorization",
    "Cache-Control",
    "Connection",
    "Content-Disposition",
    "Content-Encoding",
    "Content-Language",
    "Content-Length",
    "Content-Location",
    "Content-Range",
    "Content-Security-Policy",
    "Content-Type",
    "Cookie",
    "Date",
    "DNT",
    "ETag",
    "Expect",
    "Forwarded",
    "From",
    "Host",
    "If-Match",
    "If-Modified-Since",
    "If-None-Match",
    "If-Range",
    "If-Unmodified-Since",
    "Keep-Alive",
    "Last-Modified",
    "Link",
    "Location",
    "Origin",
    "Pragma",
    "Proxy-Authenticate",
    "Proxy-Authorization",
    "Range",
    "Referer",
    "Referrer-Policy",
    "Retry-After",
    "Sec-WebSocket-Accept",
    "Sec-WebSocket-Key",
    "Sec-WebSocket-Protocol",
    "Sec-WebSocket-Version",
    "Server",
    "Set-Cookie",
    "TE",
    "Trailer",
    "Transfer-Encoding",
    "Upgrade",
    "User-Agent",
    "Vary",
    "Via",
    "WWW-Authenticate",
    "Warning",
    "X-Forwarded-For",
    "X-Forwarded-Host",
    "X-Forwarded-Proto",
    "X-Request-ID",
    "X-Robots-Tag",
];

pub struct Completion {
    /// Byte range before the caret, including hyphens in the field name.
    pub prefix: Range<usize>,
    pub names: Vec<&'static str>,
}
/// Suggest names only while editing the field name, excluding names on other
/// active lines. Disabled rows can coexist with an active replacement.
pub fn complete(text: &str, cursor: usize) -> Option<Completion> {
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = text.get(..cursor)?;
    let start = before.rfind('\n').map_or(0, |at| at + 1);
    let line = before[start..].trim_start();
    let prefix = line.strip_prefix('#').unwrap_or(line).trim_start();
    if !prefix
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let line_index = before.bytes().filter(|byte| *byte == b'\n').count();
    let used: BTreeSet<_> = text
        .lines()
        .enumerate()
        .filter(|(index, _)| *index != line_index)
        .filter_map(|(_, line)| {
            let name = line.split_once(':')?.0.trim();
            (!name.starts_with('#')).then(|| name.to_ascii_lowercase())
        })
        .collect();
    let search = prefix.to_ascii_lowercase();
    Some(Completion {
        prefix: cursor - prefix.len()..cursor,
        names: NAMES
            .iter()
            .copied()
            .filter(|name| {
                name.to_ascii_lowercase().starts_with(&search)
                    && !used.contains(&name.to_ascii_lowercase())
            })
            .collect(),
    })
}
/// Source behavior: warn on every previous occurrence of a repeated name. The
/// diagnostic includes disabled rows. Resolution separately applies its disabled
/// and override rules. Line indices are zero based.
pub fn duplicate_lines(text: &str) -> Vec<usize> {
    let mut seen = HashMap::new();
    let mut warnings = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let Some((name, _)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().strip_prefix('#').unwrap_or(name.trim()).trim();
        if name.is_empty() {
            continue;
        }
        if let Some(previous) = seen.insert(name.to_ascii_lowercase(), index) {
            warnings.insert(previous);
        }
    }
    warnings.into_iter().collect()
}
