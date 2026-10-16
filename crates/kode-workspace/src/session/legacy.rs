//! Kotlin v6 DTOs retain camelCase defaults and unknown fields verbatim.
use super::{EditorStateV1, ProjectSessionV1, RecentFileV1};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, LineIndex, Result, Revision, Selection, Utf16Column,
    document::{Document, Utf16Position},
    editor::history::{HistoryStateV1, PersistedBuffer},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacyPosition {
    #[serde(default)]
    pub line: i64,
    #[serde(default)]
    pub column: i64,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacySnapshot {
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub cursor: LegacyPosition,
    #[serde(default)]
    pub anchor: Option<LegacyPosition>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LegacyBuffer {
    #[serde(flatten)]
    pub current: LegacySnapshot,
    #[serde(default)]
    pub undo: Vec<LegacySnapshot>,
    #[serde(default)]
    pub redo: Vec<LegacySnapshot>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyEditor {
    path: PathBuf,
    #[serde(default)]
    buffer: LegacyBuffer,
    #[serde(default)]
    scroll_top: i64,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    grammar_language: Option<String>,
    #[serde(default)]
    grammar_available: bool,
    #[serde(default)]
    last_modified_millis: Option<i64>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyRecent {
    path: PathBuf,
    #[serde(default)]
    last_opened_epoch_millis: i64,
    #[serde(default)]
    last_modified_millis: Option<i64>,
    #[serde(default)]
    editor: Option<LegacyEditor>,
    #[serde(default)]
    dirty: bool,
    #[serde(default = "super::code_viewer")]
    viewer_type: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacySession {
    #[serde(default = "legacy_version")]
    version: u32,
    #[serde(default)]
    recent_files: Vec<LegacyRecent>,
    #[serde(default)]
    open_editors: Vec<LegacyEditor>,
    #[serde(default)]
    source_roots: Vec<PathBuf>,
    #[serde(default)]
    scan_exclusions: Vec<PathBuf>,
    #[serde(default = "super::database")]
    database: Value,
    #[serde(default = "super::rest_api")]
    rest_api: Value,
    #[serde(flatten)]
    extra: Map<String, Value>,
}
fn legacy_version() -> u32 {
    6
}
fn preserve_collisions(extra: &mut Map<String, Value>, reserved: &[&str], namespace: &str) {
    let mut collisions = Map::new();
    for name in reserved.iter().copied().chain(std::iter::once(namespace)) {
        if let Some(value) = extra.remove(name) {
            collisions.insert(name.into(), value);
        }
    }
    if !collisions.is_empty() {
        extra.insert(namespace.into(), Value::Object(collisions));
    }
}
fn position(document: &Document, position: &LegacyPosition) -> kode_core::ByteOffset {
    document.snapshot().utf16_to_byte_clamped(Utf16Position {
        line: LineIndex(usize::try_from(position.line.max(0)).unwrap_or(usize::MAX)),
        column: Utf16Column(u32::try_from(position.column.max(0)).unwrap_or(u32::MAX)),
    })
}
fn snapshot(snapshot: &LegacySnapshot) -> PersistedBuffer {
    let text = snapshot.lines.join("\n");
    let document = Document::new(
        DocumentId(0),
        DocumentOrigin::File(PathBuf::new()),
        None,
        &text,
    );
    let head = position(&document, &snapshot.cursor);
    let anchor = snapshot
        .anchor
        .as_ref()
        .map_or(head, |anchor| position(&document, anchor));
    PersistedBuffer {
        text,
        selection: Selection { head, anchor },
        secondary: vec![],
    }
}
pub fn convert_buffer(buffer: &LegacyBuffer) -> HistoryStateV1 {
    let current = snapshot(&buffer.current);
    let saved = if !buffer.current.dirty {
        Some(current.text.clone())
    } else {
        buffer
            .undo
            .iter()
            .rev()
            .chain(buffer.redo.iter().rev())
            .find(|snapshot| !snapshot.dirty)
            .map(|state| state.lines.join("\n"))
    };
    // A source dirty flag with no actual saved snapshot must not become a clean
    // empty document merely because a baseline could not be reconstructed.
    let saved_baseline_known = saved
        .as_ref()
        .is_some_and(|saved| !buffer.current.dirty || saved != &current.text);
    HistoryStateV1 {
        version: 1,
        revision: Revision(0),
        saved_revision: Revision(0),
        current,
        saved_text: saved.unwrap_or_default(),
        saved_baseline_known,
        undo: buffer.undo.iter().map(snapshot).collect(),
        redo: buffer.redo.iter().map(snapshot).collect(),
        omitted_undo: 0,
        omitted_redo: 0,
    }
}
fn editor(editor: LegacyEditor) -> Result<EditorStateV1> {
    let history = convert_buffer(&editor.buffer);
    // Core validates every imported UTF-8 position and enforces history limits.
    let mut document = Document::new(
        DocumentId(0),
        DocumentOrigin::File(editor.path.clone()),
        editor.language.clone(),
        "",
    );
    document.import_history(history.clone(), &mut Default::default())?;
    let mut extra = editor.extra;
    preserve_collisions(
        &mut extra,
        &[
            "origin",
            "history",
            "format",
            "readOnly",
            "legacyFormat",
            "diskIdentity",
            "legacyBuffer",
        ],
        "legacyEditorFields",
    );
    extra.insert(
        "legacyBuffer".into(),
        serde_json::to_value(&editor.buffer).map_err(|error| DomainError::Parse {
            format: "legacy buffer",
            message: error.to_string(),
        })?,
    );
    Ok(EditorStateV1 {
        origin: DocumentOrigin::File(editor.path),
        history,
        format: Default::default(),
        scroll_top: usize::try_from(editor.scroll_top.max(0)).unwrap_or(usize::MAX),
        mime: editor.mime,
        language: editor.language,
        grammar_language: editor.grammar_language,
        grammar_available: editor.grammar_available,
        read_only: false,
        legacy_format: true,
        last_modified_millis: editor.last_modified_millis,
        disk_identity: None,
        extra,
    })
}
pub fn import(bytes: &[u8]) -> Result<ProjectSessionV1> {
    let legacy: LegacySession =
        serde_json::from_slice(bytes).map_err(|error| DomainError::Parse {
            format: "Kotlin session",
            message: error.to_string(),
        })?;
    if legacy.version != 6 {
        return Err(DomainError::Unsupported(format!(
            "Kotlin session version {}; supported version is 6",
            legacy.version
        )));
    }
    let open_editors = legacy
        .open_editors
        .into_iter()
        .map(editor)
        .collect::<Result<Vec<_>>>()?;
    let recent_files = legacy
        .recent_files
        .into_iter()
        .map(|recent| {
            Ok(RecentFileV1 {
                path: recent.path,
                last_opened_epoch_millis: recent.last_opened_epoch_millis,
                last_modified_millis: recent.last_modified_millis,
                editor: recent.editor.map(editor).transpose()?,
                dirty: recent.dirty,
                viewer_type: recent.viewer_type,
                extra: recent.extra,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut extra = legacy.extra;
    preserve_collisions(
        &mut extra,
        &["ignorePatterns", "activeOrigin", "importedFrom"],
        "legacySessionFields",
    );
    Ok(ProjectSessionV1 {
        open_editors,
        recent_files,
        source_roots: legacy.source_roots,
        scan_exclusions: legacy.scan_exclusions,
        database: legacy.database,
        rest_api: legacy.rest_api,
        extra,
        imported_from: Some(".kode.json (version 6)".into()),
        ..Default::default()
    })
}
