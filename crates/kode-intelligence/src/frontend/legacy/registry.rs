use crate::model::SymbolKind;
use kode_core::{DomainError, Result, editor::search_regex::SearchRegex};
use kode_platform::{files::open_regular_file, paths::ResourcePaths};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
const BUNDLED: &str = include_str!("../../../../../resources/codeintel/definitions.json");
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Bundle {
    languages: Vec<Config>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    language: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    line_comments: Vec<String>,
    block_comment_start: Option<String>,
    block_comment_end: Option<String>,
    #[serde(default)]
    block_mode: BlockMode,
    begin_block_regex: Option<String>,
    end_block_regex: Option<String>,
    #[serde(default)]
    patterns: Vec<Pattern>,
}
#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(super) enum BlockMode {
    #[default]
    Brace,
    Indent,
    Paren,
    Regex,
    Sql,
    None,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pattern {
    regex: String,
    kind: String,
    #[serde(default = "one")]
    group_index: usize,
}
fn one() -> usize {
    1
}
pub(super) struct DefinitionPattern {
    pub regex: SearchRegex,
    pub kind: SymbolKind,
    pub group: usize,
}
pub(super) struct LanguageConfig {
    pub line_comments: Vec<String>,
    pub block_start: Option<String>,
    pub block_end: Option<String>,
    pub mode: BlockMode,
    pub begin: Option<SearchRegex>,
    pub end: Option<SearchRegex>,
    pub patterns: Vec<DefinitionPattern>,
}
pub struct DefinitionRegistry {
    configs: BTreeMap<String, LanguageConfig>,
    aliases: BTreeMap<String, String>,
    defaults: LanguageConfig,
    pub origin: Option<PathBuf>,
    pub diagnostics: Vec<String>,
}
impl DefinitionRegistry {
    pub fn compiled() -> Result<Self> {
        Self::from_json(BUNDLED)
    }
    pub fn from_json(text: &str) -> Result<Self> {
        if text.len() > MAX_BYTES {
            return Err(invalid("definition catalog exceeds 1 MiB"));
        }
        let bundle: Bundle = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if bundle.languages.is_empty() || bundle.languages.len() > 512 {
            return Err(invalid("expected 1..512 language definitions"));
        }
        let mut diagnostics = vec![];
        let mut configs = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        for cfg in bundle.languages {
            let name = cfg.language.trim().to_lowercase();
            if name.is_empty() || cfg.patterns.len() > 4096 || cfg.aliases.len() > 512 {
                return Err(invalid("invalid language definition size/name"));
            }
            let mut patterns = vec![];
            for p in cfg.patterns {
                match compile_pattern(p) {
                    Ok(p) => patterns.push(p),
                    Err(e) => diagnostics.push(format!("{name}: {e}")),
                }
            }
            if patterns.is_empty() {
                patterns = default_patterns()?;
            }
            let mut compile_block = |raw: Option<String>| {
                raw.and_then(|s| match SearchRegex::compile(&format!("(?i:{s})")) {
                    Ok(re) => Some(re),
                    Err(e) => {
                        diagnostics.push(format!("{name}: {e}"));
                        None
                    }
                })
            };
            let begin = compile_block(cfg.begin_block_regex);
            let end = compile_block(cfg.end_block_regex);
            for alias in cfg.aliases {
                aliases.insert(
                    kode_syntax::registry::canonical_language(&alias),
                    name.clone(),
                );
                aliases.insert(alias.to_lowercase(), name.clone());
            }
            configs.insert(
                name,
                LanguageConfig {
                    line_comments: if cfg.line_comments.is_empty() {
                        vec!["//".into(), "#".into(), "--".into()]
                    } else {
                        cfg.line_comments
                            .into_iter()
                            .filter(|s| !s.is_empty())
                            .collect()
                    },
                    block_start: cfg
                        .block_comment_start
                        .filter(|s| !s.is_empty())
                        .or_else(|| Some("/*".into())),
                    block_end: cfg
                        .block_comment_end
                        .filter(|s| !s.is_empty())
                        .or_else(|| Some("*/".into())),
                    mode: cfg.block_mode,
                    begin,
                    end,
                    patterns,
                },
            );
        }
        Ok(Self {
            configs,
            aliases,
            defaults: LanguageConfig {
                line_comments: vec!["//".into(), "#".into(), "--".into()],
                block_start: Some("/*".into()),
                block_end: Some("*/".into()),
                mode: BlockMode::Brace,
                begin: None,
                end: None,
                patterns: default_patterns()?,
            },
            origin: None,
            diagnostics,
        })
    }
    /// Worker-only resource loading. The caller supplies KODE_CODEINTEL_PATH as
    /// `explicit`, avoiding process environment mutation in tests.
    pub fn load(paths: &ResourcePaths, explicit: Option<&Path>) -> Result<Self> {
        let mut candidates: Vec<_> = explicit.into_iter().map(Path::to_path_buf).collect();
        let rust_override = paths.explicit.is_some() || paths.rust_home.is_some();
        for root in [
            paths.explicit.as_ref(),
            paths.rust_home.as_ref(),
            if rust_override {
                None
            } else {
                paths.legacy_home.as_ref()
            },
            Some(&paths.executable_dir),
        ]
        .into_iter()
        .flatten()
        {
            candidates.push(root.join("codeintel/definitions.json"));
        }
        candidates.push(PathBuf::from("codeintel/definitions.json"));
        for path in candidates {
            if !path.try_exists().map_err(|e| invalid(e.to_string()))? {
                continue;
            }
            let loaded = (|| {
                let file = open_regular_file(&path)?;
                let mut text = String::new();
                file.take((MAX_BYTES + 1) as u64)
                    .read_to_string(&mut text)
                    .map_err(|e| invalid(e.to_string()))?;
                Self::from_json(&text)
            })();
            return match loaded {
                Ok(mut registry) => {
                    registry.origin = Some(path);
                    Ok(registry)
                }
                Err(error) => {
                    let mut registry = Self::compiled()?;
                    registry.diagnostics.push(format!(
                        "{}: {error}; using bundled definitions",
                        path.display()
                    ));
                    Ok(registry)
                }
            };
        }
        Self::compiled()
    }
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.configs.keys().map(String::as_str)
    }
    pub(super) fn config(&self, language: &str) -> &LanguageConfig {
        let language = language.to_lowercase();
        self.configs
            .get(&language)
            .or_else(|| {
                self.aliases
                    .get(&language)
                    .and_then(|key| self.configs.get(key))
            })
            .unwrap_or(&self.defaults)
    }
}
fn compile_pattern(p: Pattern) -> Result<DefinitionPattern> {
    let regex = SearchRegex::compile(&format!("(?i:{})", p.regex))?;
    if p.group_index >= regex.capture_count() {
        return Err(invalid("definition capture group does not exist"));
    }
    let kind = match p.kind.to_uppercase().as_str() {
        "OBJECT" => SymbolKind::Class,
        name => serde_json::from_value(serde_json::Value::String(name.into()))
            .unwrap_or(SymbolKind::Variable),
    };
    Ok(DefinitionPattern {
        regex,
        kind,
        group: p.group_index,
    })
}
fn default_patterns() -> Result<Vec<DefinitionPattern>> {
    [
        ("CLASS", r"\bclass\s+([A-Za-z_][A-Za-z0-9_]*)"),
        ("INTERFACE", r"\binterface\s+([A-Za-z_][A-Za-z0-9_]*)"),
        ("OBJECT", r"\bobject\s+([A-Za-z_][A-Za-z0-9_]*)"),
        ("ENUM", r"\benum\s+class\s+([A-Za-z_][A-Za-z0-9_]*)"),
        ("FUNCTION", r"\bfun\s+([A-Za-z_][A-Za-z0-9_]*)"),
        ("VARIABLE", r"\b(?:val|var)\s+([A-Za-z_][A-Za-z0-9_]*)"),
    ]
    .into_iter()
    .map(|(kind, regex)| {
        compile_pattern(Pattern {
            kind: kind.into(),
            regex: regex.into(),
            group_index: 1,
        })
    })
    .collect()
}
fn invalid(message: impl ToString) -> DomainError {
    DomainError::Parse {
        format: "code intelligence definitions",
        message: message.to_string(),
    }
}
