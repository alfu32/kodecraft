use super::*;
use crate::input::terminal::{Key, PointerAction};
use crossterm::event::MouseButton;
impl Form {
    fn buffer_for_tab(&self, model: &Workspace) -> Option<BufferKind> {
        match self.tab {
            Tab::Headers => Some(BufferKind::Headers),
            Tab::Raw => Some(BufferKind::RawRequest),
            Tab::Body => match model
                .node(&self.path)?
                .pointer("/request/body/mode")?
                .as_str()?
            {
                "raw" => Some(BufferKind::RawBody),
                "graphql" => Some(
                    if self.selected_buffer == Some(BufferKind::GraphqlVariables) {
                        BufferKind::GraphqlVariables
                    } else {
                        BufferKind::GraphqlQuery
                    },
                ),
                _ => None,
            },
            _ => None,
        }
    }
    pub(super) fn activate(
        &mut self,
        action: Action,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        if model.node(&self.path).is_none() {
            return Err(invalid("REST item no longer exists"));
        }
        match action {
            Action::Focus(field) => {
                if !self.fields.contains_key(&field) {
                    return Err(invalid("This field uses an embedded editor"));
                }
                self.focused = Some(field);
                focus.focused = Some(self.id);
            }
            Action::Tab(tab) => {
                if !self.tabs().contains(&tab) {
                    return Ok(());
                }
                self.tab = tab;
                self.focused = None;
                self.scroll = 0;
                self.selection = 0;
                self.follow = true;
                self.epoch = self.epoch.saturating_add(1);
                self.selected_buffer = self.buffer_for_tab(model);
                if let Some(kind) = self.selected_buffer {
                    self.intents.push(Intent::Buffer(kind));
                }
            }
            Action::Send => self.intents.push(Intent::Send),
            Action::FormatBody => self.intents.push(Intent::FormatBody),
            Action::Cookies => self.intents.push(Intent::ClearCookies),
            Action::Notices => self.intents.push(Intent::Notices),
            Action::Buffer(kind) => {
                let tab = match kind {
                    BufferKind::Headers => Tab::Headers,
                    BufferKind::RawRequest => Tab::Raw,
                    _ => Tab::Body,
                };
                if !self.tabs().contains(&tab) {
                    return Err(invalid("Selected node has no request editor"));
                }
                self.tab = tab;
                self.selected_buffer = Some(kind);
                self.focused = None;
                self.intents.push(Intent::Buffer(kind));
            }
            Action::Reveal => self.reveal = !self.reveal,
            Action::Auth(kind) => {
                editing::auth(model, &self.path, kind)?;
                self.intents.push(Intent::Changed);
            }
            Action::Body(mode) => {
                editing::body_mode(model, &self.path, mode)?;
                self.intents.push(Intent::Changed);
                self.selected_buffer = self.buffer_for_tab(model);
                if let Some(kind) = self.selected_buffer {
                    self.intents.push(Intent::Buffer(kind));
                }
            }
            Action::Setting(key) => {
                editing::setting(model, &self.path, key)?;
                self.intents.push(Intent::Changed);
            }
            Action::Acknowledge => {
                if model
                    .node(&self.path)
                    .and_then(|node| node.get("x-kode-import-incomplete"))
                    == Some(&Value::Bool(true))
                {
                    editing::acknowledge_import(model, &self.path)?;
                    self.intents.push(Intent::Changed);
                }
            }
            Action::Add(kind) => {
                let index = editing::add_row(model, &self.path, kind)?;
                self.intents.push(Intent::Changed);
                self.activate(Action::Edit(kind, index), model, focus)?;
            }
            Action::Toggle(kind, index) => {
                editing::toggle_row(model, &self.path, kind, index)?;
                self.intents.push(Intent::Changed);
            }
            Action::Remove(kind, index) => {
                editing::remove_row(model, &self.path, kind, index)?;
                self.intents.push(Intent::Changed);
            }
            Action::Edit(kind, index) => {
                let entries = editing::rows(
                    model.node(&self.path).expect("checked node"),
                    &self.path,
                    kind,
                )?;
                let entry = entries
                    .get(index)
                    .filter(|entry| entry.is_object())
                    .ok_or_else(|| invalid("Selected row is no longer editable"))?;
                let value = editing::entry_value(entry, kind);
                let secret = entry.get("secret") == Some(&Value::Bool(true))
                    || value.kind == "secret"
                    || kind == Rows::Auth
                        && matches!(value.key.as_str(), "value" | "password" | "token");
                let mut dialog = EntryDialog::new(
                    match kind {
                        Rows::Variable => "Variable",
                        Rows::Query => "Query parameter",
                        Rows::Path => "Path variable",
                        Rows::FormData => "Form field",
                        Rows::UrlEncoded => "Form parameter",
                        Rows::Auth => "Authentication value",
                    }
                    .into(),
                    value,
                    kind == Rows::Variable,
                    if kind == Rows::FormData {
                        "Type text/file"
                    } else {
                        "Description"
                    },
                )?;
                if kind == Rows::Auth {
                    dialog.fields[0].read_only = true;
                    dialog.focus = 1;
                }
                dialog.fields[1].masked = secret && !self.reveal;
                let previous = focus.focused;
                focus.focused = Some(self.id);
                self.epoch = self.epoch.saturating_add(1);
                self.modal = Some(Modal {
                    dialog,
                    kind,
                    index,
                    expected: model.export_collection(),
                    previous,
                });
            }
        }
        Ok(())
    }
    fn field_action(&mut self, action: FieldAction, proof: Option<PasteProof>) {
        match action {
            FieldAction::Copy(_) | FieldAction::PasteRequested => {
                self.intents.push(Intent::Clipboard { action, proof })
            }
            FieldAction::LimitReached => self.message = "Field input limit reached".into(),
            FieldAction::Changed => {}
        }
    }
    fn field_proof(&self, field: Field) -> Option<PasteProof> {
        let value = self.fields.get(&field)?;
        Some(PasteProof {
            key: self.key.clone(),
            epoch: self.epoch,
            target: PasteTarget::Field {
                field,
                value: value.value.clone(),
                cursor: value.cursor,
                anchor: value.anchor,
            },
        })
    }
    pub fn paste(
        &mut self,
        proof: PasteProof,
        text: String,
        model: &mut Workspace,
        focus: &FocusState,
    ) -> Result<bool> {
        if proof.key != self.key
            || proof.epoch != self.epoch
            || focus.focused != Some(self.id)
            || self.closed
        {
            return Ok(false);
        }
        match proof.target {
            PasteTarget::Entry(proof) => {
                let Some(modal) = &mut self.modal else {
                    return Ok(false);
                };
                modal.dialog.paste(proof, text);
            }
            PasteTarget::Field {
                field,
                value,
                cursor,
                anchor,
            } => {
                if self.modal.is_some() || self.focused != Some(field) {
                    return Ok(false);
                }
                let Some(target) = self.fields.get_mut(&field).filter(|target| {
                    target.value == value && target.cursor == cursor && target.anchor == anchor
                }) else {
                    return Ok(false);
                };
                let before = target.value.clone();
                let action = target.handle(&InputEvent::Paste(text));
                if target.value != before {
                    editing::field(model, &self.path, field, &target.value)?;
                    self.intents.push(Intent::Changed);
                }
                if let Some(action) = action {
                    self.field_action(action, None);
                }
            }
        }
        Ok(true)
    }
    pub(super) fn dispatch_input(
        &mut self,
        event: &InputEvent,
        model: &mut Workspace,
        focus: &mut FocusState,
    ) -> Result<bool> {
        if matches!(event, InputEvent::Focus(false)) {
            self.epoch = self.epoch.saturating_add(1);
            return Ok(false);
        }
        if self.closed {
            return Ok(false);
        }
        if let Some(modal) = &mut self.modal {
            modal.dialog.dispatch(event);
            let actions = modal.dialog.take_actions();
            let proof = modal.dialog.proof().map(|proof| PasteProof {
                key: self.key.clone(),
                epoch: self.epoch,
                target: PasteTarget::Entry(proof),
            });
            let finished = modal.dialog.finished;
            for action in actions {
                self.field_action(action, proof.clone());
            }
            if finished {
                let modal = self.modal.take().expect("active dialog");
                focus.focused = modal.previous;
                self.epoch = self.epoch.saturating_add(1);
                if let Some(value) = modal.dialog.result {
                    if !Arc::ptr_eq(&modal.expected, &model.export_collection()) {
                        self.message =
                            "REST data changed while the dialog was open; reopen this row".into();
                    } else {
                        editing::edit_row(model, &self.path, modal.kind, modal.index, &value)?;
                        self.intents.push(Intent::Changed);
                    }
                }
            }
            return Ok(true);
        }
        if let InputEvent::Pointer(pointer) = event {
            let position = Position::new(pointer.x, pointer.y);
            if !self.geometry.area.contains(position) {
                return Ok(false);
            }
            match pointer.action {
                PointerAction::Down(MouseButton::Left) => {
                    if let Some(hit) = self
                        .hits
                        .iter()
                        .find(|hit| hit.area.contains(position))
                        .cloned()
                    {
                        self.selection = self
                            .controls
                            .iter()
                            .position(|(_, action)| action == &hit.action)
                            .unwrap_or(self.selection);
                        self.follow = true;
                        self.activate(hit.action.clone(), model, focus)?;
                        if let Action::Focus(field) = hit.action
                            && let Some(value) = self.fields.get_mut(&field)
                        {
                            value.pointer(event, hit.area);
                        }
                    }
                    return Ok(true);
                }
                PointerAction::Scroll {
                    vertical: delta, ..
                } if self.geometry.content.contains(position) => {
                    self.scroll = self.scroll.saturating_add_signed(delta as isize * 3);
                    self.follow = false;
                    return Ok(true);
                }
                _ => {}
            }
            return Ok(false);
        }
        if focus.focused != Some(self.id) {
            return Ok(false);
        }
        if let InputEvent::Key(input) = event {
            if input.key == Key::Enter
                && input.modifiers.ctrl
                && !input.modifiers.alt
                && !input.modifiers.meta
            {
                self.intents.push(Intent::Send);
                return Ok(true);
            }
            if !input.modifiers.ctrl && !input.modifiers.alt && !input.modifiers.meta {
                match input.key {
                    Key::Tab => {
                        let tabs = self.tabs();
                        let index = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0);
                        let index = if input.modifiers.shift {
                            (index + tabs.len() - 1) % tabs.len()
                        } else {
                            (index + 1) % tabs.len()
                        };
                        self.activate(Action::Tab(tabs[index]), model, focus)?;
                        return Ok(true);
                    }
                    Key::Escape => {
                        self.focused = None;
                        self.epoch = self.epoch.saturating_add(1);
                        return Ok(true);
                    }
                    Key::Up | Key::Down | Key::PageUp | Key::PageDown => {
                        let step = if matches!(input.key, Key::PageUp | Key::PageDown) {
                            10
                        } else {
                            1
                        };
                        self.selection = if matches!(input.key, Key::Up | Key::PageUp) {
                            self.selection.saturating_sub(step)
                        } else {
                            self.selection
                                .saturating_add(step)
                                .min(self.controls.len().saturating_sub(1))
                        };
                        self.focused = None;
                        self.follow = true;
                        return Ok(true);
                    }
                    Key::Enter if self.focused.is_none() => {
                        if let Some((_, action)) = self.controls.get(self.selection).cloned() {
                            self.activate(action, model, focus)?;
                        }
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
        if let Some(field) = self.focused {
            let proof = self.field_proof(field);
            let target = self.fields.get_mut(&field).expect("prepared field");
            let before = target.value.clone();
            let action = target.handle(event);
            if target.value != before {
                editing::field(model, &self.path, field, &target.value)?;
                self.intents.push(Intent::Changed);
            }
            if let Some(action) = action {
                self.field_action(action, proof);
            }
            return Ok(true);
        }
        Ok(false)
    }
}
