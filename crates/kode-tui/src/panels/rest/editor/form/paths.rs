use super::*;
use crate::widgets::file_picker::{Mode, Request};
use std::path::Path;

#[derive(Clone)]
enum Target {
    Binary {
        value: String,
        cursor: usize,
        anchor: Option<usize>,
    },
    Multipart(crate::panels::rest::sidebar::entry_dialog::PathProof),
}
#[derive(Clone)]
pub struct PathProof {
    key: String,
    path: NodePath,
    epoch: u64,
    expected: Arc<Value>,
    target: Target,
}
impl Form {
    fn binary_file(&self, model: &Workspace) -> bool {
        self.tab == Tab::Body
            && self.request
            && self.modal.is_none()
            && model
                .node(&self.path)
                .and_then(|node| node.pointer("/request/body/mode"))
                .and_then(Value::as_str)
                == Some("file")
    }
    pub(super) fn request_path(&mut self, model: &Workspace) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let (request, target) = if let Some(modal) = &self.modal {
            if !matches!(modal.purpose, ModalPurpose::Row(Rows::FormData, _))
                || !Arc::ptr_eq(&modal.expected, &model.export_collection())
            {
                return Ok(());
            }
            let Some((request, proof)) = modal.dialog.path_request()? else {
                return Ok(());
            };
            (request, Target::Multipart(proof))
        } else if self.binary_file(model) && self.focused == Some(Field::File) {
            let field = &self.fields[&Field::File];
            (
                Request::new(
                    "Select binary request body file".into(),
                    Mode::OpenFile,
                    field.value.clone(),
                )?,
                Target::Binary {
                    value: field.value.clone(),
                    cursor: field.cursor,
                    anchor: field.anchor,
                },
            )
        } else {
            return Ok(());
        };
        self.intents.push(Intent::Browse {
            request,
            proof: PathProof {
                key: self.key.clone(),
                path: self.path.clone(),
                epoch: self.epoch,
                expected: model.export_collection(),
                target,
            },
        });
        Ok(())
    }
    pub fn accepts_path(&self, proof: &PathProof, model: &Workspace) -> bool {
        if self.closed
            || self.key != proof.key
            || self.path != proof.path
            || self.epoch != proof.epoch
            || !Arc::ptr_eq(&proof.expected, &model.export_collection())
        {
            return false;
        }
        match &proof.target {
            Target::Binary {
                value,
                cursor,
                anchor,
            } => {
                self.binary_file(model)
                    && self.focused == Some(Field::File)
                    && self.fields.get(&Field::File).is_some_and(|field| {
                        field.value == *value && field.cursor == *cursor && field.anchor == *anchor
                    })
            }
            Target::Multipart(proof) => self.modal.as_ref().is_some_and(|modal| {
                matches!(modal.purpose, ModalPurpose::Row(Rows::FormData, _))
                    && modal.dialog.accepts_path(proof)
            }),
        }
    }
    pub fn apply_path(
        &mut self,
        proof: &PathProof,
        model: &mut Workspace,
        path: &Path,
    ) -> Result<bool> {
        if !self.accepts_path(proof, model) {
            return Ok(false);
        }
        match &proof.target {
            Target::Binary { .. } => {
                let Some(value) = path.to_str() else {
                    return Ok(false);
                };
                let field = self
                    .fields
                    .get_mut(&Field::File)
                    .expect("binary file field");
                if value.chars().count() > field.max_chars {
                    return Err(invalid("Binary file path exceeds the field size limit"));
                }
                editing::field(model, &self.path, Field::File, value)?;
                field.value = value.into();
                field.cursor = value.len();
                field.anchor = None;
                self.intents.push(Intent::Changed);
                Ok(true)
            }
            Target::Multipart(proof) => self
                .modal
                .as_mut()
                .expect("multipart dialog")
                .dialog
                .apply_path(proof, path),
        }
    }
}
