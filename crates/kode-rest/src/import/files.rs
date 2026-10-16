use super::{Format, Import, MAX_DOCUMENT_BYTES, invalid, parse};
use crate::model::workspace::validate_collection;
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, Result, Revision,
    intelligence::DocumentSnapshot,
    services::{DiskIdentity, DocumentIo},
};
use kode_platform::files::{NativeDocumentIo, open_regular_file};
use kode_platform::jobs::CancellationToken;
use std::{io::Read, path::Path, sync::Arc};

/// Blocking worker I/O. The UI receives only a completely parsed import.
pub fn read_file(path: &Path, cancel: &CancellationToken) -> Result<Import> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let mut file = open_regular_file(path)?;
    let io_error = |source| DomainError::Io {
        operation: "read REST import",
        path: path.into(),
        source,
    };
    let before = file.metadata().map_err(io_error)?;
    if before.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err(invalid("/", "document exceeds the 16 MiB import limit"));
    }
    let mut bytes = Vec::with_capacity((before.len() as usize).min(MAX_DOCUMENT_BYTES));
    let mut chunk = [0; 64 * 1024];
    loop {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let n = file.read(&mut chunk).map_err(io_error)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_DOCUMENT_BYTES {
            return Err(invalid("/", "document exceeds the 16 MiB import limit"));
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let after = file.metadata().map_err(io_error)?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(DomainError::SaveConflict(path.into()));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid("/", "document must be UTF-8"))?;
    let format = match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("json") => Format::Json,
        _ => Format::Auto,
    };
    let parsed = parse(text, format).map_err(|error| match error {
        DomainError::Parse { format, message } => DomainError::Parse {
            format,
            message: format!("{}: {message}", path.display()),
        },
        other => other,
    })?;
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    Ok(parsed)
}

/// Guarded atomic Postman export. `None` creates a new file; replacement requires
/// the identity from a previous successful export or an explicit target capture.
/// Only collection data is accepted: runtime results/cookies are separate owners.
pub fn export_file(
    path: &Path,
    collection: &serde_json::Value,
    expected: Option<&DiskIdentity>,
    cancel: &CancellationToken,
) -> Result<DiskIdentity> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    validate_collection(collection)?;
    super::parse::validate_document(collection)?;
    struct Buffer(Vec<u8>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_DOCUMENT_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("REST export size limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Buffer(Vec::new());
    serde_json::to_writer_pretty(&mut output, collection)
        .map_err(|_| invalid("/", "export exceeds the 16 MiB import/export limit"))?;
    let text =
        String::from_utf8(output.0).map_err(|_| invalid("/", "cannot serialize collection"))?;
    let snapshot = DocumentSnapshot {
        id: DocumentId::fresh()?,
        revision: Revision(0),
        origin: DocumentOrigin::File(path.into()),
        language: Some("json".into()),
        format: Default::default(),
        text: Arc::from(text),
    };
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    if expected.is_none()
        && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| DomainError::Io {
            operation: "create REST export directory",
            path: parent.into(),
            source,
        })?;
    }
    NativeDocumentIo.save(path, &snapshot, expected)
}
