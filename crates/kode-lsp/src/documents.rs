//! Negotiated positions and protocol replicas. The application owns real edits;
//! a client actor owns this bounded tracker and writes its messages in order.
use crate::transport::protocol;
use kode_core::{
    ByteOffset, DocumentId, DocumentOrigin, DomainError, Result, Revision, TextRange,
    document::TextSnapshot,
};
use lsp_types::{
    Position, PositionEncodingKind, Range, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OPEN_DOCUMENTS: usize = 64;
pub const MAX_OPEN_BYTES: usize = 64 * 1024 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}
impl PositionEncoding {
    pub fn negotiate(encoding: Option<&PositionEncodingKind>) -> Result<Self> {
        match encoding
            .map(PositionEncodingKind::as_str)
            .unwrap_or("utf-16")
        {
            "utf-8" => Ok(Self::Utf8),
            "utf-16" => Ok(Self::Utf16),
            "utf-32" => Ok(Self::Utf32),
            other => Err(protocol(format!(
                "unsupported negotiated position encoding: {other}"
            ))),
        }
    }
}
pub fn file_uri(path: &Path) -> Result<Uri> {
    let path = std::path::absolute(path).map_err(|e| protocol(e.to_string()))?;
    let uri =
        reqwest::Url::from_file_path(&path).map_err(|_| protocol("cannot encode file URI"))?;
    Uri::from_str(uri.as_str()).map_err(|e| protocol(e.to_string()))
}
pub fn uri_path(uri: &Uri) -> Result<PathBuf> {
    reqwest::Url::parse(uri.as_str())
        .map_err(|e| protocol(e.to_string()))?
        .to_file_path()
        .map_err(|_| protocol("location is not a local file URI"))
}
fn origin_uri(origin: &DocumentOrigin) -> Result<Uri> {
    match origin {
        DocumentOrigin::File(path) => file_uri(path),
        DocumentOrigin::Virtual { .. } => Err(DomainError::Unsupported(
            "server document synchronization requires a file origin".into(),
        )),
    }
}
pub fn language_id(language: &str) -> String {
    let language = language.to_ascii_lowercase();
    match language.as_str() {
        "tsx" => "typescriptreact",
        "jsx" => "javascriptreact",
        "bash" | "sh" => "shellscript",
        "kt" | "kts" => "kotlin",
        "c#" | "cs" => "csharp",
        "c++" | "cplus" => "cpp",
        "js" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "rb" => "ruby",
        _ => &language,
    }
    .into()
}
/// LSP recognizes CR, LF and CRLF independently of the editor's LF-only display
/// model. Build this index once on a worker and reuse it for every returned range.
#[derive(Debug, Clone)]
pub struct PositionMapper {
    snapshot: TextSnapshot,
    starts: Arc<Vec<usize>>,
    encoding: PositionEncoding,
}
impl PositionMapper {
    pub fn new(snapshot: TextSnapshot, encoding: PositionEncoding) -> Result<Self> {
        if snapshot.len_bytes() > MAX_DOCUMENT_BYTES {
            return Err(protocol("LSP document exceeds 8 MiB"));
        }
        let mut starts = vec![0];
        let mut offset = 0;
        let mut chars = snapshot.chars().peekable();
        while let Some(ch) = chars.next() {
            offset += ch.len_utf8();
            if ch == '\r' || ch == '\n' {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                    offset += 1;
                }
                if starts.len() >= 1_000_000 {
                    return Err(protocol("LSP document exceeds one million protocol lines"));
                }
                starts.push(offset);
            }
        }
        Ok(Self {
            snapshot,
            starts: Arc::new(starts),
            encoding,
        })
    }
    pub fn snapshot(&self) -> &TextSnapshot {
        &self.snapshot
    }
    pub fn accounted_bytes(&self) -> usize {
        self.snapshot.len_bytes() + self.starts.len() * std::mem::size_of::<usize>()
    }
    fn line(&self, line: usize) -> Result<TextRange> {
        let start = *self
            .starts
            .get(line)
            .ok_or_else(|| protocol("LSP line index is outside the document"))?;
        let mut end = self
            .starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.snapshot.len_bytes());
        if line + 1 < self.starts.len() {
            let slice = self.snapshot.slice(TextRange {
                start: ByteOffset(start),
                end: ByteOffset(end),
            })?;
            let mut chars = slice.chars_at(slice.len_chars()).reversed();
            match chars.next() {
                Some('\n') => {
                    end -= 1;
                    if chars.next() == Some('\r') {
                        end -= 1;
                    }
                }
                Some('\r') => end -= 1,
                _ => {}
            }
        }
        Ok(TextRange {
            start: ByteOffset(start),
            end: ByteOffset(end),
        })
    }
    pub fn position(&self, offset: ByteOffset) -> Result<Position> {
        self.snapshot.byte_to_char(offset)?;
        let line = self
            .starts
            .partition_point(|start| *start <= offset.0)
            .saturating_sub(1);
        let range = self.line(line)?;
        let offset = offset.min(range.end);
        let text = self.snapshot.slice(TextRange {
            start: range.start,
            end: offset,
        })?;
        let character = match self.encoding {
            PositionEncoding::Utf8 => offset.0 - range.start.0,
            PositionEncoding::Utf16 => text.chars().map(char::len_utf16).sum(),
            PositionEncoding::Utf32 => text.len_chars(),
        };
        Ok(Position {
            line: line as u32,
            character: character
                .try_into()
                .map_err(|_| protocol("LSP column exceeds u32"))?,
        })
    }
    /// Columns past EOL clamp to EOL; interior byte/surrogate offsets clamp to
    /// the preceding scalar. Invalid lines are explicit errors.
    pub fn byte(&self, position: Position) -> Result<ByteOffset> {
        let range = self.line(position.line as usize)?;
        if self.encoding == PositionEncoding::Utf8 {
            return Ok(self.snapshot.clamp_byte(ByteOffset(
                range
                    .start
                    .0
                    .saturating_add(position.character as usize)
                    .min(range.end.0),
            )));
        }
        let mut offset = range.start;
        let mut units = 0usize;
        for ch in self.snapshot.slice(range)?.chars() {
            let count = if self.encoding == PositionEncoding::Utf16 {
                ch.len_utf16()
            } else {
                1
            };
            if units + count > position.character as usize {
                break;
            }
            units += count;
            offset.0 += ch.len_utf8();
        }
        Ok(offset)
    }
    pub fn byte_range(&self, range: Range) -> Result<TextRange> {
        TextRange::new(self.byte(range.start)?, self.byte(range.end)?)
    }
}
pub fn position(
    snapshot: &TextSnapshot,
    offset: ByteOffset,
    encoding: PositionEncoding,
) -> Result<Position> {
    PositionMapper::new(snapshot.clone(), encoding)?.position(offset)
}
pub fn byte(
    snapshot: &TextSnapshot,
    position: Position,
    encoding: PositionEncoding,
) -> Result<ByteOffset> {
    PositionMapper::new(snapshot.clone(), encoding)?.byte(position)
}
pub fn byte_range(
    snapshot: &TextSnapshot,
    range: Range,
    encoding: PositionEncoding,
) -> Result<TextRange> {
    PositionMapper::new(snapshot.clone(), encoding)?.byte_range(range)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncMode {
    pub open_close: bool,
    pub change: TextDocumentSyncKind,
    pub save: bool,
    pub save_text: bool,
}
impl SyncMode {
    pub fn negotiated(capability: Option<&TextDocumentSyncCapability>) -> Self {
        let mut mode = Self {
            open_close: false,
            change: TextDocumentSyncKind::NONE,
            save: false,
            save_text: false,
        };
        match capability {
            Some(TextDocumentSyncCapability::Kind(kind)) => {
                mode.change = *kind;
                mode.open_close = *kind != TextDocumentSyncKind::NONE;
            }
            Some(TextDocumentSyncCapability::Options(options)) => {
                mode.open_close = options.open_close.unwrap_or(false);
                mode.change = options.change.unwrap_or(TextDocumentSyncKind::NONE);
                if let Some(save) = &options.save {
                    match save {
                        lsp_types::TextDocumentSyncSaveOptions::Supported(supported) => {
                            mode.save = *supported
                        }
                        lsp_types::TextDocumentSyncSaveOptions::SaveOptions(options) => {
                            mode.save = true;
                            mode.save_text = options.include_text.unwrap_or(false);
                        }
                    }
                }
            }
            None => {}
        }
        mode
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentStamp {
    pub document: DocumentId,
    pub revision: Revision,
    pub version: i32,
}
#[derive(Debug, Clone)]
struct OpenDocument {
    snapshot: TextSnapshot,
    mapper: PositionMapper,
    language: String,
    version: i32,
}
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: &'static str,
    pub params: Value,
}
#[derive(Default)]
pub struct DocumentTracker {
    opened: BTreeMap<String, OpenDocument>,
}
impl DocumentTracker {
    pub fn mapper(&self, uri: &Uri) -> Option<PositionMapper> {
        self.opened.get(uri.as_str()).map(|doc| doc.mapper.clone())
    }
    pub fn stamp(&self, uri: &Uri) -> Option<DocumentStamp> {
        self.opened.get(uri.as_str()).map(|doc| DocumentStamp {
            document: doc.snapshot.id,
            revision: doc.snapshot.revision,
            version: doc.version,
        })
    }
    pub fn stamps(&self) -> BTreeMap<String, DocumentStamp> {
        self.opened
            .keys()
            .map(|uri| {
                let doc = &self.opened[uri];
                (
                    uri.clone(),
                    DocumentStamp {
                        document: doc.snapshot.id,
                        revision: doc.snapshot.revision,
                        version: doc.version,
                    },
                )
            })
            .collect()
    }
    pub fn synchronize(
        &mut self,
        snapshot: TextSnapshot,
        mode: SyncMode,
        encoding: PositionEncoding,
    ) -> Result<Vec<Notification>> {
        if snapshot.len_bytes() > MAX_DOCUMENT_BYTES {
            return Err(protocol("LSP document exceeds 8 MiB"));
        }
        if !mode.open_close && mode.change == TextDocumentSyncKind::NONE {
            return Err(DomainError::Unsupported(
                "server does not accept document synchronization".into(),
            ));
        }
        let uri = origin_uri(&snapshot.origin)?;
        let old = self.opened.get(uri.as_str());
        if let Some(old) = old
            && old.snapshot.id == snapshot.id
            && old.snapshot.revision > snapshot.revision
        {
            return Err(DomainError::StaleRevision {
                expected: snapshot.revision,
                current: old.snapshot.revision,
            });
        }
        let language = language_id(
            &snapshot
                .language
                .clone()
                .unwrap_or_else(|| "plaintext".into()),
        );
        if let Some(old) = old
            && (old.snapshot.id, old.snapshot.revision, &old.language)
                == (snapshot.id, snapshot.revision, &language)
        {
            return Ok(vec![]);
        }
        let mapper = PositionMapper::new(snapshot.clone(), encoding)?;
        let bytes: usize = self
            .opened
            .values()
            .map(|document| document.mapper.accounted_bytes())
            .sum();
        if old.is_none() && self.opened.len() >= MAX_OPEN_DOCUMENTS
            || bytes - old.map_or(0, |doc| doc.mapper.accounted_bytes()) + mapper.accounted_bytes()
                > MAX_OPEN_BYTES
        {
            return Err(DomainError::QueueFull);
        }
        let reset = old.is_none_or(|old| {
            old.snapshot.id != snapshot.id || old.language != language || old.version == i32::MAX
        });
        let mut messages = vec![];
        let version = if reset {
            if old.is_some() && mode.open_close {
                messages.push(Notification {
                    method: "textDocument/didClose",
                    params: json!({"textDocument":{"uri":uri}}),
                });
            }
            if mode.open_close {
                messages.push(Notification { method:"textDocument/didOpen",params:json!({"textDocument":{"uri":uri,"languageId":language,"version":1,"text":snapshot.text()}}) });
            } else {
                messages.push(Notification { method:"textDocument/didChange",params:json!({"textDocument":{"uri":uri,"version":1},"contentChanges":[{"text":snapshot.text()}]}) });
            }
            1
        } else {
            let old = old.expect("existing synchronized document");
            let version = old.version + 1;
            let change = if mode.change == TextDocumentSyncKind::FULL {
                json!({"text":snapshot.text()})
            } else if mode.change == TextDocumentSyncKind::INCREMENTAL {
                let (start, old_end, new_end) = difference(&old.snapshot, &snapshot)?;
                let range = Range {
                    start: old.mapper.position(start)?,
                    end: old.mapper.position(old_end)?,
                };
                json!({"range":range,"text":snapshot.slice(TextRange { start,end:new_end })?.to_string()})
            } else {
                return Err(DomainError::Unsupported(
                    "server does not accept document changes".into(),
                ));
            };
            messages.push(Notification { method:"textDocument/didChange",params:json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[change]}) });
            version
        };
        self.opened.insert(
            uri.as_str().to_owned(),
            OpenDocument {
                snapshot,
                mapper,
                language,
                version,
            },
        );
        Ok(messages)
    }
    pub fn save(
        &mut self,
        snapshot: TextSnapshot,
        mode: SyncMode,
        encoding: PositionEncoding,
    ) -> Result<Vec<Notification>> {
        let mut messages = self.synchronize(snapshot.clone(), mode, encoding)?;
        if mode.save {
            let mut params = json!({"textDocument":{"uri":origin_uri(&snapshot.origin)?}});
            if mode.save_text {
                params["text"] = json!(snapshot.text());
            }
            messages.push(Notification {
                method: "textDocument/didSave",
                params,
            });
        }
        Ok(messages)
    }
    pub fn close(&mut self, origin: &DocumentOrigin, mode: SyncMode) -> Result<Vec<Notification>> {
        let uri = origin_uri(origin)?;
        if self.opened.remove(uri.as_str()).is_some() && mode.open_close {
            Ok(vec![Notification {
                method: "textDocument/didClose",
                params: json!({"textDocument":{"uri":uri}}),
            }])
        } else {
            Ok(vec![])
        }
    }
}
fn difference(
    old: &TextSnapshot,
    new: &TextSnapshot,
) -> Result<(ByteOffset, ByteOffset, ByteOffset)> {
    let mut start = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b {
            break;
        }
        start += a.len_utf8();
    }
    let mut end = old.len_bytes();
    let mut new_end = new.len_bytes();
    let a = old.slice(TextRange {
        start: ByteOffset(0),
        end: ByteOffset(end),
    })?;
    let b = new.slice(TextRange {
        start: ByteOffset(0),
        end: ByteOffset(new_end),
    })?;
    for (a, b) in a
        .chars_at(a.len_chars())
        .reversed()
        .zip(b.chars_at(b.len_chars()).reversed())
    {
        if end <= start || new_end <= start || a != b {
            break;
        }
        end -= a.len_utf8();
        new_end -= b.len_utf8();
    }
    // CRLF is one protocol line break: a range cannot end between its bytes.
    let split = |offset: usize| {
        offset > 0
            && offset < old.len_bytes()
            && old
                .slice(TextRange {
                    start: ByteOffset(offset - 1),
                    end: ByteOffset(offset + 1),
                })
                .is_ok_and(|slice| slice == "\r\n")
    };
    if split(start) {
        start -= 1;
    }
    if split(end) {
        end += 1;
        new_end += 1;
    }
    Ok((ByteOffset(start), ByteOffset(end), ByteOffset(new_end)))
}
