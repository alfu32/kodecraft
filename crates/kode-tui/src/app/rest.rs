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
            && owner == "rest-environment"
            && self.rest.model.environment(&key).is_some()
        {
            self.open_rest_environment(&key, None)?;
        }
        Ok(())
    }
    pub fn open_rest_environment(
        &mut self,
        id: &str,
        override_text: Option<kode_core::document::snapshot::TextSnapshot>,
    ) -> Result<()> {
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
        if self.rest.model.node(&path).is_none() {
            return Ok(());
        }
        self.rest.model.select(&path);
        self.rest.sidebar.cursor = path.clone();
        let mut descriptor = HostDescriptor::new(
            HostRole::RawRequest,
            "rest".into(),
            format!("node:{}", path.encode()),
            "REST collection JSON snapshot".into(),
            self.project.root.clone(),
        );
        descriptor.language = Some("json".into());
        descriptor.grammar = "json".into();
        let expected = self.rest.model.export_collection();
        let data = OpenData::Node {
            collection: expected.clone(),
            path: path.clone(),
        };
        let tag = self.tag()?;
        self.rest.pending_open = Some(tag);
        self.rest.ready_open = None;
        self.pending_open = None;
        self.work.push(AppWork::Rest(Box::new(Work::Open {
            tag,
            descriptor,
            kind: DocumentKind::Node(path),
            data,
            expected: Some(expected),
        })));
        Ok(())
    }
    fn inspect_rest_notices(&mut self) -> Result<()> {
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
        let descriptor = entry.descriptor.clone();
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
        self.focus.focused = Some(view.input.id);
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.editor = Some(view);
        self.splash = false;
        // Read-only snapshots are replaceable views. Environment buffers retain
        // their registry/document identity across request and file switches.
        let stale: Vec<_> = self
            .rest
            .documents
            .iter()
            .filter_map(|(old, entry)| {
                (*old != id && !matches!(entry.kind, DocumentKind::Environment(_))).then_some(*old)
            })
            .collect();
        for old in stale {
            self.documents.remove(old)?;
            self.rest.documents.remove(&old);
            self.views.remove(&old);
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
                Intent::TreeChanged(change) => self.rest.run.tree_changed(change),
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
        if !self.rest.sidebar.has_modal()
            && !self.files.has_modal()
            && !self.database.has_modal()
            && self.sql.dialog.is_none()
            && self.sql.menu.is_none()
            && self.search.is_none()
            && self.picker.is_none()
            && self.rescan.is_none()
            && !self.git.has_modal()
            && let Some(id) = self.rest.ready_open.take()
        {
            self.attach_rest_document(id)?;
        }
        if self.rest.observed_revision != self.rest.model.revision() {
            self.rest.observed_revision = self.rest.model.revision();
            self.changed()?;
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
