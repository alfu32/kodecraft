use super::*;
use crate::documents::{PositionEncoding, uri_path};
use kode_core::{
    ByteOffset, DocumentId, LineIndex, TextRange, Utf16Column,
    document::{Document, Utf16Position, encoding},
    intelligence::{SymbolKind, identifier_prefix},
};
use lsp_types::{
    CompletionItemKind as Kind, CompletionResponse, CompletionTextEdit, GotoDefinitionResponse,
    InsertTextFormat, Range,
};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
};

pub(super) type Location = (Uri, Range, Option<Range>);
pub(super) fn definitions(response: Option<GotoDefinitionResponse>) -> Vec<Location> {
    match response {
        Some(GotoDefinitionResponse::Scalar(location)) => {
            vec![(location.uri, location.range, None)]
        }
        Some(GotoDefinitionResponse::Array(locations)) => locations
            .into_iter()
            .map(|location| (location.uri, location.range, None))
            .collect(),
        Some(GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| {
                (
                    link.target_uri,
                    link.target_selection_range,
                    Some(link.target_range),
                )
            })
            .collect(),
        None => vec![],
    }
}
fn disk_snapshot(path: &Path) -> Result<TextSnapshot> {
    let io_error = |source| DomainError::Io {
        operation: "read LSP target",
        path: path.into(),
        source,
    };
    let file = kode_platform::files::open_regular_file(path)?;
    let before = file.metadata().map_err(io_error)?;
    let mut bytes = vec![];
    (&file)
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let after = file.metadata().map_err(io_error)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(DomainError::Unsupported(
            "LSP target source exceeds 8 MiB".into(),
        ));
    }
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(path.into()));
    }
    Ok(Document::from_decoded(
        DocumentId::fresh()?,
        DocumentOrigin::File(path.into()),
        None,
        encoding::decode(&bytes)?,
    )
    .snapshot())
}
pub(super) fn targets(
    editor: &EditorLsp,
    query: &Query,
    locations: Vec<Location>,
) -> Result<Vec<NavigationTarget>> {
    let mut cache: HashMap<String, Option<(PositionMapper, bool)>> = HashMap::new();
    let mut accounted = 0;
    let encoding = query.client.info()?.encoding;
    let mut targets = vec![];
    let mut seen = HashSet::new();
    for (uri, selection, declaration) in locations.into_iter().take(1000) {
        let Ok(path) = uri_path(&uri) else {
            continue;
        };
        if selection.start > selection.end {
            continue;
        }
        if !seen.insert((
            uri.as_str().to_owned(),
            selection.start.line,
            selection.start.character,
            selection.end.line,
            selection.end.character,
        )) {
            continue;
        }
        let key = uri.as_str().to_owned();
        if !cache.contains_key(&key) {
            if cache.len() >= MAX_OPEN_DOCUMENTS {
                continue;
            }
            let captured = editor.captured_source(&DocumentOrigin::File(path.clone()));
            if let Some(snapshot) = &captured
                && !query.client.document_stamp(&uri).is_some_and(|stamp| {
                    stamp.document == snapshot.id && stamp.revision == snapshot.revision
                })
            {
                return Err(DomainError::Cancelled);
            }
            let mapped = if uri == query.uri {
                Some((query.mapper.clone(), true))
            } else if let Some(snapshot) = captured {
                PositionMapper::new(snapshot, encoding)
                    .ok()
                    .map(|mapper| (mapper, true))
            } else {
                disk_snapshot(&path)
                    .and_then(|snapshot| PositionMapper::new(snapshot, encoding))
                    .ok()
                    .map(|mapper| (mapper, false))
            };
            if let Some((mapper, _)) = &mapped {
                accounted += mapper.accounted_bytes();
                if accounted > MAX_OPEN_BYTES {
                    break;
                }
            }
            cache.insert(key.clone(), mapped);
        }
        let target = match cache.get(&key).and_then(Option::as_ref) {
            Some((mapper, captured)) => {
                let Ok(range) = mapper.byte_range(selection) else {
                    continue;
                };
                NavigationTarget {
                    path,
                    document: captured.then_some(mapper.snapshot().id),
                    identifier_range: range,
                    location: Some(mapper.snapshot().byte_to_utf16(range.start)?),
                    declaration_range: declaration.and_then(|range| mapper.byte_range(range).ok()),
                    language: mapper.snapshot().language.clone(),
                    kind: None,
                    name: None,
                    missing_source: false,
                }
            }
            None if encoding == PositionEncoding::Utf16 => NavigationTarget {
                path,
                document: None,
                identifier_range: TextRange::caret(ByteOffset(0)),
                location: Some(Utf16Position {
                    line: LineIndex(selection.start.line as usize),
                    column: Utf16Column(selection.start.character),
                }),
                declaration_range: None,
                language: None,
                kind: None,
                name: None,
                missing_source: true,
            },
            None => continue,
        };
        targets.push(target);
    }
    Ok(targets)
}
pub(super) fn completions(
    query: &Query,
    request: &IntelligenceRequest,
    response: Option<CompletionResponse>,
) -> Result<Vec<CompletionItem>> {
    let items = match response {
        Some(CompletionResponse::Array(items)) => items,
        Some(CompletionResponse::List(list)) => list.items,
        None => vec![],
    };
    let prefix = identifier_prefix(&request.snapshot.text, request.position)?;
    let mut seen = HashSet::new();
    let mut output = vec![];
    for item in items.into_iter().take(1000) {
        if item.label.trim().is_empty()
            || item.label.len() > 2048
            || item.insert_text_format == Some(InsertTextFormat::SNIPPET)
            || item
                .additional_text_edits
                .as_ref()
                .is_some_and(|edits| !edits.is_empty())
            || item.command.is_some()
        {
            continue;
        }
        let insert_text = match item.text_edit {
            Some(CompletionTextEdit::Edit(edit)) => {
                // E09 inserts only the typed prefix and preserves receiver/suffix.
                if query.mapper.byte_range(edit.range).ok() != Some(prefix) {
                    continue;
                }
                edit.new_text
            }
            Some(CompletionTextEdit::InsertAndReplace(_)) => continue,
            None => item.insert_text.unwrap_or_else(|| item.label.clone()),
        };
        if insert_text.len() > 128 * 1024 || !seen.insert(item.label.clone()) {
            continue;
        }
        output.push(CompletionItem {
            label: item.label,
            insert_text,
            detail: item
                .detail
                .map(|detail| detail.chars().take(2048).collect())
                .or(Some("lsp".into())),
            kind: item.kind.and_then(kind),
            replacement_range: prefix,
            source: "lsp".into(),
        });
        if output.len() == 50 {
            break;
        }
    }
    Ok(output)
}
fn kind(kind: Kind) -> Option<SymbolKind> {
    Some(match kind {
        Kind::CLASS | Kind::STRUCT => SymbolKind::Class,
        Kind::INTERFACE => SymbolKind::Interface,
        Kind::METHOD => SymbolKind::Method,
        Kind::FUNCTION | Kind::CONSTRUCTOR => SymbolKind::Function,
        Kind::FIELD | Kind::PROPERTY => SymbolKind::Field,
        Kind::VARIABLE | Kind::CONSTANT => SymbolKind::Variable,
        Kind::ENUM | Kind::ENUM_MEMBER => SymbolKind::Enum,
        Kind::MODULE => SymbolKind::Module,
        Kind::KEYWORD => SymbolKind::Keyword,
        Kind::TYPE_PARAMETER => SymbolKind::TypeAlias,
        _ => return None,
    })
}
