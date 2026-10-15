//! Source keyword/color resources and markup/CSS special cases, in byte units.
use crate::model::{LineToken, PatternDiagnostic, SyntaxSpan};
use kode_core::{
    ByteOffset, DomainError, LineIndex, Result, TextRange, document::TextSnapshot,
    editor::search_regex::SearchRegex,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
const MATCH_LIMIT: usize = 100_000;
struct Entry {
    pattern: SearchRegex,
    keywords: Vec<String>,
}
struct Rules {
    tag: SearchRegex,
    processing: SearchRegex,
    attribute: SearchRegex,
    value: SearchRegex,
    css: SearchRegex,
}
pub struct KeywordProvider {
    entries: BTreeMap<String, Entry>,
    colors: BTreeMap<String, [u8; 3]>,
    pub diagnostics: Vec<PatternDiagnostic>,
    rules: Rules,
}
fn rgb(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    let bits = u32::from_str_radix(value, 16).ok()?;
    Some([(bits >> 16) as u8, (bits >> 8) as u8, bits as u8])
}
fn normalize(language: &str) -> String {
    let key = language.trim().trim_start_matches('.').to_ascii_lowercase();
    match key.as_str() {
        "kt" | "kts" => "kotlin",
        "js" => "javascript",
        "ts" => "typescript",
        "c+" => "cplus",
        "c++" | "cc" | "cxx" | "hpp" | "hxx" => "cpp",
        "sh" | "shellscript" => "bash",
        "py" => "python",
        "rb" => "ruby",
        "golang" => "go",
        "htm" => "html",
        _ => &key,
    }
    .to_owned()
}
impl KeywordProvider {
    pub fn compiled() -> Result<Self> {
        Self::from_resources(
            include_str!("../../../resources/keyword-patterns.txt"),
            include_str!("../../../resources/token-colors.txt"),
        )
    }
    pub fn from_resources(patterns: &str, colors: &str) -> Result<Self> {
        let mut entries = BTreeMap::new();
        let mut diagnostics = vec![];
        for (index, line) in patterns.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((language, pattern)) = line.split_once('=') else {
                diagnostics.push(PatternDiagnostic {
                    source: "keyword-patterns.txt".into(),
                    line: index + 1,
                    language: String::new(),
                    message: "expected language=pattern".into(),
                });
                continue;
            };
            let language = language.trim().to_ascii_lowercase();
            let pattern = pattern.trim();
            match SearchRegex::compile(pattern) {
                Ok(pattern_compiled) => {
                    entries.insert(
                        language,
                        Entry {
                            pattern: pattern_compiled,
                            keywords: extract_keywords(pattern),
                        },
                    );
                }
                Err(error) => diagnostics.push(PatternDiagnostic {
                    source: "keyword-patterns.txt".into(),
                    line: index + 1,
                    language,
                    message: error.to_string(),
                }),
            }
        }
        let mut palette = BTreeMap::new();
        for line in colors
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            if let Some((name, value)) = line.split_once('=')
                && let Some(color) = rgb(value)
            {
                palette.insert(name.trim().to_ascii_lowercase(), color);
            }
        }
        Ok(Self {
            entries,
            colors: palette,
            diagnostics,
            rules: Rules {
                tag: SearchRegex::compile(
                    r#"<\s*/?\s*(?<name>[A-Za-z_][A-Za-z0-9_.:-]*)(?<body>[^>]*)(?:>|$)"#,
                )?,
                processing: SearchRegex::compile(
                    r#"<\?\s*(?<name>[A-Za-z_][A-Za-z0-9_.:-]*)(?<body>[^>]*)\?>"#,
                )?,
                attribute: SearchRegex::compile(r"\b(?<name>[A-Za-z_:][A-Za-z0-9_.:-]*)(?=\s*=)")?,
                value: SearchRegex::compile(
                    r#"=\s*(?<value>\"(?:[^\"\\]|\\.)*\"|'(?:[^'\\]|\\.)*')"#,
                )?,
                css: SearchRegex::compile(r"(?<![A-Za-z0-9_-])\.[A-Za-z_-][A-Za-z0-9_-]*")?,
            },
        })
    }
    pub fn languages(&self) -> BTreeSet<&str> {
        self.entries.keys().map(String::as_str).collect()
    }
    pub fn keywords(&self, language: &str) -> &[String] {
        self.entries
            .get(&normalize(language))
            .map_or(&[], |entry| &entry.keywords)
    }
    pub fn is_keyword(&self, language: &str, word: &str) -> Result<bool> {
        if word.trim().is_empty() {
            return Ok(false);
        }
        self.entries
            .get(&normalize(language))
            .map_or(Ok(false), |entry| entry.pattern.is_match(word))
    }
    pub fn color(&self, class: &str) -> [u8; 3] {
        self.colors
            .get(&class.to_ascii_lowercase())
            .or_else(|| self.colors.get("default"))
            .copied()
            .unwrap_or([0xcc, 0x78, 0x32])
    }
    pub fn tokens_for_lines(
        &self,
        start: LineIndex,
        lines: &[&str],
        language: &str,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<LineToken>> {
        let language = normalize(language);
        let mut tokens = vec![];
        for (offset, text) in lines.iter().enumerate() {
            if cancelled() {
                return Err(DomainError::Cancelled);
            }
            let line =
                LineIndex(start.0.checked_add(offset).ok_or_else(|| {
                    DomainError::Unsupported("syntax line index overflow".into())
                })?);
            if matches!(
                language.as_str(),
                "html" | "html-derivative" | "xml" | "xsl" | "jsx" | "tsx" | "vue-html"
            ) {
                self.markup(line, text, &language, &mut tokens)?;
            } else {
                if matches!(language.as_str(), "css" | "scss" | "less") {
                    self.rules.css.for_each(text, |range| {
                        push(
                            &mut tokens,
                            line,
                            text,
                            range,
                            "css.class",
                            [0x98, 0x76, 0xaa],
                        )
                    })?;
                }
                if let Some(entry) = self.entries.get(&language) {
                    entry.pattern.for_each(text, |range| {
                        if range.start.0 >= text.len() {
                            return Ok(());
                        }
                        push(
                            &mut tokens,
                            line,
                            text,
                            range,
                            "keyword",
                            self.color("keyword"),
                        )
                    })?;
                }
            }
            if tokens.len() > MATCH_LIMIT {
                return Err(DomainError::Unsupported(
                    "syntax token limit exceeded".into(),
                ));
            }
        }
        tokens.sort_by_key(|token| (token.line, token.line_range.start, token.line_range.end));
        Ok(tokens)
    }
    fn markup(
        &self,
        line: LineIndex,
        text: &str,
        language: &str,
        tokens: &mut Vec<LineToken>,
    ) -> Result<()> {
        let mut tags = vec![];
        for pattern in [&self.rules.processing, &self.rules.tag] {
            pattern.for_each_named(text, &["name", "body"], |whole, groups| {
                if let Some(name) = groups[0] {
                    push(tokens, line, text, name, "tag", [0xe8, 0xbf, 0x6a])?;
                }
                if let Some(body) = groups[1]
                    && !body.is_empty()
                {
                    let source = &text[body.start.0..body.end.0];
                    self.rules
                        .attribute
                        .for_each_named(source, &["name"], |_, groups| {
                            if let Some(name) = groups[0] {
                                let name = shift(name, body.start.0)?;
                                let class_name = &text[name.start.0..name.end.0];
                                let class = if class_name.eq_ignore_ascii_case("class")
                                    || class_name.eq_ignore_ascii_case("className")
                                {
                                    "css.class"
                                } else {
                                    "attribute"
                                };
                                push(
                                    tokens,
                                    line,
                                    text,
                                    name,
                                    class,
                                    if class == "css.class" {
                                        [0x98, 0x76, 0xaa]
                                    } else {
                                        [0xa5, 0xc2, 0x61]
                                    },
                                )?;
                            }
                            Ok(())
                        })?;
                    self.rules
                        .value
                        .for_each_named(source, &["value"], |_, groups| {
                            if let Some(value) = groups[0] {
                                push(
                                    tokens,
                                    line,
                                    text,
                                    shift(value, body.start.0)?,
                                    "string",
                                    [0x6a, 0x87, 0x59],
                                )?;
                            }
                            Ok(())
                        })?;
                }
                tags.push(whole);
                Ok(())
            })?;
        }
        if matches!(language, "jsx" | "tsx")
            && let Some(entry) = self.entries.get(language)
        {
            entry.pattern.for_each(text, |range| {
                if tags.iter().all(|tag| !tag.contains(range.start)) {
                    push(tokens, line, text, range, "keyword", self.color("keyword"))?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    pub fn highlight(
        &self,
        snapshot: &TextSnapshot,
        language: &str,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<SyntaxSpan>> {
        let mut spans = vec![];
        for line in 0..snapshot.len_lines() {
            if cancelled() {
                return Err(DomainError::Cancelled);
            }
            let text = snapshot.line_content(LineIndex(line))?.to_string();
            let start = snapshot.line_start(LineIndex(line))?.0;
            for token in
                self.tokens_for_lines(LineIndex(line), &[&text], language, &mut cancelled)?
            {
                spans.push(SyntaxSpan {
                    range: shift(token.line_range, start)?,
                    class: token.class,
                    color: token.color,
                });
                if spans.len() > MATCH_LIMIT {
                    return Err(DomainError::Unsupported(
                        "syntax token limit exceeded".into(),
                    ));
                }
            }
        }
        Ok(spans)
    }
}
fn push(
    tokens: &mut Vec<LineToken>,
    line: LineIndex,
    text: &str,
    range: TextRange,
    class: &str,
    color: [u8; 3],
) -> Result<()> {
    range.validate(text)?;
    if tokens.len() >= MATCH_LIMIT {
        return Err(DomainError::Unsupported(
            "syntax token limit exceeded".into(),
        ));
    }
    tokens.push(LineToken {
        line,
        line_range: range,
        class: class.into(),
        text: text[range.start.0..range.end.0].into(),
        color: Some(color),
    });
    Ok(())
}
fn shift(range: TextRange, start: usize) -> Result<TextRange> {
    let add = |value: usize| {
        value
            .checked_add(start)
            .map(ByteOffset)
            .ok_or_else(|| DomainError::Unsupported("syntax byte offset overflow".into()))
    };
    TextRange::new(add(range.start.0)?, add(range.end.0)?)
}
fn extract_keywords(pattern: &str) -> Vec<String> {
    let groups = regex::Regex::new(r"\(([^()]*)\)").expect("fixed group scanner");
    let groups: Vec<_> = groups
        .captures_iter(pattern)
        .map(|capture| capture[1].to_owned())
        .collect();
    let clean = |part: &str| part.replace(r"\b", "").replace('\\', "").trim().to_owned();
    let selected = groups
        .iter()
        .find(|body| {
            body.contains('|')
                && body.split('|').any(|part| {
                    let candidate = clean(part);
                    !candidate.is_empty() && !matches!(candidate.as_str(), "^" | "$")
                })
        })
        .or_else(|| groups.first());
    selected.map_or_else(Vec::new, |body| {
        body.split('|')
            .map(clean)
            .filter(|word| !word.is_empty())
            .collect()
    })
}
/// Read-only providers are shared across bounded parse/highlight workers.
pub type SharedKeywords = Arc<KeywordProvider>;
