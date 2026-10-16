use super::{EditorStateV1, resolve_path};
use kode_core::{
    DocumentId, DocumentOrigin, JobTag, Result, Selection,
    document::{
        Document, TextSnapshot,
        encoding::{DecodedText, decode},
    },
    editor::selection::CaretState,
    services::{DiskIdentity, DocumentIo},
};
use std::path::Path;

fn remap_caret(old: &TextSnapshot, new: &TextSnapshot, caret: &CaretState) -> Result<CaretState> {
    let remap = |selection: Selection| -> Result<Selection> {
        Ok(Selection {
            head: new.utf16_to_byte_clamped(old.byte_to_utf16(selection.head)?),
            anchor: new.utf16_to_byte_clamped(old.byte_to_utf16(selection.anchor)?),
        })
    };
    CaretState {
        selection: remap(caret.selection)?,
        secondary: caret
            .secondary
            .iter()
            .copied()
            .map(remap)
            .collect::<Result<_>>()?,
    }
    .normalized(new)
}

fn replace_legacy_text(
    buffer: &mut kode_core::editor::history::PersistedBuffer,
    text: String,
) -> Result<()> {
    if buffer.text == text {
        return Ok(());
    }
    let origin = DocumentOrigin::Virtual {
        owner: "session-conversion".into(),
        key: "buffer".into(),
        label: String::new(),
    };
    let old = Document::new(DocumentId(0), origin.clone(), None, &buffer.text).snapshot();
    let new = Document::new(DocumentId(0), origin, None, &text).snapshot();
    let caret = remap_caret(
        &old,
        &new,
        &CaretState {
            selection: buffer.selection,
            secondary: buffer.secondary.clone(),
        },
    )?;
    buffer.selection = caret.selection;
    buffer.secondary = caret.secondary;
    buffer.text = text;
    Ok(())
}
fn crlf_legacy(buffer: &mut kode_core::editor::history::PersistedBuffer) -> Result<()> {
    let mut text = String::with_capacity(buffer.text.len());
    let mut previous = None;
    for character in buffer.text.chars() {
        if character == '\n' && previous != Some('\r') {
            text.push('\r');
        }
        text.push(character);
        previous = Some(character);
    }
    replace_legacy_text(buffer, text)
}

