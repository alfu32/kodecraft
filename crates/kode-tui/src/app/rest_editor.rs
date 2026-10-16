use super::state::{AppState, AppWork};
use crate::{
    editor::host::{ClipboardJob, HostDescriptor, HostExecution, HostRole},
    input::terminal::{InputEvent, Key, PointerAction},
    panels::rest::{
        editor::{
            buffers,
            form::{BufferKind, Form, Intent},
            jobs,
        },
        state::{DocumentKind, Entry, Work},
    },
    theme::Theme,
    widgets::field::FieldAction,
};
use crossterm::event::MouseButton;
use kode_core::{
    DocumentId, DocumentOrigin, JobTag, RequestId, Result,
    document::{Document, snapshot::TextSnapshot},
};
use kode_rest::model::{NodePath, editing};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
use std::sync::Arc;

impl AppState {
    pub(super) fn deactivate_rest_form(&mut self) {
        self.rest.runner.visible = false;
        self.rest.runner.focus_after_open = false;
        if let Some(mut form) = self.rest.form.take() {
            form.invalidate(&mut self.focus);
            if !form.closed {
                self.rest.forms.insert(form.key.clone(), form);
            }
        }
        self.rest.response.cache_view();
        if let Some(view) = &mut self.rest.response.editor {
            view.detach(&mut self.focus);
        }
        self.rest.response.installed = None;
        self.rest.response.pending = None;
        self.rest.response.requested = None;
        self.rest.response.ready = None;
        self.rest.pending_derived = None;
    }
    pub(super) fn rest_model_capture(&self) -> jobs::ModelCapture {
        let environment_text = self
            .rest
            .model
            .active_environment()
            .and_then(|environment| {
                self.rest
                    .environment_document(&environment.id)
                    .and_then(|id| self.documents.document(id).ok())
                    .map(|document| document.snapshot())
                    .or_else(|| self.rest.staged_buffers.get(&environment.id).cloned())
            });
        jobs::ModelCapture {
            project: self.rest.model.project_state(),
            environment_text,
            environment_draft: self
                .rest
                .model
                .active_environment()
                .and_then(|environment| self.rest.drafts.get(&environment.id))
                .cloned(),
            root: self.project.root.clone(),
        }
    }
    pub(super) fn begin_rest_form(&mut self, path: NodePath) -> Result<()> {
        if self.closing || !self.rest.persistence_enabled || self.rest.model.node(&path).is_none() {
            return Ok(());
        }
        self.deactivate_rest_form();
        self.rest.model.select(&path);
        self.rest.sidebar.cursor = path.clone();
        let key = match self.rest.run.keys.get(&path) {
            Some(key) => key.clone(),
            None => {
                let key = format!("rest:{}", RequestId::fresh()?.0);
                Arc::make_mut(&mut self.rest.run.keys).insert(path.clone(), key.clone());
                self.rest.run.live_keys.insert(key.clone());
                key
            }
        };
        let tag = self.tag()?;
        self.rest.pending_open = Some(tag);
        self.rest.ready_open = None;
        self.rest.open_focus = Some((self.focus.focused, true));
        self.pending_open = None;
        let capture = Box::new(self.rest_model_capture());
        self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
            jobs::Job::Form {
                tag,
                key,
                path,
                capture,
            },
        )))));
        Ok(())
    }
    pub(super) fn request_buffer_captures(&self) -> Vec<buffers::Override> {
        self.rest
            .documents
            .iter()
            .filter_map(|(id, entry)| {
                let DocumentKind::Buffer { key, kind } = &entry.kind else {
                    return None;
                };
                let document = self.documents.document(*id).ok()?;
                if !kind.editable()
                    || (!self.rest.dirty_buffers.contains(id) && document.revision().0 == 0)
                {
                    return None;
                }
                Some(buffers::Override {
                    path: self.rest.path_for_key(key)?,
                    kind: *kind,
                    snapshot: document.snapshot(),
                })
            })
            .collect()
    }
    pub(super) fn open_rest_buffer(&mut self, kind: BufferKind) -> Result<()> {
        let Some(form) = &self.rest.form else {
            return Ok(());
        };
        if form.closed {
            return Ok(());
        }
        let key = form.key.clone();
        let path = form.path.clone();
        self.pending_open = None;
        self.rest.pending_open = None;
        self.rest.ready_open = None;
        self.rest.open_focus = Some((self.focus.focused, true));
        if kind.editable()
            && kind != BufferKind::RawBody
            && let Some(id) = self.rest.buffers.get(&(key.clone(), kind))
        {
            self.rest.ready_open = Some(*id);
            return Ok(());
        }
        let capture = self.rest_model_capture();
        let input = Box::new(buffers::Input {
            runtime_tokens: self.rest.oauth.tokens.get(&key).cloned(),
            collection: self.rest.model.export_collection(),
            path: path.clone(),
            kind,
            key,
            root: self.project.root.clone(),
            environment: self.rest.model.active_environment().cloned(),
            environment_text: capture.environment_text,
            environment_draft: capture.environment_draft,
            overrides: self.request_buffer_captures(),
            draft: self
                .rest
                .request_drafts
                .get(&(path.clone(), kind.name().into()))
                .cloned(),
            drafts: self.rest.request_drafts.clone(),
        });
        let tag = self.tag()?;
        self.rest.pending_open = Some(tag);
        self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
            jobs::Job::Buffer {
                tag,
                structure: self.rest.run.structure,
                input,
            },
        )))));
        Ok(())
    }
    pub(super) fn refresh_rest_preview(&mut self) -> Result<()> {
        self.open_rest_buffer(BufferKind::RawRequest)?;
        if let Some((_, transfer)) = &mut self.rest.open_focus {
            *transfer = false;
        }
        Ok(())
    }
    pub(super) fn rest_buffer_changed(&mut self, snapshot: TextSnapshot) -> Result<bool> {
        let Some(Entry {
            kind: DocumentKind::Buffer { key, kind },
            ..
        }) = self.rest.documents.get(&snapshot.id)
        else {
            return Ok(false);
        };
        let (key, kind) = (key.clone(), *kind);
        if !kind.editable() {
            return Ok(false);
        }
        let Some(path) = self.rest.path_for_key(&key) else {
            return Ok(false);
        };
        let mut tag = self.tag()?;
        tag.document = Some((snapshot.id, snapshot.revision));
        self.rest.dirty_buffers.insert(snapshot.id);
        self.rest.pending_changes.insert(snapshot.id, tag);
        self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
            jobs::Job::Change {
                tag,
                key,
                kind,
                path,
                collection: self.rest.model.export_collection(),
                snapshot,
            },
        )))));
        self.changed()?;
        Ok(true)
    }
    pub(super) fn complete_rest_editor(
        &mut self,
        tag: JobTag,
        context: jobs::Context,
        result: Result<jobs::Reply>,
    ) -> Result<()> {
        if self.closing {
            return Ok(());
        }
        match context {
            jobs::Context::Format { key, document } => {
                if self
                    .rest
                    .form
                    .as_ref()
                    .is_none_or(|form| form.key != key || form.closed)
                {
                    return Ok(());
                }
                if !tag.document.is_some_and(|(id, revision)| {
                    id == document
                        && self
                            .documents
                            .document(id)
                            .is_ok_and(|document| document.revision() == revision)
                }) {
                    return Ok(());
                }
                match result {
                    Ok(jobs::Reply::Format(Some(text))) => {
                        let area = self.rest_editor_area();
                        if let Some(view) = self
                            .editor
                            .as_mut()
                            .filter(|view| view.document_id() == document)
                        {
                            let end = self.documents.document(document)?.snapshot().len_bytes();
                            view.programmatic_replace(
                                kode_core::TextRange {
                                    start: kode_core::ByteOffset(0),
                                    end: kode_core::ByteOffset(end),
                                },
                                &text,
                                &mut self.documents,
                                area,
                            )?;
                        }
                    }
                    Ok(jobs::Reply::Format(None)) => {
                        if let Some(form) = &mut self.rest.form {
                            form.message =
                                "Body unchanged: already formatted, invalid or unsupported content"
                                    .into();
                        }
                    }
                    Err(error) => self.notice = error.to_string(),
                    _ => {}
                }
            }
            jobs::Context::Response(proof) => {
                if self.rest.response.pending != Some(tag)
                    || self.rest.response.requested.as_ref() != Some(&proof)
                    || self
                        .rest
                        .form
                        .as_ref()
                        .is_none_or(|form| form.closed || form.key != proof.key)
                {
                    return Ok(());
                }
                self.rest.response.pending = None;
                match result {
                    Ok(jobs::Reply::Response(prepared)) => {
                        self.rest.response.ready = Some((proof, *prepared))
                    }
                    Err(error) => self.notice = error.to_string(),
                    _ => {}
                }
            }
            jobs::Context::Form {
                key,
                path,
                collection,
            } => {
                if self.rest.pending_open != Some(tag) {
                    return Ok(());
                }
                self.rest.pending_open = None;
                let Some(live) = self.rest.path_for_key(&key) else {
                    return Ok(());
                };
                if path != live || !Arc::ptr_eq(&collection, &self.rest.model.export_collection()) {
                    return self.begin_rest_form(live);
                }
                match result {
                    Ok(jobs::Reply::Form(prepared)) => {
                        let (prepared, derived) = *prepared;
                        let mut form = match self.rest.forms.remove(&key) {
                            Some(mut form) => {
                                form.path = live.clone();
                                form.refresh_fields(prepared);
                                form
                            }
                            None => Form::new(live.clone(), key.clone(), prepared)?,
                        };
                        form.set_derived(derived);
                        if let Some(error) = self.rest.run.failures.get(&key) {
                            form.message = format!("Previous send: {error}");
                        } else if self
                            .rest
                            .run
                            .outcomes
                            .iter()
                            .any(|outcome| outcome.item.id == key)
                            && !self
                                .rest
                                .run
                                .responses
                                .iter()
                                .any(|response| response.key.item_id == key)
                        {
                            form.message = format!(
                                "Response preview no longer retained ({} results/{} MiB history)",
                                crate::panels::rest::run::RESPONSE_HISTORY_COUNT,
                                crate::panels::rest::run::RESPONSE_HISTORY_BYTES / (1024 * 1024)
                            );
                        }
                        let descriptor = HostDescriptor::new(
                            HostRole::RawRequest,
                            "rest-node".into(),
                            key.clone(),
                            "REST request".into(),
                            self.project.root.clone(),
                        );
                        let document = Document::new(
                            DocumentId::fresh()?,
                            DocumentOrigin::Virtual {
                                owner: descriptor.owner.clone(),
                                key: key.clone(),
                                label: descriptor.label.clone(),
                            },
                            None,
                            "",
                        );
                        let id =
                            self.documents
                                .insert(document, &descriptor, self.generation, None)?;
                        self.rest.documents.insert(
                            id,
                            Entry {
                                kind: DocumentKind::Node(live),
                                descriptor,
                            },
                        );
                        self.rest.markers.insert(id, key.clone());
                        self.rest.forms.insert(key, form);
                        self.rest.ready_open = Some(id);
                    }
                    Err(error) => self.notice = error.to_string(),
                    _ => {}
                }
            }
            jobs::Context::Buffer {
                key,
                kind,
                structure,
            } => {
                if self.rest.pending_open != Some(tag) {
                    return Ok(());
                }
                self.rest.pending_open = None;
                if self.rest.form.as_ref().is_none_or(|form| {
                    form.closed || form.key != key || form.selected_buffer != Some(kind)
                }) {
                    return Ok(());
                }
                if structure != self.rest.run.structure {
                    return self.open_rest_buffer(kind);
                }
                match result {
                    Ok(jobs::Reply::Buffer(prepared)) => {
                        let id = if kind.editable()
                            && let Some(id) = self.rest.buffers.get(&(key.clone(), kind))
                        {
                            *id
                        } else {
                            self.documents.insert(
                                prepared.document,
                                &prepared.descriptor,
                                self.generation,
                                None,
                            )?
                        };
                        if self.rest.path_for_key(&key).is_some_and(|path| {
                            self.rest
                                .request_drafts
                                .contains_key(&(path, kind.name().into()))
                        }) {
                            self.rest.dirty_buffers.insert(id);
                        }
                        self.rest.buffer_warnings.insert(id, prepared.warnings);
                        self.rest.documents.insert(
                            id,
                            Entry {
                                kind: DocumentKind::Buffer {
                                    key: key.clone(),
                                    kind,
                                },
                                descriptor: prepared.descriptor,
                            },
                        );
                        if kind.editable() {
                            self.rest.buffers.insert((key, kind), id);
                        }
                        self.rest.ready_open = Some(id);
                    }
                    Err(error) => {
                        if let Some(form) = &mut self.rest.form {
                            form.message = error.to_string();
                        }
                    }
                    _ => {}
                }
            }
            jobs::Context::Change {
                key,
                kind,
                document,
            } => {
                if self.rest.pending_changes.get(&document) != Some(&tag) {
                    return Ok(());
                }
                self.rest.pending_changes.remove(&document);
                if !tag.document.is_some_and(|(id, revision)| {
                    self.documents
                        .document(id)
                        .is_ok_and(|document| document.revision() == revision)
                }) {
                    return Ok(());
                }
                let Some(path) = self.rest.path_for_key(&key) else {
                    return Ok(());
                };
                let applied = match result {
                    Ok(jobs::Reply::Change(jobs::Change::Headers { value, warnings })) => {
                        self.rest.buffer_warnings.insert(document, warnings.clone());
                        if let Some(view) = self
                            .editor
                            .as_mut()
                            .filter(|view| view.document_id() == document)
                        {
                            view.host.warnings = warnings;
                        }
                        editing::replace_headers(&mut self.rest.model, &path, value)
                    }
                    Ok(jobs::Reply::Change(jobs::Change::Text(text))) => {
                        if let Some(field) = kind.field() {
                            editing::field(&mut self.rest.model, &path, field, &text)
                        } else {
                            Ok(false)
                        }
                    }
                    Err(error) => Err(error),
                    _ => Ok(false),
                };
                match applied {
                    Ok(true) => self.changed()?,
                    Err(error) => {
                        self.notice = error.to_string();
                        if let Some(form) = self.rest.form.as_mut().filter(|form| form.key == key) {
                            form.message = self.notice.clone();
                        }
                    }
                    _ => {}
                }
            }
            jobs::Context::Derived { key, revision } => {
                if self.rest.pending_derived != Some(tag) {
                    return Ok(());
                }
                self.rest.pending_derived = None;
                if revision != self.rest.model.revision() {
                    return Ok(());
                }
                if let Some(form) = self.rest.form.as_mut().filter(|form| form.key == key)
                    && let Ok(jobs::Reply::Derived(derived)) = result
                {
                    let (prepared, derived) = *derived;
                    form.refresh_fields(prepared);
                    form.set_derived(derived);
                    self.rest.derived_revision = revision;
                }
            }
        }
        self.drain_rest()
    }
    pub(super) fn drain_rest_form(&mut self) -> Result<()> {
        self.sync_rest_oauth();
        if let Some(form) = &mut self.rest.form {
            form.running = self.rest.run.pending.is_some() || self.rest.run.running.is_some();
        }
        let intents = self
            .rest
            .form
            .as_mut()
            .map(Form::take_intents)
            .unwrap_or_default();
        for intent in intents {
            match intent {
                Intent::Browse { request, proof } => {
                    self.start_path_picker(request, super::path_picker::Target::RestForm(proof))?
                }
                Intent::OAuth(op) => self.rest_oauth_action(op)?,
                Intent::OAuthRedirect { flow, text } => self.rest_oauth_redirect(flow, text),
                Intent::Send => {
                    if let Some(form) = &self.rest.form {
                        self.send_rest_request(form.path.clone())?;
                    }
                }
                Intent::Buffer(kind) => self.open_rest_buffer(kind)?,
                Intent::Changed => self.changed()?,
                Intent::FormatBody => self.format_rest_body()?,
                Intent::RunHistory => self.show_rest_runner()?,
                Intent::ClearCookies => {
                    self.rest.run.client.clear_cookies();
                    self.rest.response.requested = None;
                    self.rest.response.pending = None;
                    self.rest.response.ready = None;
                    if let Some(form) = &mut self.rest.form {
                        form.message = "Cookies cleared".into();
                    }
                }
                Intent::Notices => self.inspect_rest_notices()?,
                Intent::Clipboard { action, proof } => {
                    let job = match action {
                        FieldAction::Copy(text) => {
                            ClipboardJob::Publish(self.clipboard.remember(&text)?)
                        }
                        FieldAction::PasteRequested => ClipboardJob::Read,
                        _ => continue,
                    };
                    let tag = self.tag()?;
                    self.work
                        .push(AppWork::RestFormClipboard { tag, job, proof });
                }
            }
        }
        if let Some(form) = &self.rest.form
            && !form.closed
            && self.rest.pending_derived.is_none()
            && self.rest.derived_revision != self.rest.model.revision()
        {
            let key = form.key.clone();
            let path = form.path.clone();
            let tag = self.tag()?;
            self.rest.pending_derived = Some(tag);
            self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
                jobs::Job::Derived {
                    tag,
                    key,
                    path,
                    revision: self.rest.model.revision(),
                    capture: Box::new(self.rest_model_capture()),
                },
            )))));
        }
        self.drain_rest_response()?;
        Ok(())
    }
    fn format_rest_body(&mut self) -> Result<()> {
        let Some(view) = &self.editor else {
            return Ok(());
        };
        let Some(Entry {
            kind: DocumentKind::Buffer { key, kind },
            ..
        }) = self.rest.documents.get(&view.document_id())
        else {
            return Ok(());
        };
        if !matches!(kind, BufferKind::RawBody | BufferKind::GraphqlVariables) {
            return Ok(());
        }
        let key = key.clone();
        let snapshot = self.documents.document(view.document_id())?.snapshot();
        let content_type = if *kind == BufferKind::GraphqlVariables {
            Some("application/json".into())
        } else {
            view.host.descriptor.mime.clone()
        };
        let mut tag = self.tag()?;
        tag.document = Some((snapshot.id, snapshot.revision));
        self.work.push(AppWork::Rest(Box::new(Work::Editor(Box::new(
            jobs::Job::Format {
                tag,
                key,
                snapshot,
                content_type,
            },
        )))));
        Ok(())
    }
    fn send_rest_request(&mut self, path: NodePath) -> Result<()> {
        if self
            .rest
            .model
            .node(&path)
            .is_some_and(|node| editing::is_request(node, &path))
        {
            self.run_rest_selection(path)?;
        } else if let Some(form) = &mut self.rest.form {
            form.message = "Select a request to send; use the sidebar to run a collection".into();
        }
        Ok(())
    }
    pub(super) fn execute_rest_snapshot(&mut self, execution: HostExecution) -> Result<()> {
        let Some(Entry {
            kind: DocumentKind::Buffer { key, .. },
            ..
        }) = self.rest.documents.get(&execution.snapshot.id)
        else {
            return Ok(());
        };
        let Some(path) = self.rest.path_for_key(key) else {
            return Ok(());
        };
        self.send_rest_request(path)
    }
    pub(super) fn rest_editor_area(&self) -> Rect {
        self.rest
            .form
            .as_ref()
            .map_or(self.geometry.viewer, |form| form.geometry.editor)
    }
    fn rest_buffer_visible(&self) -> bool {
        self.rest.form.as_ref().is_some_and(|form| self.editor.as_ref().and_then(|view|self.rest.documents.get(&view.document_id())).is_some_and(|entry|matches!(&entry.kind,DocumentKind::Buffer {key,kind} if *key==form.key && Some(*kind)==form.selected_buffer)))
    }
    pub(super) fn dispatch_rest_editor(&mut self, event: &InputEvent) -> Result<bool> {
        if self.dispatch_rest_runner(event)? {
            return Ok(true);
        }
        if self.dispatch_rest_response(event)? {
            return Ok(true);
        }
        let Some(form) = &self.rest.form else {
            return Ok(false);
        };
        if form.has_modal() {
            return self.rest.form.as_mut().expect("active form").dispatch(
                event,
                &mut self.rest.model,
                &mut self.focus,
            );
        }
        let editor_area = form.geometry.editor;
        if let InputEvent::Key(input) = event
            && input.modifiers.ctrl
            && input.key == Key::Tab
            && crate::input::shortcuts::command_key(crate::input::shortcuts::Scope::Request, *input)
                .is_some()
        {
            let tabs = form.tabs();
            let index = tabs.iter().position(|tab| *tab == form.tab).unwrap_or(0);
            let next = if input.modifiers.shift {
                (index + tabs.len() - 1) % tabs.len()
            } else {
                (index + 1) % tabs.len()
            };
            if self.focus.focused == Some(form.id)
                || self
                    .editor
                    .as_ref()
                    .is_some_and(|view| self.focus.focused == Some(view.input.id))
            {
                self.focus.focused = Some(form.id);
                self.rest.form.as_mut().expect("active form").perform(
                    crate::panels::rest::editor::form::Action::Tab(tabs[next]),
                    &mut self.rest.model,
                    &mut self.focus,
                )?;
                return Ok(true);
            }
        }
        let pointer = match event {
            InputEvent::Pointer(pointer) => Some(pointer),
            _ => None,
        };
        if self.rest_buffer_visible()
            && let Some(view) = &mut self.editor
        {
            if let Some(pointer) = pointer
                && editor_area.contains(Position::new(pointer.x, pointer.y))
                && pointer.action == PointerAction::Down(MouseButton::Left)
            {
                self.focus.focused = Some(view.input.id);
            }
            if (view.input.is_dragging(&self.focus)
                || pointer.is_some_and(|pointer| {
                    editor_area.contains(Position::new(pointer.x, pointer.y))
                })
                || self.focus.focused == Some(view.input.id))
                && view.dispatch(event, &mut self.documents, &mut self.focus, editor_area)?
            {
                return Ok(true);
            }
        }
        let form = self.rest.form.as_mut().expect("active form");
        if let Some(pointer) = pointer
            && form
                .geometry
                .area
                .contains(Position::new(pointer.x, pointer.y))
            && pointer.action == PointerAction::Down(MouseButton::Left)
        {
            self.focus.focused = Some(form.id);
        }
        form.dispatch(event, &mut self.rest.model, &mut self.focus)
    }
    pub(super) fn render_rest_editor(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
    ) -> Result<Option<Position>> {
        let request_area = self.rest_response_request_area(area);
        let visible = self.rest_buffer_visible();
        let Some(form) = &mut self.rest.form else {
            return Ok(None);
        };
        let mut cursor = form.render(
            request_area,
            buffer,
            theme,
            &self.rest.model,
            self.focus.focused == Some(form.id),
        )?;
        if visible && let Some(view) = &mut self.editor {
            view.synchronize(&self.documents)?;
            if let Some(position) = view.render(
                &self.documents,
                &self.focus,
                form.geometry.editor,
                buffer,
                theme,
                &[],
            )? {
                cursor = Some(position);
            }
        }
        if let Some(position) = self.render_rest_response(area, buffer, theme)? {
            cursor = Some(position);
        }
        self.render_rest_runner(area, buffer, theme);
        Ok(cursor)
    }
}
