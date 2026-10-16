use super::*;
use crate::input::terminal::Key;
use crate::widgets::dialog::DialogAction;
impl Sidebar {
    fn install_modal(&mut self, dialog: Dialog, model: &Workspace, focus: &mut FocusState) {
        self.clipboard_epoch = self.clipboard_epoch.saturating_add(1);
        let previous = self
            .modal
            .as_ref()
            .map_or(focus.focused, |modal| modal.previous);
        self.modal = Some(Modal {
            dialog,
            expected: model.export_collection(),
            previous,
        });
        focus.focused = Some(self.id);
        focus.captured = None;
    }
    fn input(
        &mut self,
        title: &str,
        initial: String,
        purpose: Purpose,
        model: &Workspace,
        focus: &mut FocusState,
    ) -> Result<()> {
        self.install_modal(
            Dialog::Input {
                nonce: RequestId::fresh()?,
                purpose,
                dialog: TextDialog::new(title.into(), initial),
            },
            model,
            focus,
        );
        Ok(())
    }
    fn confirm(&mut self, title: &str, action: Action, model: &Workspace, focus: &mut FocusState) {
        let mut dialog = TextDialog::new(title.into(), title.into());
        dialog.field.read_only = true;
        dialog.focus = 1;
        self.install_modal(Dialog::Confirm { action, dialog }, model, focus);
    }
    pub fn perform(
        &mut self,
        action: Action,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<()> {
        use Action::*;
        let old_cursor = self.cursor.clone();
        let old_environment = self.environment.clone();
        let is_folder = matches!(&action, AddFolder(_));
        let is_up = matches!(&action, MoveUp(_));
        match action {
            Open(path) => {
                if model.select(&path) {
                    self.cursor = path.clone();
                    self.expand_ancestors(&path);
                    self.intents.push(Intent::Open(path));
                }
            }
            Toggle(path) => {
                if !self.expanded.remove(&path) {
                    self.expanded.insert(path);
                }
                self.dirty = true;
            }
            Menu(path) => {
                let Some(node) = model.node(&path) else {
                    return Ok(());
                };
                let request = node
                    .get("request")
                    .is_some_and(|value| value.is_object() || value.is_string());
                let mut options = vec![];
                if !path.is_root() {
                    options.push(("Open".into(), Open(path.clone())));
                }
                if !request {
                    options.extend([
                        ("Add request".into(), AddRequest(path.clone())),
                        ("Add folder".into(), AddFolder(path.clone())),
                    ]);
                }
                options.extend([
                    ("Run".into(), Run(path.clone())),
                    ("Rename".into(), Rename(path.clone())),
                ]);
                if !request {
                    options.push(("Import OpenAPI".into(), ImportOpenApi(path.clone())));
                }
                if !path.is_root() {
                    options.extend([
                        ("Duplicate".into(), Copy(path.clone())),
                        ("Move up".into(), MoveUp(path.clone())),
                        ("Move down".into(), MoveDown(path.clone())),
                        ("Move to folder".into(), MoveTo(path.clone())),
                        ("Delete".into(), Delete(path.clone())),
                    ]);
                } else {
                    options.push(("Export collection".into(), Export));
                }
                options.push(("Import notices".into(), Notices));
                self.install_modal(
                    Dialog::Choice(ChoiceDialog::new(
                        "REST actions".into(),
                        options,
                        "Choose",
                        true,
                    )),
                    model,
                    focus,
                );
            }
            AddRequest(path) | AddFolder(path) => {
                let folder = is_folder;
                let parent = Self::parent_for(model, &path);
                let added = if folder {
                    model.add_folder(&parent, "New folder")
                } else {
                    model.add_request(&parent, "New request")
                };
                if let Some(path) = added {
                    self.inserted(&path);
                    model.select(&path);
                    self.cursor = path.clone();
                    self.expand_ancestors(&path);
                    self.intents.push(Intent::Open(path));
                    self.message = if folder {
                        "Added folder"
                    } else {
                        "Added request"
                    }
                    .into();
                }
            }
            Run(path) => {
                if model.node(&path).is_some() {
                    self.intents.push(Intent::Run(path));
                }
            }
            Rename(path) => {
                let Some(node) = model.node(&path) else {
                    return Ok(());
                };
                let name = if path.is_root() {
                    model.collection_name()
                } else {
                    node.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                self.input(
                    "Rename REST item",
                    name,
                    Purpose::Rename(path),
                    model,
                    focus,
                )?;
            }
            Copy(path) => {
                if let Some(copy) = model.duplicate(&path) {
                    self.inserted(&copy);
                    self.cursor = copy.clone();
                    model.select(&copy);
                    self.expand_ancestors(&copy);
                    self.intents.push(Intent::Open(copy));
                    self.message = "Duplicated item".into();
                }
            }
            MoveUp(path) | MoveDown(path) => {
                let result = if is_up {
                    model.move_up(&path)
                } else {
                    model.move_down(&path)
                };
                if let Some(moved) = result {
                    self.moved(&path, &moved);
                    model.select(&moved);
                    self.cursor = moved;
                    self.message = "Moved item".into();
                }
            }
            MoveTo(path) => {
                let options = model
                    .tree()
                    .into_iter()
                    .filter(|node| node.kind != NodeKind::Request && !node.path.starts_with(&path))
                    .map(|node| {
                        (
                            format!("{}{}", " ".repeat(node.depth.min(64) * 2), node.name),
                            MoveInto {
                                from: path.clone(),
                                to: node.path,
                            },
                        )
                    })
                    .collect();
                self.install_modal(
                    Dialog::Choice(ChoiceDialog::new(
                        "Move to folder".into(),
                        options,
                        "Move",
                        false,
                    )),
                    model,
                    focus,
                );
            }
            MoveInto { from, to } => {
                if let Some(moved) = model.move_node(&from, &to, None) {
                    self.moved(&from, &moved);
                    self.cursor = moved.clone();
                    model.select(&moved);
                    self.message = "Moved item".into();
                }
            }
            Delete(path) => {
                if path.is_root() {
                    return Ok(());
                }
                if model
                    .node(&path)
                    .and_then(|node| node.get("item"))
                    .and_then(Value::as_array)
                    .is_some_and(|items| !items.is_empty())
                {
                    self.confirm(
                        "Delete non-empty REST folder?",
                        DeleteConfirmed(path),
                        model,
                        focus,
                    );
                } else {
                    self.perform(DeleteConfirmed(path), model, focus)?;
                }
            }
            DeleteConfirmed(path) => {
                if model.delete(&path) {
                    self.removed(&path, model);
                    self.message = "Deleted item".into();
                }
            }
            ImportPostman => {
                if self.pending_file.is_some() {
                    return Ok(());
                }
                if model.collection()["item"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                {
                    self.confirm(
                        "Replace the existing REST collection?",
                        ImportPostmanConfirmed,
                        model,
                        focus,
                    );
                } else {
                    self.perform(ImportPostmanConfirmed, model, focus)?;
                }
            }
            ImportPostmanConfirmed => self.input(
                "Import Postman collection — file",
                "collection.postman_collection.json".into(),
                Purpose::Import(Kind::Postman, NodePath::default()),
                model,
                focus,
            )?,
            ImportOpenApi(path) => {
                if self.pending_file.is_none() {
                    let parent = Self::parent_for(model, &path);
                    self.input(
                        "Import OpenAPI — file",
                        "openapi.yaml".into(),
                        Purpose::Import(Kind::OpenApi, parent),
                        model,
                        focus,
                    )?;
                }
            }
            Export => {
                if self.pending_file.is_none() {
                    self.input(
                        "Export Postman collection — file",
                        "collection.postman_collection.json".into(),
                        Purpose::Export,
                        model,
                        focus,
                    )?;
                }
            }
            Notices => self.intents.push(Intent::InspectNotices),
            NewEnvironment => self.input(
                "New environment",
                "New environment".into(),
                Purpose::NewEnvironment,
                model,
                focus,
            )?,
            CopyEnvironment(id) => {
                if let Some(environment) = model.environment(&id) {
                    self.input(
                        "Copy environment",
                        format!("{} copy", environment.name),
                        Purpose::CopyEnvironment(id),
                        model,
                        focus,
                    )?;
                }
            }
            RenameEnvironment(id) => {
                if let Some(environment) = model.environment(&id) {
                    self.input(
                        "Rename environment",
                        environment.name.clone(),
                        Purpose::RenameEnvironment(id),
                        model,
                        focus,
                    )?;
                }
            }
            ActivateEnvironment(id) => {
                if model.activate_environment(Some(&id)) {
                    self.environment = Some(id);
                    self.message = "Activated environment".into();
                }
            }
            RemoveEnvironment(id) => {
                if model.remove_environment(&id) {
                    self.environment = model
                        .active_environment()
                        .map(|environment| environment.id.clone());
                    self.intents.push(Intent::EnvironmentRemoved(id));
                    self.message = "Removed environment".into();
                }
            }
            OpenEnvironment(id) => {
                if model.environment(&id).is_some() {
                    self.environment = Some(id.clone());
                    self.environment_focus = true;
                    self.intents.push(Intent::Environment(id));
                }
            }
            EnvironmentMenu(id) => {
                if model.environment(&id).is_some() {
                    let actions = vec![
                        ("Open".into(), OpenEnvironment(id.clone())),
                        ("New".into(), NewEnvironment),
                        ("Copy".into(), CopyEnvironment(id.clone())),
                        ("Rename".into(), RenameEnvironment(id.clone())),
                        ("Activate".into(), ActivateEnvironment(id.clone())),
                        ("Remove".into(), RemoveEnvironment(id)),
                    ];
                    self.install_modal(
                        Dialog::Choice(ChoiceDialog::new(
                            "Environment actions".into(),
                            actions,
                            "Choose",
                            true,
                        )),
                        model,
                        focus,
                    );
                }
            }
        }
        self.follow_cursor |= old_cursor != self.cursor;
        self.follow_environment |= old_environment != self.environment;
        self.dirty = true;
        Ok(())
    }
    pub(super) fn parent_for(model: &Workspace, path: &NodePath) -> NodePath {
        if model
            .node(path)
            .and_then(|node| node.get("request"))
            .is_some_and(|request| request.is_object() || request.is_string())
        {
            path.parent()
        } else {
            path.clone()
        }
    }
    fn field_action(&mut self, action: FieldAction, nonce: Option<RequestId>, field: &TextField) {
        match action {
            FieldAction::Copy(_) => self.intents.push(Intent::Clipboard {
                action,
                proof: None,
            }),
            FieldAction::PasteRequested => self.intents.push(Intent::Clipboard {
                action,
                proof: Some(PasteProof {
                    nonce,
                    epoch: self.clipboard_epoch,
                    field: field.value.clone(),
                    cursor: field.cursor,
                    anchor: field.anchor,
                }),
            }),
            FieldAction::LimitReached => self.message = "Field character limit reached".into(),
            FieldAction::Changed => {}
        }
    }
    pub(super) fn filter_action(&mut self, action: FieldAction) {
        let field = self.filter.clone();
        self.field_action(action, None, &field);
        self.dirty = true;
        self.scroll = 0;
    }
    pub(super) fn modal_input(
        &mut self,
        event: &InputEvent,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<bool> {
        let Some(mut modal) = self.modal.take() else {
            return Ok(false);
        };
        let mut next = None;
        let mut submitted = None;
        let finished = match &mut modal.dialog {
            Dialog::Input {
                nonce,
                purpose,
                dialog,
            } => {
                dialog.handle(event);
                match dialog.action.take() {
                    Some(DialogAction::Submitted(value)) => {
                        submitted = Some((purpose.clone(), value, *nonce))
                    }
                    Some(DialogAction::Field(action)) => {
                        self.field_action(action, Some(*nonce), &dialog.field)
                    }
                    _ => {}
                }
                dialog.finished
            }
            Dialog::Choice(dialog) => {
                dialog.dispatch(event);
                next = dialog.result.take();
                dialog.finished
            }
            Dialog::Confirm { action, dialog } => {
                if let InputEvent::Key(input) = event
                    && matches!(
                        input.key,
                        Key::Tab | Key::Left | Key::Right | Key::Up | Key::Down
                    )
                {
                    dialog.focus = if dialog.focus == 1 { 2 } else { 1 };
                } else {
                    let event = if matches!(event,InputEvent::Key(input)if input.key==Key::Delete&&dialog.focus==1)
                    {
                        match event {
                            InputEvent::Key(input) => {
                                InputEvent::Key(crate::input::terminal::KeyInput {
                                    key: Key::Enter,
                                    ..*input
                                })
                            }
                            _ => unreachable!(),
                        }
                    } else {
                        event.clone()
                    };
                    dialog.handle(&event);
                }
                if dialog.focus == 0 {
                    dialog.focus = 1;
                }
                if matches!(dialog.action.take(), Some(DialogAction::Submitted(_))) {
                    next = Some(action.clone());
                }
                dialog.finished
            }
        };
        if !finished {
            self.modal = Some(modal);
            return Ok(true);
        }
        focus.focused = modal.previous;
        let is_environment = matches!(
            &submitted,
            Some((
                Purpose::NewEnvironment
                    | Purpose::CopyEnvironment(_)
                    | Purpose::RenameEnvironment(_),
                _,
                _
            ))
        );
        if (submitted.is_some() || next.is_some())
            && !is_environment
            && !Arc::ptr_eq(&modal.expected, &model.export_collection())
        {
            self.message = "Collection changed; reopen this action".into();
            return Ok(true);
        }
        if let Some((purpose, value, nonce)) = submitted {
            if let Err(error) = self.submit(purpose, value, nonce, modal.expected.clone(), model) {
                self.message = error.to_string();
                if let Dialog::Input { dialog, .. } = &mut modal.dialog {
                    dialog.finished = false;
                }
                focus.focused = Some(self.id);
                self.modal = Some(modal);
            }
        } else if let Some(action) = next {
            self.perform(action, model, focus)?;
        }
        Ok(true)
    }
    fn submit(
        &mut self,
        purpose: Purpose,
        value: String,
        nonce: RequestId,
        expected: Arc<Value>,
        model: &mut Workspace,
    ) -> Result<()> {
        match purpose {
            Purpose::Rename(path) => {
                model.rename(&path, &value)?;
                self.message = "Renamed item".into();
            }
            Purpose::Import(kind, parent) => {
                if value.trim().is_empty() {
                    return Err(kode_core::DomainError::Unsupported(
                        "Choose an import filename".into(),
                    ));
                }
                self.pending_file = Some(nonce);
                self.message = "Reading import…".into();
                self.intents.push(Intent::Import {
                    nonce,
                    kind,
                    path: PathBuf::from(value),
                    parent,
                    expected,
                });
            }
            Purpose::Export => {
                if value.trim().is_empty() {
                    return Err(kode_core::DomainError::Unsupported(
                        "Choose an export filename".into(),
                    ));
                }
                self.pending_file = Some(nonce);
                self.message = "Exporting collection…".into();
                self.intents.push(Intent::Export {
                    nonce,
                    path: PathBuf::from(value),
                    collection: expected,
                });
            }
            Purpose::NewEnvironment => {
                let id = model.create_environment(&value);
                self.environment = Some(id.clone());
                self.intents.push(Intent::Environment(id));
                self.message = "Created environment".into();
            }
            Purpose::CopyEnvironment(id) => {
                if let Some(target) = model.duplicate_environment(&id, Some(&value)) {
                    self.environment = Some(target.clone());
                    self.intents
                        .push(Intent::CopiedEnvironment { source: id, target });
                    self.message = "Copied environment".into();
                }
            }
            Purpose::RenameEnvironment(id) => {
                if model.rename_environment(&id, &value) {
                    self.message = "Renamed environment".into();
                }
            }
        }
        self.dirty = true;
        Ok(())
    }
}
