//! Optional ordered regex grammars. Resource loading is a worker-side operation.
use crate::model::{LineToken, PatternDiagnostic};
use indexmap::IndexMap;
use kode_core::{DomainError, LineIndex, Result, editor::search_regex::SearchRegex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegexGrammarDefinition {
    pub language: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub tokens: IndexMap<String, String>,
}
#[derive(Debug, Default, Deserialize)]
struct GrammarIndex {
    #[serde(default)]
    languages: Vec<String>,
}
#[derive(Debug, Default)]
pub struct LoadedGrammars {
    pub definitions: Vec<RegexGrammarDefinition>,
    pub diagnostics: Vec<PatternDiagnostic>,
}
fn io(path: &Path, operation: &'static str, source: std::io::Error) -> DomainError {
    DomainError::Io {
        path: path.into(),
        operation,
        source,
    }
}
pub fn load_directory(root: &Path) -> Result<LoadedGrammars> {
    if !root.is_dir() {
        return Err(DomainError::MissingResource(root.display().to_string()));
    }
    let mut loaded = LoadedGrammars::default();
    let index = root.join("index.json");
    let mut names = match std::fs::read(&index) {
        Ok(bytes) => match serde_json::from_slice::<GrammarIndex>(&bytes) {
            Ok(index) => index.languages,
            Err(error) => {
                loaded.diagnostics.push(PatternDiagnostic {
                    source: index.display().to_string(),
                    line: 0,
                    language: String::new(),
                    message: error.to_string(),
                });
                vec![]
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(source) => return Err(io(&index, "read grammar index", source)),
    };
    if names.is_empty() {
        for entry in
            std::fs::read_dir(root).map_err(|source| io(root, "list grammar directory", source))?
        {
            let entry = entry.map_err(|source| io(root, "read grammar directory entry", source))?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && path.file_name().is_some_and(|name| name != "index.json")
                && path.is_file()
            {
                names.push(
                    path.file_stem()
                        .expect("JSON filename stem")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        names.sort();
    }
    for language in names {
        // An index names sibling definitions; it is never a path traversal API.
        if language.is_empty()
            || Path::new(&language)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
            || language.contains(['/', '\\'])
        {
            loaded.diagnostics.push(PatternDiagnostic {
                source: index.display().to_string(),
                line: 0,
                language,
                message: "grammar name must be a single filename component".into(),
            });
            continue;
        }
        let path = root.join(format!("{language}.json"));
        let bytes =
            std::fs::read(&path).map_err(|source| io(&path, "read regex grammar", source))?;
        match serde_json::from_slice(&bytes) {
            Ok(definition) => loaded.definitions.push(definition),
            Err(error) => loaded.diagnostics.push(PatternDiagnostic {
                source: path.display().to_string(),
                line: error.line(),
                language,
                message: error.to_string(),
            }),
        }
    }
    Ok(loaded)
}
struct Compiled {
    pattern: SearchRegex,
    groups: Vec<(String, String)>,
}
pub struct RegexProvider {
    grammars: BTreeMap<String, Compiled>,
    extensions: BTreeMap<String, String>,
    pub diagnostics: Vec<PatternDiagnostic>,
}
impl RegexProvider {
    pub fn new(definitions: impl IntoIterator<Item = RegexGrammarDefinition>) -> Result<Self> {
        let mut provider = Self {
            grammars: BTreeMap::new(),
            extensions: BTreeMap::from([("txt".into(), "text".into())]),
            diagnostics: vec![],
        };
        provider.add_definition(RegexGrammarDefinition {
            language: "text".into(),
            extensions: vec!["txt".into()],
            tokens: IndexMap::from([(
                "punctuation".into(),
                r#"[.,=+\[\]{}()#$%^&*!@'"\\|/]"#.into(),
            )]),
        });
        for definition in definitions {
            provider.add_definition(definition);
        }
        Ok(provider)
    }
    pub fn add_definition(&mut self, definition: RegexGrammarDefinition) {
        self.grammars.remove(&definition.language);
        for extension in &definition.extensions {
            self.extensions.insert(
                extension.trim_start_matches('.').to_ascii_lowercase(),
                definition.language.clone(),
            );
        }
        let mut groups = vec![];
        let mut parts = vec![];
        for (qualifier, pattern) in definition.tokens {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            let name = format!("token{}", groups.len());
            let pattern = if pattern.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                format!(r"\b(?:{pattern})\b")
            } else {
                pattern.into()
            };
            parts.push(format!("(?<{name}>{pattern})"));
            groups.push((name, qualifier));
        }
        if parts.is_empty() {
            return;
        }
        match SearchRegex::compile(&format!("({})", parts.join("|"))) {
            Ok(pattern) => {
                self.grammars
                    .insert(definition.language, Compiled { pattern, groups });
            }
            Err(error) => self.diagnostics.push(PatternDiagnostic {
                source: "regex grammar".into(),
                line: 0,
                language: definition.language,
                message: error.to_string(),
            }),
        }
    }
    pub fn languages(&self) -> BTreeSet<&str> {
        self.grammars.keys().map(String::as_str).collect()
    }
    pub fn language_for_extension(&self, extension: &str) -> Option<&str> {
        self.extensions
            .get(&extension.trim_start_matches('.').to_ascii_lowercase())
            .map(String::as_str)
    }
    pub fn tokens_for_lines(
        &self,
        start: LineIndex,
        lines: &[&str],
        language: &str,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Vec<LineToken>> {
        let Some(grammar) = self.grammars.get(language) else {
            return Ok(vec![]);
        };
        let names: Vec<_> = grammar
            .groups
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        let mut tokens = vec![];
        for (line, text) in lines.iter().enumerate() {
            if cancelled() {
                return Err(DomainError::Cancelled);
            }
            grammar.pattern.for_each_named(text, &names, |_, groups| {
                if cancelled() {
                    return Err(DomainError::Cancelled);
                }
                if tokens.len() >= 100_000 {
                    return Err(DomainError::Unsupported(
                        "regex grammar token limit exceeded".into(),
                    ));
                }
                let Some((index, range)) = groups
                    .iter()
                    .enumerate()
                    .find_map(|(index, range)| range.map(|range| (index, range)))
                else {
                    return Ok(());
                };
                range.validate(text)?;
                let class = grammar.groups[index].1.clone();
                tokens.push(LineToken {
                    line: LineIndex(start.0.checked_add(line).ok_or_else(|| {
                        DomainError::Unsupported("syntax line index overflow".into())
                    })?),
                    line_range: range,
                    text: text[range.start.0..range.end.0].into(),
                    color: color_for_qualifier(&class),
                    class,
                });
                Ok(())
            })?;
        }
        Ok(tokens)
    }
}
pub fn color_for_qualifier(qualifier: &str) -> Option<[u8; 3]> {
    let lower = qualifier.to_ascii_lowercase();
    let contains = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let hex = if contains(&["comment"]) {
        0x808080
    } else if contains(&["string"]) {
        0x6a8759
    } else if contains(&["regex"]) {
        0xc6794c
    } else if contains(&["number", "numeric"]) {
        0x6897bb
    } else if contains(&["keyword", "boolean"]) {
        0xcc7832
    } else if contains(&["constant"]) {
        0x9876aa
    } else if contains(&["annotation", "decorator"]) {
        0xbbb529
    } else if contains(&["operator", "punctuation", "delimiter", "brace", "bracket"]) {
        0xa9b7c6
    } else if contains(&["function", "method"]) {
        0xffc66d
    } else if contains(&["variable", "identifier"]) {
        0xa9b7c6
    } else if contains(&["attribute", "property"]) {
        0xa5c261
    } else if contains(&["tag", "element"]) {
        0xe8bf6a
    } else if contains(&[
        "type",
        "class",
        "interface",
        "enum",
        "namespace",
        "module",
        "package",
    ]) {
        0xa9b7c6
    } else {
        return None;
    };
    Some([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8])
}
