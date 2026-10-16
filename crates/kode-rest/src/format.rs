//! Optional display formatting. Raw request/response bytes remain authoritative.
//! Run this on a worker; invalid/over-budget input stays intact, and mixed-content
//! regions retain their original text.
use kode_platform::jobs::CancellationToken;
use serde::Deserialize;
use std::{
    borrow::Cow,
    ops::{ControlFlow, Range},
};

const INPUT_LIMIT: usize = 10 * 1024 * 1024;
const OUTPUT_LIMIT: usize = 20 * 1024 * 1024;
const NODE_LIMIT: usize = 100_000;
const DEPTH_LIMIT: usize = 96;

pub fn language_for_content_type(content_type: Option<&str>) -> Option<&'static str> {
    let kind = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    Some(match kind.as_str() {
        "application/json" => "json",
        "application/yaml" | "text/yaml" => "yaml",
        "application/javascript" | "text/javascript" | "application/ecmascript" => "javascript",
        "text/html" | "application/xhtml+xml" => "html",
        "application/xml" | "text/xml" => "xml",
        "application/graphql" => "graphql",
        _ if kind.ends_with("+json") => "json",
        _ if kind.ends_with("+yaml") => "yaml",
        _ if kind.ends_with("+xml") => "xml",
        _ => return None,
    })
}

