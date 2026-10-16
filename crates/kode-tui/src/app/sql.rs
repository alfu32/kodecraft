use super::state::{AppState, AppWork};
use crate::editor::host::{EditorView, HostDescriptor, HostRole, QueuedEditorHost, SaveTarget};
use kode_core::{DocumentId, DocumentOrigin, Result};

pub(super) fn descriptor(root: &std::path::Path, id: String, title: String) -> HostDescriptor {
    let mut descriptor = HostDescriptor::new(
        HostRole::Sql,
        "database".into(),
        id.clone(),
        title,
        root.into(),
    );
    descriptor.language = Some("sql".into());
    descriptor.mime = Some("application/sql".into());
    descriptor.grammar = "sql".into();
    descriptor.save_target = SaveTarget::Owner(id);
    descriptor
}
impl AppState {
    fn show_sql_diagnostic(&mut self, id: &str) -> Result<()> {
        let console = self.sql.manager.get(id).expect("active SQL console");
        let mut text = format!(
            "Console: {}\nDatasource: {}\n",
            id, console.state.data_source_id
        );
        if let Some(error) = self.sql.diagnostics.get(&console.state.data_source_id) {
            text.push_str(error);
            text.push_str("\n\n");
        }
        if let Some(panel) = self.sql.results.get(id) {
            text.push_str(&panel.details());
        }
        self.show_diagnostic("SQL diagnostics".into(), text)
    }