pub struct RestoredEditor {
    pub document: Document,
    pub caret: CaretState,
    pub scroll_top: usize,
    pub disk_identity: Option<DiskIdentity>,
    pub stale: bool,
    /// The caller must persist this recovery before discarding an old dirty view.
    pub recovery: Option<EditorStateV1>,
    /// Missing/unreadable file feedback never discards persisted dirty content.
    pub disk_error: Option<kode_core::DomainError>,
}
pub struct PreparedRestore {
    pub editor: RestoredEditor,
    pub recovery_path: Option<std::path::PathBuf>,
}
/// Worker entrypoint for shell startup/open. Recovery reaches disk before a
/// stale dirty buffer is replaced; a failed recovery prevents publication.
pub fn restore_with_recovery(
    store: &super::persistence::SessionStore,
    state: &EditorStateV1,
    tag: JobTag,
    io: &dyn DocumentIo,
) -> Result<PreparedRestore> {
    let editor = restore(&store.root, state, tag, io)?;
    let recovery_path = editor
        .recovery
        .as_ref()
        .map(|state| store.recover_editor(state))
        .transpose()?;
    Ok(PreparedRestore {
        editor,
        recovery_path,
    })
}
pub fn is_stale(stored_millis: Option<i64>, current_millis: Option<i64>) -> bool {
    match (stored_millis, current_millis) {
        (Some(stored), Some(current)) => current > stored.saturating_add(1_000),
        _ => false,
    }
}
/// Worker-only: current disk bytes establish the conflict baseline, while dirty
/// history is restored only when it is safe to apply to that file generation.
pub fn restore(
    root: &Path,
    state: &EditorStateV1,
    tag: JobTag,
    io: &dyn DocumentIo,
) -> Result<RestoredEditor> {
    let mut origin = state.origin.clone();
    if let DocumentOrigin::File(path) = &mut origin {
        *path = resolve_path(root, path)?;
    }
    let id = DocumentId::fresh()?;
    let mut disk_error = None;
    let loaded = if matches!(origin, DocumentOrigin::File(_)) {
        match io.load(&origin, tag) {
            Ok(loaded) => match decode(&loaded.bytes) {
                Ok(decoded) => Some((decoded, loaded.disk_identity)),
                Err(error) => {
                    disk_error = Some(error);
                    None
                }
            },
            Err(error) => {
                disk_error = Some(error);
                None
            }
        }
    } else {
        None
    };
    let current_millis = loaded
        .as_ref()
        .and_then(|(_, identity)| identity.as_ref())
        .and_then(|identity| identity.modified_nanos)
        .and_then(|nanos| i64::try_from(nanos / 1_000_000).ok());
    let changed_identity = state
        .disk_identity
        .as_ref()
        .zip(loaded.as_ref().and_then(|(_, identity)| identity.as_ref()))
        .is_some_and(|(old, new)| {
            old.content_hash != new.content_hash
                || old.length != new.length
                || old.modified_nanos != new.modified_nanos
        });
    let mut history = state.history.clone();
    let mut saved_document = Document::from_decoded(
        id,
        origin.clone(),
        state.language.clone(),
        DecodedText {
            text: history.current.text.clone(),
            format: state.format,
        },
    );
    let mut old_caret = CaretState::default();
    saved_document.import_history(history.clone(), &mut old_caret)?;
    let changed_baseline = loaded.as_ref().is_some_and(|(decoded, _)| {
        // Inferred legacy saved text is not exact byte/hash evidence. Native
        // captures carry an actual disk baseline and can compare it directly.
        !state.legacy_format && history.saved_baseline_known && history.saved_text != decoded.text
    });
    let stale = loaded.is_some()
        && (changed_identity
            || changed_baseline
            || is_stale(state.last_modified_millis, current_millis));
    let recovery = (stale && saved_document.is_dirty()).then(|| state.clone());
    if stale {
        let (decoded, disk_identity) = loaded.expect("stale requires disk bytes");
        let mut document = Document::from_decoded(id, origin, state.language.clone(), decoded);
        let old_snapshot = saved_document.snapshot();
        let new_snapshot = document.snapshot();
        let caret = remap_caret(&old_snapshot, &new_snapshot, &old_caret)?;
        document.set_read_only(state.read_only);
        return Ok(RestoredEditor {
            document,
            caret,
            scroll_top: state.scroll_top,
            disk_identity,
            stale,
            recovery,
            disk_error,
        });
    }
    let (format, disk_identity) = match loaded {
        Some((decoded, identity)) => {
            if state.legacy_format {
                if decoded.format.line_ending == kode_core::document::encoding::LineEnding::CrLf {
                    for buffer in std::iter::once(&mut history.current)
                        .chain(&mut history.undo)
                        .chain(&mut history.redo)
                    {
                        crlf_legacy(buffer)?;
                    }
                }
                if !saved_document.is_dirty() {
                    // Clean normalized sessions use exact current disk bytes,
                    // retaining their converted undo/redo instead of becoming
                    // dirty merely because the source discarded CRLF/BOM data.
                    replace_legacy_text(&mut history.current, decoded.text.clone())?;
                }
            }
            if state.legacy_format || !history.saved_baseline_known {
                // Legacy sessions lack saved bytes/BOM metadata; disk establishes
                // the real baseline without changing their current dirty text.
                history.saved_text = decoded.text;
                history.saved_baseline_known = true;
            }
            (decoded.format, identity)
        }
        None => (state.format, None),
    };
    let mut document = Document::from_decoded(
        id,
        origin,
        state.language.clone(),
        DecodedText {
            text: history.current.text.clone(),
            format,
        },
    );
    let mut caret = CaretState::default();
    document.import_history(history, &mut caret)?;
    document.set_read_only(state.read_only);
    Ok(RestoredEditor {
        document,
        caret,
        scroll_top: state.scroll_top,
        disk_identity,
        stale,
        recovery,
        disk_error,
    })
}
