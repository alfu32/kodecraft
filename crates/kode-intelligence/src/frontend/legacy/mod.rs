//! Explicitly low-confidence regex fallback with source-preserving byte ranges.
mod registry;
mod sanitize;
use crate::{ids::*, model::*};
use kode_core::{ByteOffset, DomainError, Result, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_syntax::keywords::KeywordProvider;
pub use registry::DefinitionRegistry;
use registry::{BlockMode, LanguageConfig};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

pub fn extract(
    registry: &DefinitionRegistry,
    keywords: &KeywordProvider,
    path: &str,
    language: &str,
    snapshot: &TextSnapshot,
    cancel: &CancellationToken,
) -> Result<FileSemanticDelta> {
    let started = Instant::now();
    let source = snapshot.text();
    if source.len() > 16 * 1024 * 1024 {
        return Err(limit());
    }
    let mut delta = super::empty_delta(path, language, &source, snapshot.revision.0);
    let file = delta.file.id;
    let scope = SemanticIds::scope(file, ByteOffset(0), ScopeKind::File);
    delta.scopes.push(ScopeRecord {
        id: scope,
        file_id: file,
        parent_id: None,
        owner_symbol_id: None,
        kind: ScopeKind::File,
        range: SourceRange::new(ByteOffset(0), ByteOffset(source.len()))?,
    });
    let config = registry.config(language);
    let (sanitized, lexical) = sanitize::sanitize(&source, config, cancel)?;
    delta.lexical_tokens = lexical
        .into_iter()
        .map(|(range, kind)| LexicalTokenRecord {
            file_id: file,
            range,
            kind,
            confidence: Confidence::new(SemanticSource::Heuristic, 0.5).expect("finite confidence"),
        })
        .collect();
    let confidence = Confidence::new(SemanticSource::Heuristic, 0.5)?;
    let mut definitions = HashMap::<SourceRange, (SymbolKind, Option<String>)>::new();
    let mut tracker = BlockTracker::default();
    let mut owners: Vec<(String, usize)> = vec![];
    let mut offset = 0;
    let mut matches = 0usize;
    for line in sanitized.split_inclusive('\n') {
        check(cancel, started, matches)?;
        if line.len() > 256 * 1024 {
            return Err(limit());
        }
        if !line.trim().is_empty() {
            let depth = tracker.update(line, config)?;
            while owners.last().is_some_and(|(_, d)| *d > depth) {
                owners.pop();
            }
            for pattern in &config.patterns {
                pattern
                    .regex
                    .for_each_capture(line, pattern.group, |span| {
                        matches += 1;
                        check(cancel, started, matches)?;
                        if span.is_empty() {
                            return Ok(());
                        }
                        let name = &line[span.start.0..span.end.0];
                        if name.trim().is_empty() || keywords.is_keyword(language, name)? {
                            return Ok(());
                        }
                        let range = SourceRange::new(
                            ByteOffset(offset + span.start.0),
                            ByteOffset(offset + span.end.0),
                        )?;
                        if definitions
                            .get(&range)
                            .is_none_or(|(kind, _)| priority(pattern.kind) > priority(*kind))
                        {
                            definitions.insert(
                                range,
                                (pattern.kind, owners.last().map(|(name, _)| name.clone())),
                            );
                            if opens_scope(pattern.kind) {
                                owners.push((name.into(), depth));
                            }
                        }
                        Ok(())
                    })?;
            }
        }
        offset += line.len();
    }
    let mut declaration_ids = HashMap::new();
    let mut definitions: Vec<_> = definitions.into_iter().collect();
    definitions.sort_by_key(|(range, _)| (range.start, range.end));
    for (range, (kind, owner)) in definitions {
        let name = &source[range.start.0..range.end.0];
        let id = SemanticIds::symbol(file, range.start, kind, name);
        declaration_ids.insert(range, id);
        delta.symbols.push(SymbolRecord {
            id,
            file_id: file,
            name: name.into(),
            qualified_name: Some(
                owner.map_or_else(|| name.to_owned(), |owner| format!("{owner}.{name}")),
            ),
            kind,
            declaration_range: range,
            name_range: range,
            scope_id: scope,
            owner_symbol_id: None,
            declared_type_id: None,
            inferred_type_id: None,
            flags: 0,
            confidence,
        });
    }
    let mut iterator = sanitized.char_indices().peekable();
    while let Some((start, ch)) = iterator.next() {
        if ch != '_' && !ch.is_alphabetic() {
            continue;
        }
        let mut end = start + ch.len_utf8();
        while let Some((i, ch)) = iterator.peek().copied() {
            if ch != '_' && !ch.is_alphanumeric() {
                break;
            }
            iterator.next();
            end = i + ch.len_utf8();
        }
        matches += 1;
        check(cancel, started, matches)?;
        let range = SourceRange::new(ByteOffset(start), ByteOffset(end))?;
        let declaration = declaration_ids.get(&range).copied();
        let text = &source[start..end];
        if declaration.is_none() && keywords.is_keyword(language, text)? {
            continue;
        }
        delta.occurrences.push(OccurrenceRecord {
            id: SemanticIds::occurrence(file, range),
            file_id: file,
            range,
            text: text.into(),
            kind: if declaration.is_some() {
                OccurrenceKind::Declaration
            } else {
                OccurrenceKind::Unknown
            },
            scope_id: scope,
            resolved_symbol_id: declaration,
            receiver_occurrence_id: None,
            confidence,
        });
    }
    // Qualified captures can span multiple identifier tokens; keep their exact
    // declaration occurrence as well, matching the legacy compatibility bridge.
    let occurrence_ids: HashSet<_> = delta.occurrences.iter().map(|o| o.id).collect();
    for (range, id) in declaration_ids {
        check(cancel, started, matches)?;
        let occurrence_id = SemanticIds::occurrence(file, range);
        if !occurrence_ids.contains(&occurrence_id) {
            delta.occurrences.push(OccurrenceRecord {
                id: occurrence_id,
                file_id: file,
                range,
                text: source[range.start.0..range.end.0].into(),
                kind: OccurrenceKind::Declaration,
                scope_id: scope,
                resolved_symbol_id: Some(id),
                receiver_occurrence_id: None,
                confidence,
            });
        }
    }
    delta
        .occurrences
        .sort_by_key(|o| (o.range.start, o.range.end));
    delta.exported_surface_hash = public_surface_hash(&delta);
    delta.validate_source(&source)?;
    Ok(delta)
}
fn priority(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Method | SymbolKind::Field => 3,
        SymbolKind::Function => 2,
        SymbolKind::Variable => 1,
        _ => 0,
    }
}
fn opens_scope(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Class
            | SymbolKind::Interface
            | SymbolKind::Enum
            | SymbolKind::Module
            | SymbolKind::Package
            | SymbolKind::Function
            | SymbolKind::Method
    )
}
fn limit() -> DomainError {
    DomainError::Unsupported("heuristic extraction work limit exceeded".into())
}
fn check(cancel: &CancellationToken, started: Instant, count: usize) -> Result<()> {
    if cancel.is_cancelled() {
        Err(DomainError::Cancelled)
    } else if count > 100_000 || started.elapsed() > Duration::from_secs(2) {
        Err(limit())
    } else {
        Ok(())
    }
}
#[derive(Default)]
struct BlockTracker {
    depth: usize,
    indents: Vec<usize>,
}
impl BlockTracker {
    fn update(&mut self, line: &str, config: &LanguageConfig) -> Result<usize> {
        let balance = |open: char, close: char, depth: usize| {
            depth
                .saturating_add(line.matches(open).count())
                .saturating_sub(line.matches(close).count())
        };
        self.depth = match config.mode {
            BlockMode::Brace => balance('{', '}', self.depth),
            BlockMode::Paren => balance('(', ')', self.depth),
            BlockMode::Indent => {
                let indent = line
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '\t'))
                    .map(|c| if c == '\t' { 4 } else { 1 })
                    .sum();
                if self.indents.last().is_none_or(|last| indent > *last) {
                    self.indents.push(indent);
                }
                while self.indents.last().is_some_and(|last| *last > indent) {
                    self.indents.pop();
                }
                self.indents.len().saturating_sub(1)
            }
            BlockMode::Regex => {
                let begin = config
                    .begin
                    .as_ref()
                    .map_or(Ok(false), |re| re.is_match(line))?;
                let end = config
                    .end
                    .as_ref()
                    .map_or(Ok(false), |re| re.is_match(line))?;
                self.depth
                    .saturating_add(usize::from(begin))
                    .saturating_sub(usize::from(end))
            }
            BlockMode::Sql => self
                .depth
                .saturating_add(usize::from(line.to_uppercase().contains("BEGIN")))
                .saturating_sub(usize::from(line.to_uppercase().contains("END"))),
            BlockMode::None => self.depth,
        };
        Ok(self.depth)
    }
}