pub fn pretty_print<'a>(
    content: &'a str,
    content_type: Option<&str>,
    enabled: bool,
    cancel: &CancellationToken,
) -> Cow<'a, str> {
    if !enabled || content.len() > INPUT_LIMIT || content.trim().is_empty() || cancel.is_cancelled()
    {
        return Cow::Borrowed(content);
    }
    let formatted = match language_for_content_type(content_type) {
        Some("json" | "graphql") => json(content, cancel),
        Some("xml") => xml(content, cancel),
        Some("html") => html(content, cancel),
        Some("javascript") => javascript(content, cancel),
        // Preserve block scalar text and line endings; trimming YAML lines can
        // change literal/folded scalar values. Only plain separator whitespace
        // outside a possible scalar is eligible for the source's trim operation.
        Some("yaml") => yaml(content, cancel),
        _ => None,
    };
    match formatted {
        Some(text) if text != content => Cow::Owned(text),
        _ => Cow::Borrowed(content),
    }
}
struct Buffer(Vec<u8>);
impl Buffer {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn push(&mut self, text: impl AsRef<[u8]>) -> Option<()> {
        let text = text.as_ref();
        if text.len() > OUTPUT_LIMIT.saturating_sub(self.0.len()) {
            return None;
        }
        self.0.extend_from_slice(text);
        Some(())
    }
    fn line(&mut self, depth: usize, width: usize) -> Option<()> {
        if depth > DEPTH_LIMIT {
            return None;
        }
        self.push(b"\n")?;
        for _ in 0..depth * width {
            self.push(b" ")?;
        }
        Some(())
    }
    fn finish(self) -> Option<String> {
        String::from_utf8(self.0).ok()
    }
}
fn json(content: &str, cancel: &CancellationToken) -> Option<String> {
    let mut parser = serde_json::Deserializer::from_str(content);
    serde::de::IgnoredAny::deserialize(&mut parser).ok()?;
    parser.end().ok()?;
    // Format validated tokens, preserving duplicate keys, number spellings,
    // escapes and order instead of converting through a JSON value map.
    let bytes = content.as_bytes();
    let mut output = Buffer::new();
    let (mut quoted, mut escaped, mut depth, mut previous) = (false, false, 0usize, 0u8);
    for (index, &byte) in bytes.iter().enumerate() {
        if index % 4096 == 0 && cancel.is_cancelled() {
            return None;
        }
        if quoted {
            output.push([byte])?;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
                previous = b'"';
            }
            continue;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        match byte {
            b'"' => {
                quoted = true;
                output.push([byte])?;
            }
            b'{' | b'[' => {
                output.push([byte])?;
                depth += 1;
                if depth > DEPTH_LIMIT {
                    return None;
                }
                let next = bytes[index + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if next != Some(&if byte == b'{' { b'}' } else { b']' }) {
                    output.line(depth, 4)?;
                }
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if previous != if byte == b'}' { b'{' } else { b'[' } {
                    output.line(depth, 4)?;
                }
                output.push([byte])?;
            }
            b',' => {
                output.push([byte])?;
                output.line(depth, 4)?;
            }
            b':' => output.push(b": ")?,
            _ => output.push([byte])?,
        }
        previous = byte;
    }
    output.finish()
}
type Edit = (Range<usize>, String);
fn markup_whitespace(text: &str) -> bool {
    text.bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
}
fn gap_edits(
    content: &str,
    start: usize,
    end: usize,
    children: &[Range<usize>],
    depth: usize,
) -> Option<Vec<Edit>> {
    if depth >= DEPTH_LIMIT || children.is_empty() {
        return None;
    }
    let mut edits = Vec::new();
    let mut at = start;
    for child in children {
        if child.start < at || child.end > end || !markup_whitespace(content.get(at..child.start)?)
        {
            return None;
        }
        edits.push((at..child.start, format!("\n{}", "  ".repeat(depth + 1))));
        at = child.end;
    }
    if !markup_whitespace(content.get(at..end)?) {
        return None;
    }
    edits.push((at..end, format!("\n{}", "  ".repeat(depth))));
    Some(edits)
}
fn apply(content: &str, mut edits: Vec<Edit>, cancel: &CancellationToken) -> Option<String> {
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut output = Buffer::new();
    let mut at = 0;
    for (range, text) in edits {
        if range.start < at || cancel.is_cancelled() {
            return None;
        }
        output.push(content.get(at..range.start)?)?;
        output.push(text)?;
        at = range.end;
    }
    output.push(content.get(at..)?)?;
    output.finish()
}
fn tag_end(content: &str, start: usize, end: usize) -> Option<usize> {
    let mut quote = None;
    for (index, &byte) in content.as_bytes().get(start..end)?.iter().enumerate() {
        if quote == Some(byte) {
            quote = None;
        } else if quote.is_none() {
            if matches!(byte, b'\'' | b'"') {
                quote = Some(byte);
            } else if byte == b'>' {
                return Some(start + index + 1);
            }
        }
    }
    None
}
fn mixed(node: roxmltree::Node<'_, '_>) -> bool {
    node.children().any(|child| child.is_element())
        && node.children().any(|child| {
            child.is_text() && child.text().is_some_and(|text| !markup_whitespace(text))
        })
}
fn xml(content: &str, cancel: &CancellationToken) -> Option<String> {
    let document = roxmltree::Document::parse_with_options(
        content,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: NODE_LIMIT as u32,
            entity_resolver: None,
        },
    )
    .ok()?;
    let mut edits = Vec::new();
    let mut pending = vec![(document.root_element(), 0, false, false)];
    while let Some((node, depth, preserve, ancestor_mixed)) = pending.pop() {
        if depth > DEPTH_LIMIT || cancel.is_cancelled() {
            return None;
        }
        let preserve = match node.attribute(("http://www.w3.org/XML/1998/namespace", "space")) {
            Some("preserve") => true,
            Some("default") => false,
            _ => preserve,
        };
        let is_mixed = ancestor_mixed || mixed(node);
        pending.extend(
            node.children()
                .filter(|child| child.is_element())
                .map(|child| (child, depth + 1, preserve, is_mixed)),
        );
        if preserve || is_mixed {
            continue;
        }
        let children: Vec<_> = node
            .children()
            .filter(|child| !child.is_text())
            .map(|child| child.range())
            .collect();
        if children.is_empty() {
            continue;
        }
        let range = node.range();
        let start = tag_end(content, range.start, range.end)?;
        let end = range.start + content.get(range.clone())?.rfind("</")?;
        if let Some(gaps) = gap_edits(content, start, end, &children, depth) {
            edits.extend(gaps);
        }
    }
    apply(content, edits, cancel)
}
fn parse(
    content: &str,
    language: tree_sitter::Language,
    cancel: &CancellationToken,
) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).ok()?;
    let mut checks = 0usize;
    let mut progress = |_: &tree_sitter::ParseState| {
        checks += 1;
        if checks > NODE_LIMIT || cancel.is_cancelled() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let mut read = |offset: usize, _| &content.as_bytes()[offset..];
    let tree = parser.parse_with_options(
        &mut read,
        None,
        Some(tree_sitter::ParseOptions::new().progress_callback(&mut progress)),
    )?;
    (!tree.root_node().has_error()).then_some(tree)
}
fn same_shape(before: &tree_sitter::Tree, after: &tree_sitter::Tree) -> bool {
    let mut pending = vec![(before.root_node(), after.root_node())];
    let mut count = 0;
    while let Some((before, after)) = pending.pop() {
        count += 1;
        if count > NODE_LIMIT
            || before.kind_id() != after.kind_id()
            || before.child_count() != after.child_count()
        {
            return false;
        }
        let mut left = before.walk();
        let mut right = after.walk();
        pending.extend(before.children(&mut left).zip(after.children(&mut right)));
    }
    true
}
fn html(content: &str, cancel: &CancellationToken) -> Option<String> {
    let tree = parse(content, tree_sitter_html::LANGUAGE.into(), cancel)?;
    let mut pending = vec![(tree.root_node(), 0, false)];
    let mut edits = Vec::new();
    let mut count = 0;
    while let Some((node, depth, blocked)) = pending.pop() {
        count += 1;
        if count > NODE_LIMIT || depth > DEPTH_LIMIT || cancel.is_cancelled() {
            return None;
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        let is_element = node.kind() == "element";
        let blocked = blocked
            || children
                .iter()
                .any(|child| matches!(child.kind(), "text" | "entity" | "raw_text"))
            || children
                .iter()
                .find(|child| child.kind() == "start_tag")
                .is_some_and(|child| {
                    let tag = &content[child.byte_range()];
                    ["pre", "textarea", "script", "style"].iter().any(|name| {
                        tag[1..]
                            .split(|ch: char| ch.is_whitespace() || ch == '>')
                            .next()
                            .is_some_and(|tag| tag.eq_ignore_ascii_case(name))
                    })
                });
        if is_element && !blocked {
            let start = children.iter().find(|child| child.kind() == "start_tag");
            let end = children.iter().find(|child| child.kind() == "end_tag");
            if let (Some(start), Some(end)) = (start, end) {
                let content_children: Vec<_> = children
                    .iter()
                    .filter(|child| {
                        child.start_byte() >= start.end_byte()
                            && child.end_byte() <= end.start_byte()
                    })
                    .map(|child| {
                        let mut range = child.byte_range();
                        // Implicit end tags can own separator whitespace in
                        // this grammar. Keep that gap outside the child so
                        // repeated formatting does not accumulate blank lines.
                        range.end = range.start
                            + content[range.clone()]
                                .trim_end_matches([' ', '\t', '\r', '\n'])
                                .len();
                        range
                    })
                    .collect();
                if let Some(gaps) = gap_edits(
                    content,
                    start.end_byte(),
                    end.start_byte(),
                    &content_children,
                    depth,
                ) {
                    edits.extend(gaps);
                }
            }
        }
        pending.extend(
            children
                .into_iter()
                .rev()
                .map(|child| (child, depth + usize::from(is_element), blocked)),
        );
    }
    let output = apply(content, edits, cancel)?;
    let formatted = parse(&output, tree_sitter_html::LANGUAGE.into(), cancel)?;
    same_shape(&tree, &formatted).then_some(output)
}
fn javascript(content: &str, cancel: &CancellationToken) -> Option<String> {
    let tree = parse(content, tree_sitter_javascript::LANGUAGE.into(), cancel)?;
    let mut pending = vec![tree.root_node()];
    let mut tokens = Vec::new();
    let mut count = 0;
    while let Some(node) = pending.pop() {
        count += 1;
        if count > NODE_LIMIT || cancel.is_cancelled() {
            return None;
        }
        if node.child_count() == 0
            || matches!(
                node.kind(),
                "string"
                    | "template_string"
                    | "regex"
                    | "comment"
                    | "jsx_element"
                    | "jsx_self_closing_element"
            )
        {
            if node.end_byte() > node.start_byte() {
                tokens.push(node.byte_range());
            }
        } else {
            let mut cursor = node.walk();
            pending.extend(
                node.children(&mut cursor)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev(),
            );
        }
    }
    let mut edits = Vec::new();
    let mut depth = 0usize;
    for pair in tokens.windows(2) {
        let token = &content[pair[0].clone()];
        let next = &content[pair[1].clone()];
        if token == "{" {
            depth += 1;
        } else if token == "}" {
            depth = depth.saturating_sub(1);
        }
        if depth > DEPTH_LIMIT {
            return None;
        }
        let gap = pair[0].end..pair[1].start;
        if !content[gap.clone()].trim().is_empty() {
            return None;
        }
        if matches!(token, "{" | ";") || next == "}" {
            let indent = if next == "}" {
                depth.saturating_sub(1)
            } else {
                depth
            };
            edits.push((gap, format!("\n{}", "  ".repeat(indent))));
        } else if token == "," && !content[gap.clone()].contains(['\n', '\r']) {
            edits.push((gap, " ".into()));
        }
    }
    let output = apply(content, edits, cancel)?;
    let formatted = parse(&output, tree_sitter_javascript::LANGUAGE.into(), cancel)?;
    same_shape(&tree, &formatted).then_some(output)
}
fn yaml(content: &str, cancel: &CancellationToken) -> Option<String> {
    // Validate without converting numbers or scalar styles. Quoted multiline
    // scalars and block scalars retain trailing whitespace that belongs to data.
    if content.contains(['|', '>', '\'', '"']) {
        return None;
    }
    let options = serde_saphyr::options! {
        with_snippet:false,
        duplicate_keys:serde_saphyr::options::DuplicateKeyPolicy::Error,
        reject_unsupported_tags:true,
        budget:serde_saphyr::budget! {
            max_depth:96,max_documents:1,max_events:500_000,max_nodes:100_000,
            max_total_scalar_bytes:INPUT_LIMIT,max_aliases:1024,max_anchors:1024,
        },
        alias_limits:serde_saphyr::alias_limits! {
            max_total_replayed_events:100_000,max_replay_stack_depth:32,max_alias_expansions_per_anchor:1024,
        },
    };
    let _: serde::de::IgnoredAny = serde_saphyr::from_str_with_options(content, options).ok()?;
    let mut output = Buffer::new();
    for line in content.split_inclusive('\n') {
        if cancel.is_cancelled() {
            return None;
        }
        let newline = if line.ends_with("\r\n") {
            "\r\n"
        } else if line.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let body = line.strip_suffix(newline).unwrap_or(line);
        output.push(body.trim_end_matches([' ', '\t']))?;
        output.push(newline)?;
    }
    output.finish()
}
