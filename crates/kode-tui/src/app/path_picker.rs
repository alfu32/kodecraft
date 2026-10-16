//! Field-owned path choice. Selecting only changes a draft; its owner submits the operation.
use super::state::{AppState, AppWork};
use crate::{
    editor::host::ClipboardJob,
    focus::FocusId,
    panels::{database::session_dialog, rest::sidebar},
    widgets::{
        field::FieldAction,
        file_picker::{self, FilePicker, Prepared, Request},
    },
};
use kode_core::{DomainError, JobTag, RequestId, Result};
use kode_platform::jobs::CancellationToken;
use std::path::Path;

#[derive(Clone)]
pub enum Target {
    RestForm(crate::panels::rest::editor::form::PathProof),
    Database(crate::panels::database::connection_dialog::PathProof),
    Sql {
        proof: session_dialog::PasteProof,
        focus: usize,
    },
    Rest(sidebar::PathProof),
}
pub struct Pending {
    tag: JobTag,
    request: Request,
    target: Target,
    focus: Option<FocusId>,
    cancel: CancellationToken,
}
pub struct Active {
    pub picker: FilePicker,
    target: Target,
}
#[derive(Clone)]
pub struct PasteProof {
    pub request: RequestId,
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
}
impl Target {
    fn current(&self, state: &AppState) -> bool {
        match self {
            Self::RestForm(proof) => state
                .rest
                .form
                .as_ref()
                .is_some_and(|form| form.accepts_path(proof, &state.rest.model)),
            Self::Database(proof) => state
                .database
                .form
                .as_ref()
                .is_some_and(|form| form.accepts_path(proof)),
            Self::Sql { proof, focus } => state.sql.dialog.as_ref().is_some_and(|dialog| {
                matches!(dialog.mode, session_dialog::Mode::Save)
                    && !dialog.busy
                    && !dialog.dialog.finished
                    && dialog.nonce == proof.nonce
                    && dialog.dialog.focus == *focus
                    && dialog.dialog.field.value == proof.value
                    && dialog.dialog.field.cursor == proof.cursor
                    && dialog.dialog.field.anchor == proof.anchor
            }),
            Self::Rest(proof) => state.rest.sidebar.accepts_path(proof, &state.rest.model),
        }
    }
    fn apply(&self, state: &mut AppState, path: &Path) -> Result<()> {
        if !self.current(state) {
            return Ok(());
        }
        let Some(value) = path.to_str() else {
            return Ok(());
        };
        match self {
            Self::RestForm(proof) => {
                if let Some(form) = &mut state.rest.form {
                    form.apply_path(proof, &mut state.rest.model, path)?;
                }
            }
            Self::Database(proof) => {
                if let Some(form) = &mut state.database.form {
                    form.apply_path(proof, path);
                }
            }
            Self::Sql { .. } => {
                if let Some(dialog) = &mut state.sql.dialog {
                    if value.chars().count() > dialog.dialog.field.max_chars {
                        return Err(DomainError::Unsupported(
                            "Selected SQL path exceeds the field size limit".into(),
                        ));
                    }
                    dialog.dialog.field.value = value.into();
                    dialog.dialog.field.cursor = value.len();
                    dialog.dialog.field.anchor = None;
                }
            }
            Self::Rest(proof) => {
                state
                    .rest
                    .sidebar
                    .apply_path(proof, &state.rest.model, path);
            }
        }
        Ok(())
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl AppState {
    pub(super) fn start_path_picker(&mut self, request: Request, target: Target) -> Result<()> {
        if !target.current(self) || self.path_picker.is_some() || self.closing {
            return Ok(());
        }
        self.pending_path = None;
        let tag = self.tag()?;
        let cancel = CancellationToken::new();
        self.work.push(AppWork::PathPrepare {
            tag,
            base: self.project.root.clone(),
            value: request.value.clone(),
            cancel: cancel.clone(),
        });
        self.pending_path = Some(Pending {
            tag,
            request,
            target,
            focus: self.focus.focused,
            cancel,
        });
        Ok(())
    }
    pub(super) fn complete_path_prepare(
        &mut self,
        tag: JobTag,
        result: Result<Prepared>,
    ) -> Result<()> {
        if self
            .pending_path
            .as_ref()
            .is_none_or(|pending| pending.tag != tag)
        {
            return Ok(());
        }
        let pending = self.pending_path.take().expect("matched pending path");
        if tag.workspace != self.generation
            || self.closing
            || !self.window_focused
            || self.focus.focused != pending.focus
            || !pending.target.current(self)
        {
            return Ok(());
        }
        match result {
            Ok(prepared) => {
                self.path_picker = Some(Active {
                    picker: FilePicker::new(pending.request.clone(), prepared, &mut self.focus)?,
                    target: pending.target.clone(),
                })
            }
            Err(DomainError::Cancelled) => {}
            Err(error) => self.notice = error.to_string(),
        }
        Ok(())
    }
    pub(super) fn complete_path_work(
        &mut self,
        request: RequestId,
        tag: JobTag,
        result: Result<file_picker::Report>,
    ) -> Result<()> {
        if !self.window_focused || self.closing {
            return Ok(());
        }
        if let Some(active) = &mut self.path_picker
            && active.picker.request.id == request
        {
            active.picker.complete(tag, result, &mut self.focus)?;
        } else if let Some((_, picker)) = &mut self.picker
            && picker.request.id == request
        {
            picker.complete(tag, result, &mut self.focus)?;
        }
        self.drain_path_picker()
    }
    pub(super) fn cancel_path_picker(&mut self) {
        self.pending_path = None;
        self.pending_picker = None;
        if let Some((_, mut picker)) = self.picker.take() {
            picker.cancel(&mut self.focus);
        }
        if let Some(mut active) = self.path_picker.take() {
            active.picker.cancel(&mut self.focus);
        }
        self.work.retain(|work| {
            !matches!(
                work,
                AppWork::PathPrepare { .. }
                    | AppWork::Picker { .. }
                    | AppWork::PathWork { .. }
                    | AppWork::PathClipboard { .. }
            )
        });
    }
    pub(super) fn drain_path_picker(&mut self) -> Result<()> {
        if self.pending_path.as_ref().is_some_and(|pending| {
            !pending.target.current(self)
                || pending.focus != self.focus.focused
                || !self.window_focused
        }) {
            self.pending_path = None;
        }
        if self
            .path_picker
            .as_ref()
            .is_some_and(|active| !active.target.current(self))
        {
            self.cancel_path_picker();
        }
        let picker = if let Some(active) = &mut self.path_picker {
            &mut active.picker
        } else if let Some((_, picker)) = &mut self.picker {
            picker
        } else {
            return Ok(());
        };
        let request = picker.request.id;
        for work in picker.take_work() {
            self.work.push(AppWork::PathWork { request, work });
        }
        let field = &picker.field;
        let proof = PasteProof {
            request,
            value: field.value.clone(),
            cursor: field.cursor,
            anchor: field.anchor,
        };
        let action = picker.take_field_action();
        let finished = picker.finished && self.path_picker.is_some();
        let job = match action {
            Some(FieldAction::Copy(text)) => {
                Some((ClipboardJob::Publish(self.clipboard.remember(&text)?), None))
            }
            Some(FieldAction::PasteRequested) => Some((ClipboardJob::Read, Some(proof))),
            Some(FieldAction::LimitReached) => {
                self.notice = "Path field character limit reached".into();
                None
            }
            _ => None,
        };
        if let Some((job, proof)) = job {
            self.work.push(AppWork::PathClipboard {
                tag: self.tag()?,
                job,
                proof,
            });
        }
        if finished {
            let mut active = self.path_picker.take().expect("finished picker");
            if let Some(path) = active.picker.result.take()
                && let Err(error) = active.target.apply(self, &path)
            {
                self.notice = error.to_string();
            }
        }
        Ok(())
    }
    pub(super) fn paste_path(&mut self, proof: PasteProof, text: String) {
        if !self.window_focused {
            return;
        }
        if let Some(active) = &mut self.path_picker
            && active.picker.request.id == proof.request
            && self.focus.focused == Some(active.picker.id)
        {
            active
                .picker
                .paste(&proof.value, proof.cursor, proof.anchor, text);
        } else if let Some((_, picker)) = &mut self.picker
            && picker.request.id == proof.request
            && self.focus.focused == Some(picker.id)
        {
            picker.paste(&proof.value, proof.cursor, proof.anchor, text);
        }
    }
}
