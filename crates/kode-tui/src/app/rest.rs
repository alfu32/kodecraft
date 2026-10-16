use super::state::{AppState, AppWork};
use crate::{
    editor::host::{EditorView, HostDescriptor, HostRole, QueuedEditorHost},
    panels::rest::{
        sidebar::Intent,
        state::{DocumentKind, Entry, OpenData, Work},
    },
    widgets::field::FieldAction,
};
use kode_core::{DocumentId, DocumentOrigin, Result};
use std::sync::Arc;
impl AppState {
    pub(super) fn restore_rest(&mut self) -> Result<()> {
        if let Some(warning) = self.rest.warning.clone() {
            self.notice = warning;
        }
        if self.pending_open.is_none()
            && let Some(DocumentOrigin::Virtual { owner, key, .. }) =
                self.persistence.session().active_origin.clone()
        {
            if owner == "rest-environment" && self.rest.model.environment(&key).is_some() {
                self.open_rest_environment(&key, None)?;
            } else if owner == "rest-node"
                && let Ok(path) = kode_rest::model::NodePath::decode(Some(&key))
            {
                self.open_rest_node(path)?;
            }
        }
        Ok(())
    }
    pub fn open_rest_environment(
        &mut self,
        id: &str,
        override_text: Option<kode_core::document::snapshot::TextSnapshot>,
    ) -> Result<()> {
        self.deactivate_rest_form();
        self.rest.open_focus = Some((self.focus.focused, true));
        if let Some(document) = self.rest.environment_document(id) {
            self.rest.pending_open = None;
            self.rest.ready_open = None;
            self.pending_open = None;
            return self.attach_rest_document(document);
        }
        let Some(environment) = self.rest.model.environment(id).cloned() else {
            return Ok(());
        };
        if let Some(snapshot) = &override_text {
            self.rest.staged_buffers.insert(id.into(), snapshot.clone());
        }
        let override_text = override_text.or_else(|| self.rest.staged_buffers.get(id).cloned());
        let mut descriptor = HostDescriptor::new(
            HostRole::Environment,
            "rest-environment".into(),
            id.into(),
            format!("Environment — {}", environment.name),
            self.project.root.clone(),
        );
        descriptor.language = Some("properties".into());
        descriptor.grammar = "properties".into();
        let data = OpenData::Environment {
            environment: Box::new(environment),
            draft: self.rest.drafts.get(id).cloned(),
            override_text,
        };
        let tag = self.tag()?;
        self.rest.pending_open = Some(tag);
        self.rest.ready_open = None;
        self.pending_open = None;
        self.work.push(AppWork::Rest(Box::new(Work::Open {
            tag,
            descriptor,
            kind: DocumentKind::Environment(id.into()),
            data,
            expected: None,
        })));
        Ok(())
    }
    pub fn open_rest_node(&mut self, path: kode_rest::model::NodePath) -> Result<()> {
        self.begin_rest_form(path)
    }
    pub(super) fn inspect_rest_notices(&mut self) -> Result<()> {
        self.deactivate_rest_form();
        self.rest.open_focus = Some((self.focus.focused, true));
        let descriptor = HostDescriptor::new(
            HostRole::RawRequest,
            "rest".into(),
            "import-notices".into(),
            "OpenAPI import notices".into(),
            self.project.root.clone(),
        );
        let tag = self.tag()?;
        self.rest.pending_open = Some(tag);
        self.rest.ready_open = None;
        self.pending_open = None;
        self.work.push(AppWork::Rest(Box::new(Work::Open {
            tag,
            descriptor,
            kind: DocumentKind::Notices,
            data: OpenData::Notices(self.rest.model.export_collection()),
            expected: None,
        })));
        Ok(())
    }
    pub(super) fn attach_rest_document(&mut self, id: DocumentId) -> Result<()> {
        let Some(entry) = self.rest.documents.get(&id) else {
            return Ok(());
        };
        let previous_focus = self.focus.focused;
        let previous_editor_focus = self.editor.as_ref().map(|view| view.input.id);
        let transfer = self
            .rest
            .open_focus
            .take()
            .is_none_or(|(_, transfer)| transfer);
        let descriptor = entry.descriptor.clone();
        let kind = entry.kind.clone();
        let marker = self.rest.markers.get(&id).cloned();
        if marker.is_some() || !matches!(kind, DocumentKind::Buffer { .. }) {
            let runner = (self.rest.runner.visible, self.rest.runner.focus_after_open);
            self.deactivate_rest_form();
            if marker.is_some() {
                (self.rest.runner.visible, self.rest.runner.focus_after_open) = runner;
            }
        }
        self.cache_main_view()?;
        self.clear_diff()?;
        self.detach_image();
        self.active_hex = None;
        let mut view = EditorView::new(
            id,
            &self.documents,
            QueuedEditorHost::new(descriptor),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        if let Some(stored) = self.views.get(&id) {
            view.input.state.caret = stored.caret.clone();
            view.input.state.scroll_rows = stored.scroll_rows;
        }
        if let Some(mut old) = self.editor.take() {
            old.detach(&mut self.focus);
            let owner = old.owner();
            self.work.extend(
                old.take_work()
                    .into_iter()
                    .map(|work| AppWork::Editor { owner, work }),
            );
        }
        if matches!(
            kind,
            DocumentKind::Buffer {
                kind: crate::panels::rest::editor::form::BufferKind::Headers,
                ..
            }
        ) {
            view.host.external = Some(Arc::new(crate::panels::rest::editor::headers::HeaderNames));
            view.host.warnings = self
                .rest
                .buffer_warnings
                .get(&id)
                .cloned()
                .unwrap_or_default();
        }
        self.focus.focused = Some(view.input.id);
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.editor = Some(view);
        let mut restore_buffer = None;
        if let Some(key) = marker {
            self.rest.form = self.rest.forms.remove(&key);
            if let Some(form) = &self.rest.form {
                self.focus.focused = Some(form.id);
                restore_buffer = form.selected_buffer;
            }
        }
        if self.rest.runner.visible && self.rest.runner.focus_after_open {
            self.focus.focused = Some(self.rest.runner.id);
            self.rest.runner.focus_after_open = false;
        }
        self.splash = false;
        // Read-only snapshots are replaceable views. Environment buffers retain
        // their registry/document identity across request and file switches.
        let stale: Vec<_> = self
            .rest
            .documents
            .iter()
            .filter_map(|(old, entry)| {
                (*old != id
                    && matches!(
                        entry.kind,
                        DocumentKind::Node(_)
                            | DocumentKind::Notices
                            | DocumentKind::Buffer {
                                kind: crate::panels::rest::editor::form::BufferKind::RawRequest,
                                ..
                            }
                    ))
                .then_some(*old)
            })
            .collect();
        for old in stale {
            self.documents.remove(old)?;
            self.rest.documents.remove(&old);
            self.rest.markers.remove(&old);
            self.rest.buffer_warnings.remove(&old);
            self.views.remove(&old);
        }
        if !transfer {
            self.focus.focused =
                if previous_focus.is_some() && previous_focus == previous_editor_focus {
                    self.editor.as_ref().map(|view| view.input.id)
                } else {
                    previous_focus
                };
        }
        if let Some(kind) = restore_buffer {
            self.open_rest_buffer(kind)?;
            if (!transfer || self.focus.focused == Some(self.rest.runner.id))
                && let Some((_, transfer)) = &mut self.rest.open_focus
            {
                *transfer = false;
            }
        }
        self.changed()
    }
    pub(super) fn drain_rest(&mut self) -> Result<()> {
        for intent in self.rest.sidebar.take_intents() {
            match intent {
                Intent::Open(path) => self.open_rest_node(path)?,
                Intent::Environment(id) => self.open_rest_environment(&id, None)?,
                Intent::CopiedEnvironment { source, target } => {
                    let snapshot = self
                        .rest
                        .environment_document(&source)
                        .and_then(|id| self.documents.document(id).ok())
                        .map(|document| document.snapshot())
                        .or_else(|| self.rest.staged_buffers.get(&source).cloned());
                    if let Some(draft) = self.rest.drafts.get(&source).cloned() {
                        self.rest.drafts.insert(target.clone(), draft);
                    }
                    self.open_rest_environment(&target, snapshot)?;
                }
                Intent::EnvironmentRemoved(id) => {
                    if let Some(document) = self.rest.environment_document(&id) {
                        if self
                            .editor
                            .as_ref()
                            .is_some_and(|view| view.document_id() == document)
                        {
                            if let Some(mut old) = self.editor.take() {
                                old.detach(&mut self.focus);
                            }
                            self.focus.focused = Some(self.rest.sidebar.id);
                        }
                        self.documents.remove(document)?;
                        self.rest.documents.remove(&document);
                        self.rest.pending_environments.remove(&document);
                        self.views.remove(&document);
                    }
                    self.rest.drafts.remove(&id);
                    self.rest.staged_buffers.remove(&id);
                }
                Intent::Run(path) => self.run_rest_selection(path)?,
                Intent::TreeChanged(change) => {
                    if let Some(form) = &mut self.rest.form {
                        form.rebase(&change, &mut self.focus);
                    }
                    for form in self.rest.forms.values_mut() {
                        form.rebase(&change, &mut self.focus);
                    }
                    self.rest.forms.retain(|_, form| !form.closed);
                    self.rest.request_drafts = std::mem::take(&mut self.rest.request_drafts)
                        .into_iter()
                        .filter_map(|((path, kind), text)| {
                            let path = match &change {
                                crate::panels::rest::sidebar::TreeChange::Inserted(inserted) => {
                                    Some(path.after_insert(inserted))
                                }
                                crate::panels::rest::sidebar::TreeChange::Removed(removed) => {
                                    path.after_remove(removed)
                                }
                                crate::panels::rest::sidebar::TreeChange::Moved { from, to }
                                    if path.starts_with(from) =>
                                {
                                    let mut moved = to.clone();
                                    moved.0.extend_from_slice(&path.0[from.0.len()..]);
                                    Some(moved)
                                }
                                crate::panels::rest::sidebar::TreeChange::Moved { from, to } => {
                                    path.after_remove(from).map(|path| path.after_insert(to))
                                }
                                crate::panels::rest::sidebar::TreeChange::Replaced => None,
                            }?;
                            Some(((path, kind), text))
                        })
                        .collect();
                    self.rest.run.tree_changed(change);
                    let stale: Vec<_> = self
                        .rest
                        .documents
                        .iter()
                        .filter_map(|(id, entry)| {
                            let dead = match &entry.kind {
                                DocumentKind::Buffer { key, .. } => {
                                    self.rest.path_for_key(key).is_none()
                                }
                                DocumentKind::Node(_) => self
                                    .rest
                                    .markers
                                    .get(id)
                                    .is_some_and(|key| self.rest.path_for_key(key).is_none()),
                                _ => false,
                            };
                            dead.then_some(*id)
                        })
                        .collect();
                    for id in stale {
                        if self
                            .editor
                            .as_ref()
                            .is_some_and(|view| view.document_id() == id)
                            && let Some(mut old) = self.editor.take()
                        {
                            old.detach(&mut self.focus);
                        }
                        self.documents.remove(id)?;
                        self.rest.documents.remove(&id);
                        self.views.remove(&id);
                        self.rest.markers.remove(&id);
                        self.rest.pending_changes.remove(&id);
                        self.rest.dirty_buffers.remove(&id);
                        self.rest.buffer_warnings.remove(&id);
                    }
                    self.rest
                        .buffers
                        .retain(|_, id| self.rest.documents.contains_key(id));
                    if self.rest.form.as_ref().is_some_and(|form| form.closed) {
                        self.deactivate_rest_form();
                        self.focus.focused = Some(self.rest.sidebar.id);
                        self.notice = "REST item removed".into();
                    }
                }
                Intent::InspectNotices => self.inspect_rest_notices()?,
                Intent::Import {
                    nonce,
                    kind,
                    path,
                    parent,
                    expected,
                } => {
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.project.root.join(path)
                    };
                    let tag = self.tag()?;
                    self.work.push(AppWork::Rest(Box::new(Work::Import {
                        tag,
                        nonce,
                        kind,
                        path,
                        parent,
                        expected,
                    })));
                }
                Intent::Export {
                    nonce,
                    path,
                    collection,
                } => {
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.project.root.join(path)
                    };
                    let expected = self.rest.exports.get(&path).cloned();
                    let tag = self.tag()?;
                    self.work.push(AppWork::Rest(Box::new(Work::Export {
                        tag,
                        nonce,
                        path,
                        collection,
                        expected,
                    })));
                }
                Intent::Clipboard { action, proof } => {
                    let job = match action {
                        FieldAction::Copy(text) => crate::editor::host::ClipboardJob::Publish(
                            self.clipboard.remember(&text)?,
                        ),
                        FieldAction::PasteRequested => crate::editor::host::ClipboardJob::Read,
                        _ => continue,
                    };
                    let tag = self.tag()?;
                    self.work.push(AppWork::RestClipboard { tag, job, proof });
                }
            }
        }
        self.drain_rest_form()?;
        self.drain_rest_runner()?;
        if !self.closing
            && !self.rest.form.as_ref().is_some_and(|form| form.has_modal())
            && !self.rest.sidebar.has_modal()
            && !self.files.has_modal()
            && !self.database.has_modal()
            && self.sql.dialog.is_none()
            && self.sql.menu.is_none()
            && self.search.is_none()
            && self.picker.is_none()
            && self.rescan.is_none()
            && !self.git.has_modal()
            && self
                .rest
                .open_focus
                .is_none_or(|(expected, _)| expected == self.focus.focused)
            && let Some(id) = self.rest.ready_open.take()
        {
            self.attach_rest_document(id)?;
        }
        if self.rest.observed_revision != self.rest.model.revision() {
            self.rest.observed_revision = self.rest.model.revision();
            self.changed()?;
            if self.rest.pending_open.is_none()
                && self.rest.form.as_ref().is_some_and(|form| {
                    form.selected_buffer
                        == Some(crate::panels::rest::editor::form::BufferKind::RawRequest)
                })
            {
                self.refresh_rest_preview()?;
            }
        }
        Ok(())
    }
    pub fn run_rest_selection(&mut self, selection: kode_rest::model::NodePath) -> Result<()> {
        use crate::panels::rest::run::{EnvironmentText, Prepare};
        if self.closing {
            return Ok(());
        }
        if self.rest.run.running.is_some() || self.rest.run.pending.is_some() {
            self.rest.run.pending = None;
            self.rest.run.cancel();
            self.rest.sidebar.message = "Stopping REST run…".into();
            return Ok(());
        }
        if let Some(form) = self
            .rest
            .form
            .as_mut()
            .filter(|form| form.path == selection)
        {
            form.message.clear();
        }
        self.rest.run.scope = Some(selection.clone());
        self.rest.runner.reset();
        if self
            .rest
            .model
            .node(&selection)
            .is_some_and(|node| !kode_rest::model::editing::is_request(node, &selection))
        {
            self.open_rest_node(selection.clone())?;
            if let Some((_, transfer)) = &mut self.rest.open_focus {
                *transfer = false;
            }
            self.rest.runner.visible = true;
        }
        let environment = self.rest.model.active_environment().cloned();
        let environment_text = environment.as_ref().and_then(|environment| {
            self.rest
                .environment_document(&environment.id)
                .and_then(|id| self.documents.document(id).ok())
                .map(|document| EnvironmentText::Buffer(document.snapshot()))
                .or_else(|| {
                    self.rest
                        .staged_buffers
                        .get(&environment.id)
                        .cloned()
                        .map(EnvironmentText::Buffer)
                })
                .or_else(|| {
                    self.rest
                        .drafts
                        .get(&environment.id)
                        .cloned()
                        .map(EnvironmentText::Draft)
                })
        });
        let tag = self.tag()?;
        self.rest.run.pending = Some(tag);
        self.rest.sidebar.message = "Preparing REST run…".into();
        self.work
            .push(AppWork::Rest(Box::new(Work::Run(Box::new(Prepare {
                tag,
                structure: self.rest.run.structure,
                overrides: self.request_buffer_captures(),
                drafts: self.rest.request_drafts.clone(),
                selection,
                collection: self.rest.model.export_collection(),
                root: self.project.root.clone(),
                environment,
                environment_text,
                keys: self.rest.run.keys.clone(),
            })))));
        Ok(())
    }
    pub(super) fn rest_environment_changed(
        &mut self,
        snapshot: kode_core::document::snapshot::TextSnapshot,
    ) -> Result<bool> {
        let Some(Entry {
            kind: DocumentKind::Environment(environment),
            ..
        }) = self.rest.documents.get(&snapshot.id)
        else {
            return Ok(false);
        };
        let environment = environment.clone();
        let mut tag = self.tag()?;
        tag.document = Some((snapshot.id, snapshot.revision));
        self.rest.pending_environments.insert(snapshot.id, tag);
        self.work.push(AppWork::Rest(Box::new(Work::Environment {
            tag,
            environment,
            project: self.rest.model.project_state(),
            snapshot,
        })));
        Ok(true)
    }
    pub(super) fn install_rest_prepared(
        &mut self,
        tag: kode_core::JobTag,
        descriptor: HostDescriptor,
        kind: DocumentKind,
        expected: Option<Arc<serde_json::Value>>,
        document: kode_core::document::Document,
    ) -> Result<()> {
        if self.rest.pending_open != Some(tag) || self.closing {
            return Ok(());
        }
        self.rest.pending_open = None;
        if expected
            .is_some_and(|expected| !Arc::ptr_eq(&expected, &self.rest.model.export_collection()))
        {
            self.notice = "REST collection changed while opening; select the item again".into();
            return Ok(());
        }
        if let DocumentKind::Environment(id) = &kind
            && self.rest.model.environment(id).is_none()
        {
            return Ok(());
        }
        let id = self
            .documents
            .insert(document, &descriptor, self.generation, None)?;
        if let DocumentKind::Environment(environment) = &kind {
            self.rest.staged_buffers.remove(environment);
        }
        self.rest.documents.insert(id, Entry { kind, descriptor });
        if matches!(self.rest.documents[&id].kind, DocumentKind::Environment(_)) {
            self.rest_environment_changed(self.documents.document(id)?.snapshot())?;
        }
        self.rest.ready_open = Some(id);
        self.drain_rest()
    }
}