    pub(super) fn cancel_sql(&mut self, id: &str) -> Result<()> {
        let tag = self.tag()?;
        let Some(running) = self.sql.running.get_mut(id) else {
            return Ok(());
        };
        if running.cancel.is_cancelled() {
            if running.cancel_retry.is_none()
                && let Some(execution) = running.progress.borrow().execution_id.clone()
            {
                running.cancel_retry = Some(tag);
                self.work.push(AppWork::SqlCancel {
                    tag,
                    console: id.to_string(),
                    request: running.tag.request,
                    execution,
                });
            }
        } else {
            running.cancel.cancel();
        }
        self.notice = "Cancellation requested; waiting for the database outcome".into();
        Ok(())
    }
    pub(super) fn restore_sql(&mut self) -> Result<()> {
        let value = &self.persistence.session().database;
        if value.is_null() {
            return Ok(());
        }
        let restored = serde_json::from_value(value.clone())
            .map_err(|_| "Invalid saved database sessions; original state retained".to_string())
            .and_then(|state| {
                self.sql
                    .manager
                    .restore(state, None)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = restored {
            self.sql.persistence_enabled = false;
            self.notice = error;
            return Ok(());
        }
        let sessions: Vec<_> = self
            .sql
            .manager
            .all()
            .map(|console| console.state.clone())
            .collect();
        for session in sessions {
            let host = descriptor(
                &self.project.root,
                session.id.clone(),
                session.title.clone(),
            );
            let document =
                self.documents
                    .create_virtual(&host, self.generation, &session.buffer)?;
            self.sql_sources
                .insert(document, session.data_source_id.clone());
            self.sql.documents.insert(document, session.id.clone());
        }
        self.sync_database_sessions();
        if self.pending_open.is_none()
            && let Some(DocumentOrigin::Virtual { owner, key, .. }) =
                &self.persistence.session().active_origin
            && owner == "database"
            && let Some(id) = self.sql_document(key)
        {
            self.open_database_session(id)?;
        }
        Ok(())
    }
    pub fn sql_document(&self, console: &str) -> Option<DocumentId> {
        self.sql
            .documents
            .iter()
            .find_map(|(id, key)| (key == console).then_some(*id))
    }
    pub fn active_sql(&self) -> Option<&str> {
        if self.diff.is_some() {
            return None;
        }
        self.editor
            .as_ref()
            .and_then(|view| self.sql.documents.get(&view.document_id()))
            .map(String::as_str)
    }
    pub(super) fn request_sql_control(
        &mut self,
        action: crate::panels::database::console::Control,
    ) -> Result<()> {
        let Some(id) = self.active_sql() else {
            return Ok(());
        };
        let Some(console) = self.sql.manager.get(id) else {
            return Ok(());
        };
        let source = console.state.data_source_id.clone();
        if self.sql.controls.contains_key(&source) {
            self.notice = "A transaction operation is already pending for this datasource".into();
            return Ok(());
        }
        let tag = self.tag()?;
        self.sql.controls.insert(
            source.clone(),
            (tag, kode_platform::jobs::CancellationToken::new()),
        );
        self.work.push(AppWork::SqlControl {
            tag,
            source,
            action,
        });
        Ok(())
    }
    pub(super) fn start_sql_save(&mut self, job: crate::editor::host::OwnerSaveJob) -> Result<()> {
        use crate::panels::database::session_dialog::{Mode, SessionDialog, default_filename};
        let console = self
            .sql
            .documents
            .get(&job.snapshot.id)
            .cloned()
            .ok_or(kode_core::DomainError::NoSaveTarget)?;
        let state = &self
            .sql
            .manager
            .get(&console)
            .ok_or(kode_core::DomainError::NoSaveTarget)?
            .state;
        let initial = self.sql.saved.get(&console).map_or_else(
            || default_filename(&state.title),
            |(path, _)| path.to_string_lossy().into_owned(),
        );
        self.sql.dialog = Some(SessionDialog::new(
            console,
            Mode::Save,
            "Save SQL session — file name / path",
            initial,
            self.focus.focused,
            Some(job),
        )?);
        Ok(())
    }
    pub(super) fn dismiss_sql_dialog(&mut self) -> Result<()> {
        if self.sql.dialog.as_ref().is_some_and(|dialog| dialog.busy) {
            return Ok(());
        }
        if let Some(dialog) = self.sql.dialog.take() {
            self.focus.focused = dialog.previous_focus;
            if let Some(job) = dialog.save {
                self.documents.complete_owner_save(
                    job.snapshot.id,
                    job.tag,
                    Err(kode_core::DomainError::Cancelled),
                )?;
            }
        }
        self.sql.menu = None;
        Ok(())
    }
    pub(super) fn dispatch_sql_modal(
        &mut self,
        event: &crate::input::terminal::InputEvent,
    ) -> Result<bool> {
        use crate::panels::database::session_dialog::Mode;
        use crate::widgets::{dialog::DialogAction, field::FieldAction};
        if let Some(menu) = &mut self.sql.menu {
            menu.dispatch(event);
            if menu.finished {
                let action = menu.action;
                self.sql.menu = None;
                if let Some(action) = action
                    && let Some(view) = &mut self.editor
                {
                    view.execute(action, &self.documents)?;
                }
            }
            return Ok(true);
        }
        let Some(dialog) = &mut self.sql.dialog else {
            return Ok(false);
        };
        if dialog.busy {
            return Ok(true);
        }
        dialog.dialog.handle(event);
        let proof = dialog.proof();
        let action = dialog.dialog.action.take();
        match action {
            Some(DialogAction::Dismissed) => self.dismiss_sql_dialog()?,
            Some(DialogAction::Browse) => {
                if matches!(dialog.mode, Mode::Save) {
                    let request = crate::widgets::file_picker::Request::new(
                        dialog.dialog.title.clone(),
                        crate::widgets::file_picker::Mode::SaveFile,
                        dialog.dialog.field.value.clone(),
                    )?;
                    let target = super::path_picker::Target::Sql {
                        proof,
                        focus: dialog.dialog.focus,
                    };
                    self.start_path_picker(request, target)?;
                }
            }
            Some(DialogAction::Field(action)) => {
                let job = match action {
                    FieldAction::Copy(text) => Some(crate::editor::host::ClipboardJob::Publish(
                        self.clipboard.remember(&text)?,
                    )),
                    FieldAction::PasteRequested => Some(crate::editor::host::ClipboardJob::Read),
                    _ => None,
                };
                if let Some(job) = job {
                    self.work.push(AppWork::SqlClipboard {
                        tag: self.tag()?,
                        job,
                        proof: Some(proof),
                    });
                }
            }
            Some(DialogAction::Submitted(value)) => {
                let dialog = self.sql.dialog.as_mut().expect("SQL dialog");
                match dialog.mode {
                    Mode::Save => {
                        if value.trim().is_empty() {
                            dialog.dialog.finished = false;
                            self.notice = "Enter a SQL file name or path".into();
                            return Ok(true);
                        }
                        let path = std::path::PathBuf::from(value);
                        let path = if path.is_absolute() {
                            path
                        } else {
                            self.project.root.join(path)
                        };
                        let expected = self
                            .sql
                            .saved
                            .get(&dialog.console)
                            .filter(|(previous, _)| previous == &path)
                            .map(|(_, identity)| identity.clone());
                        dialog.busy = true;
                        self.work.push(AppWork::SqlSave {
                            job: dialog.save.as_ref().expect("save job").clone(),
                            path,
                            expected,
                        });
                    }
                    Mode::Catalog | Mode::Schema => {
                        let console = self.sql.manager.get(&dialog.console).expect("session");
                        let mut catalog = console.state.catalog.clone();
                        let mut schema = console.state.schema.clone();
                        let value = (!value.trim().is_empty()).then(|| value.trim().to_string());
                        if matches!(dialog.mode, Mode::Catalog) {
                            catalog = value
                        } else {
                            schema = value
                        }
                        self.sql
                            .manager
                            .set_context(&dialog.console, catalog, schema);
                        self.dismiss_sql_dialog()?;
                        self.changed()?;
                    }
                }
            }
            None => {}
        }
        Ok(true)
    }
    pub(super) fn dispatch_sql(
        &mut self,
        event: &crate::input::terminal::InputEvent,
    ) -> Result<bool> {
        use crate::input::terminal::{InputEvent, Key, PointerAction};
        use crate::panels::database::{
            console::Control,
            session_dialog::{Mode, SessionDialog},
        };
        let Some(id) = self.active_sql().map(str::to_string) else {
            return Ok(false);
        };
        let hover = matches!(event,InputEvent::Pointer(pointer) if self.sql.toolbar[0].contains(ratatui::layout::Position::new(pointer.x,pointer.y)));
        if hover {
            if let Some(view) = &self.editor {
                let snapshot = self.documents.document(view.document_id())?.snapshot();
                let selection = view.input.state.caret.selection;
                let proof = (snapshot.id, snapshot.revision, selection);
                if self.sql.hover != Some(proof) && snapshot.len_bytes() <= 4 * 1024 * 1024 {
                    self.sql.hover = Some(proof);
                    let mut tag = self.tag()?;
                    tag.document = Some((snapshot.id, snapshot.revision));
                    let dialect = self
                        .sql
                        .manager
                        .get(&id)
                        .and_then(|console| {
                            self.database
                                .tree
                                .sources
                                .iter()
                                .find(|source| source.id == console.state.data_source_id)
                        })
                        .map_or(kode_database::statement::Dialect::Standard, |source| {
                            if matches!(source.driver.id.as_str(), "mysql" | "mariadb") {
                                kode_database::statement::Dialect::MySql
                            } else {
                                kode_database::statement::Dialect::Standard
                            }
                        });
                    self.work.push(AppWork::SqlHighlight {
                        tag,
                        execution: crate::panels::database::console::Highlight {
                            snapshot,
                            selection,
                            dialect,
                        },
                    });
                }
            }
        } else {
            self.sql.hover = None;
            if let Some(view) = &mut self.editor {
                view.host.transient = None;
            }
        }
        let open_details = matches!(event,InputEvent::Key(input) if input.key==Key::Function(4) && !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta);
        let sql_focused =
            self.editor.as_ref().is_some_and(|view| {
                self.focus.focused == Some(view.input.id) && !view.search.visible
            }) || self
                .sql
                .results
                .get(&id)
                .is_some_and(|panel| self.focus.focused == Some(panel.id));
        if open_details && sql_focused {
            self.show_sql_diagnostic(&id)?;
            return Ok(true);
        }
        if let Some(panel) = self.sql.results.get_mut(&id) {
            let handled = panel.dispatch(event, &mut self.focus);
            let details = panel.take_details_request();
            if details {
                self.show_sql_diagnostic(&id)?;
            }
            if handled {
                return Ok(true);
            }
        }
        let editor_focused = self
            .editor
            .as_ref()
            .is_some_and(|view| self.focus.focused == Some(view.input.id) && !view.search.visible);
        let mut action = None;
        match event {
            InputEvent::Key(input) if editor_focused && !input.modifiers.meta => {
                let Some(input) = crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Sql,
                    *input,
                ) else {
                    return Ok(false);
                };
                if input.modifiers.ctrl && input.modifiers.alt {
                    action = match input.key {
                        Key::Character('a' | 'A') => Some(3),
                        Key::Character('c' | 'C') => Some(2),
                        Key::Character('r' | 'R') => Some(4),
                        _ => None,
                    };
                }
                if input.key == Key::Escape && self.sql.running.contains_key(&id) {
                    self.cancel_sql(&id)?;
                    return Ok(true);
                }
            }
            InputEvent::Pointer(pointer) => {
                let point = ratatui::layout::Position::new(pointer.x, pointer.y);
                if pointer.action == PointerAction::Down(crossterm::event::MouseButton::Right)
                    && self.geometry.viewer.contains(point)
                {
                    self.sql.menu = Some(Default::default());
                    return Ok(true);
                }
                if pointer.action == PointerAction::Down(crossterm::event::MouseButton::Left) {
                    action = self
                        .sql
                        .toolbar
                        .iter()
                        .position(|rect| rect.contains(point));
                    if let Some(index) = self
                        .sql
                        .contexts
                        .iter()
                        .position(|rect| rect.contains(point))
                    {
                        let console = self.sql.manager.get(&id).expect("session");
                        let initial = if index == 0 {
                            &console.state.catalog
                        } else {
                            &console.state.schema
                        };
                        self.sql.dialog = Some(SessionDialog::new(
                            id,
                            if index == 0 {
                                Mode::Catalog
                            } else {
                                Mode::Schema
                            },
                            if index == 0 {
                                "SQL catalog (blank uses datasource default)"
                            } else {
                                "SQL schema (blank uses datasource default)"
                            },
                            initial.clone().unwrap_or_default(),
                            self.focus.focused,
                            None,
                        )?);
                        return Ok(true);
                    }
                }
            }
            _ => {}
        }
        match action {
            Some(0) => {
                if let Some(view) = &mut self.editor {
                    view.execute(
                        crate::editor::host::ExecutionAction::SqlSelectionOrStatement,
                        &self.documents,
                    )?;
                }
            }
            Some(1) => {
                if let Some(view) = &mut self.editor {
                    use crate::input::terminal::{KeyInput, Modifiers};
                    self.focus.focused = Some(view.input.id);
                    view.dispatch(
                        &InputEvent::Key(KeyInput {
                            key: Key::Character('s'),
                            modifiers: Modifiers {
                                ctrl: true,
                                ..Default::default()
                            },
                            repeat: false,
                        }),
                        &mut self.documents,
                        &mut self.focus,
                        self.geometry.viewer,
                    )?;
                }
            }
            Some(2) => self.request_sql_control(Control::Commit)?,
            Some(3) => self.request_sql_control(Control::AutoCommit)?,
            Some(4) => self.request_sql_control(Control::Rollback)?,
            Some(5) => {
                self.cancel_sql(&id)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub(super) fn render_sql(
        &mut self,
        buffer: &mut ratatui::buffer::Buffer,
        theme: &crate::theme::Theme,
    ) -> Result<()> {
        use crate::panels::database::results::clipped;
        use crate::widgets::controls::draw_text;
        use ratatui::layout::Rect;
        let Some(id) = self.active_sql().map(str::to_string) else {
            return Ok(());
        };
        let console = self.sql.manager.get(&id).expect("session");
        let status = self.sql.connections.get(&console.state.data_source_id);
        let auto = status
            .and_then(|status| status.session.as_ref())
            .map_or(console.state.auto_commit, |session| session.auto_commit);
        let labels = [
            " Execute ",
            " Save ",
            " Commit ",
            if auto { " Auto:on " } else { " Auto:off " },
            " Rollback ",
            " Cancel ",
        ];
        let area = self.geometry.sql_toolbar;
        let columns = (area.width / 11).clamp(1, 6);
        self.sql.toolbar = [Rect::default(); 6];
        for (index, label) in labels.iter().enumerate() {
            let row = index as u16 / columns;
            let column = index as u16 % columns;
            if row >= area.height {
                continue;
            }
            let x = area.x + column * (area.width / columns);
            let width = if column == columns - 1 {
                area.right() - x
            } else {
                area.width / columns
            };
            let rect = Rect::new(x, area.y + row, width, 1);
            self.sql.toolbar[index] = rect;
            draw_text(rect, buffer, label, theme.style("lsp-button"));
        }
        let area = self.geometry.sql_context;
        let widths = [
            area.width / 3,
            area.width / 3,
            area.width - area.width / 3 * 2,
        ];
        let mut x = area.x;
        for (index, label) in [
            format!(
                "Catalog: {}",
                console.state.catalog.as_deref().unwrap_or("default")
            ),
            format!(
                "Schema: {}",
                console.state.schema.as_deref().unwrap_or("default")
            ),
            format!(
                "{:?}{}",
                status.map_or(kode_database::connections::Phase::Disconnected, |status| {
                    status.phase
                }),
                if self
                    .sql
                    .controls
                    .contains_key(&console.state.data_source_id)
                {
                    " / pending"
                } else {
                    ""
                }
            ),
        ]
        .iter()
        .enumerate()
        {
            let rect = Rect::new(x, area.y, widths[index], area.height);
            if index < 2 {
                self.sql.contexts[index] = rect;
            }
            draw_text(
                rect,
                buffer,
                &clipped(label, rect.width as usize),
                theme.style("tab-strip"),
            );
            x = rect.right();
        }
        if let Some(panel) = self.sql.results.get_mut(&id) {
            panel.render(
                self.geometry.sql_results,
                buffer,
                theme,
                self.focus.focused == Some(panel.id),
            );
        }
        Ok(())
    }
    pub(super) fn attach_sql_document(&mut self, id: DocumentId) -> Result<()> {
        let key = self
            .sql
            .documents
            .get(&id)
            .cloned()
            .ok_or_else(|| kode_core::DomainError::MissingResource("SQL session".into()))?;
        let console = self
            .sql
            .manager
            .get(&key)
            .ok_or_else(|| kode_core::DomainError::MissingResource("SQL session".into()))?;
        let host = descriptor(&self.project.root, key, console.state.title.clone());
        let mut view = EditorView::new(
            id,
            &self.documents,
            QueuedEditorHost::new(host),
            self.generation,
            self.clipboard.clone(),
            self.clock.clone(),
        )?;
        if let Some(stored) = self.views.get(&id) {
            view.input.state.caret = stored.caret.clone();
            view.input.state.scroll_rows = stored.scroll_rows;
        } else {
            let snapshot = self.documents.document(id)?.snapshot();
            view.input.state.caret.move_to(
                &snapshot,
                kode_core::ByteOffset(snapshot.len_bytes()),
                false,
            )?;
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
        self.splash = false;
        let owner = view.owner();
        self.work.extend(
            view.take_work()
                .into_iter()
                .map(|work| AppWork::Editor { owner, work }),
        );
        self.editor = Some(view);
        Ok(())
    }
}
