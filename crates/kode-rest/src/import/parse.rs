use super::{Import, invalid};
use kode_core::Result;
use serde_json::Value;
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Auto,
    Json,
    Yaml,
}

pub fn parse(text: &str, format: Format) -> Result<Import> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(invalid("/", "document exceeds the 16 MiB import limit"));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let json = format == Format::Json
        || (format == Format::Auto && text.trim_start().starts_with(['{', '[']));
    let document: Value = if json {
        serde_json::from_str(text).map_err(|error| {
            invalid(
                "/",
                &format!(
                    "invalid JSON at line {}, column {}",
                    error.line(),
                    error.column()
                ),
            )
        })?
    } else {
        let options = serde_saphyr::options! {
            with_snippet: false,
            duplicate_keys: serde_saphyr::options::DuplicateKeyPolicy::Error,
            reject_unsupported_tags: true,
            strict_booleans: true,
            budget: serde_saphyr::budget! {
                max_depth: 96,
                max_documents: 1,
                max_events: 1_000_000,
                max_nodes: 500_000,
                max_total_scalar_bytes: MAX_DOCUMENT_BYTES,
                max_aliases: 1024,
                max_anchors: 1024,
            },
            alias_limits: serde_saphyr::alias_limits! {
                max_total_replayed_events: 100_000,
                max_replay_stack_depth: 32,
                max_alias_expansions_per_anchor: 1024,
            },
        };
        let tree = serde_saphyr::from_str_with_options::<serde_saphyr::Spanned<super::yaml::Node>>(
            text, options,
        )
        .map_err(|error| {
            let location = error
                .location()
                .map(|p| format!(" at line {}, column {}", p.line(), p.column()))
                .unwrap_or_default();
            invalid(
                "/",
                &format!("invalid YAML or parsing limit exceeded{location}"),
            )
        })?;
        super::yaml::into_json(tree, text)?
    };
    Import::from_document(document)
}

// Bound metadata depth too, not just the collection's visible item tree. Leave
// headroom for project/session wrappers under serde_json's default recursion cap.
pub(super) fn validate_document(document: &Value) -> Result<()> {
    let mut pending = vec![(document, 0)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > 96 || nodes > 500_000 {
            return Err(invalid(
                "/",
                "document exceeds 96 JSON levels or 500,000 values",
            ));
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|v| (v, depth + 1))),
            Value::Object(values) => pending.extend(values.values().map(|v| (v, depth + 1))),
            _ => {}
        }
    }
    encoded_size(document)?;
    Ok(())
}
pub(super) fn encoded_size(document: &Value) -> Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_DOCUMENT_BYTES.saturating_sub(self.0) {
                return Err(std::io::Error::other("REST document size limit"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, document)
        .map_err(|_| invalid("/", "converted document exceeds the 16 MiB limit"))?;
    Ok(counter.0)
}
