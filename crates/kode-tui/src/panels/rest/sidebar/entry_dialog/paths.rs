use super::*;
use crate::widgets::file_picker::{Mode, Request};
use kode_core::DomainError;
use std::path::Path;

#[derive(Clone)]
pub struct PathProof {
    nonce: RequestId,
    focus: usize,
    fields: Vec<(String, usize, Option<usize>)>,
}
impl EntryDialog {
    pub fn file_enabled(&self) -> bool {
        self.file_source && !self.variable && self.fields[2].value.trim() == "file"
    }
    pub fn take_browse(&mut self) -> bool {
        std::mem::take(&mut self.browse_requested)
    }
    pub fn path_request(&self) -> Result<Option<(Request, PathProof)>> {
        if self.finished || !self.file_enabled() {
            return Ok(None);
        }
        let value = &self.fields[1].value;
        let initial = if value.trim_start().starts_with('[') {
            let paths: Vec<String> = serde_json::from_str(value).map_err(|_| {
                DomainError::Unsupported(
                    "Multipart file sources must be a path or a JSON array of paths".into(),
                )
            })?;
            paths.last().cloned().unwrap_or_default()
        } else {
            value.clone()
        };
        let request = Request::new(
            "Select multipart file (adds to an existing source array)".into(),
            Mode::OpenFile,
            initial,
        )?;
        Ok(Some((
            request,
            PathProof {
                nonce: self.nonce,
                focus: self.focus,
                fields: self
                    .fields
                    .iter()
                    .map(|field| (field.value.clone(), field.cursor, field.anchor))
                    .collect(),
            },
        )))
    }
    pub fn accepts_path(&self, proof: &PathProof) -> bool {
        self.file_enabled()
            && !self.finished
            && self.nonce == proof.nonce
            && self.focus == proof.focus
            && self
                .fields
                .iter()
                .map(|field| (&field.value, field.cursor, field.anchor))
                .eq(proof
                    .fields
                    .iter()
                    .map(|(value, cursor, anchor)| (value, *cursor, *anchor)))
    }
    pub fn apply_path(&mut self, proof: &PathProof, path: &Path) -> Result<bool> {
        if !self.accepts_path(proof) {
            return Ok(false);
        }
        let Some(value) = path.to_str() else {
            return Ok(false);
        };
        let field = &mut self.fields[1];
        let updated = if field.value.trim_start().starts_with('[') {
            let mut paths: Vec<String> = serde_json::from_str(&field.value).map_err(|_| {
                DomainError::Unsupported("Invalid multipart file source array".into())
            })?;
            if !paths.iter().any(|path| path == value) {
                paths.push(value.into());
            }
            serde_json::to_string(&paths)
                .map_err(|error| DomainError::Unsupported(error.to_string()))?
        } else if value.trim_start().starts_with('[') {
            // This field also accepts JSON arrays; keep a literal '[' filename unambiguous.
            serde_json::to_string(&vec![value])
                .map_err(|error| DomainError::Unsupported(error.to_string()))?
        } else {
            value.into()
        };
        if updated.chars().count() > field.max_chars {
            return Err(DomainError::Unsupported(
                "Multipart file source exceeds the field size limit".into(),
            ));
        }
        field.cursor = updated.len();
        field.anchor = None;
        field.value = updated;
        Ok(true)
    }
}
