use super::*;
use crate::input::terminal::{Key, PointerAction};
use crossterm::event::MouseButton;
impl Form {
    pub(super) fn open_authenticate_dialog(
        &mut self,
        view: &OAuthView,
        collection: Arc<Value>,
        focus: &mut FocusState,
    ) -> Result<()> {
        let Some(url) = &view.url else {
            return Ok(());
        };
        let mut dialog = EntryDialog::new(
            "Authenticate OAuth 2.0".into(),
            editing::EntryValue {
                key: "Access token".into(),
                value: String::new(),
                description: url.clone(),
                kind: "secret".into(),
                enabled: true,
            },
            false,
            "Login URL",
        )?;
        dialog.fields[0].read_only = true;
        dialog.fields[1].masked = true;
        dialog.fields[2].read_only = true;
        dialog.copy_field = Some(2);
        dialog.focus = 1;
        self.epoch = self.epoch.saturating_add(1);
        self.modal = Some(Modal {
            dialog,
            purpose: ModalPurpose::AccessToken(view.flow),
            expected: collection,
            previous: focus.focused,
        });
        focus.focused = Some(self.id);
        Ok(())
    }
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
        if !matches!(action, Action::InlineValue(_, _)) {
            self.inline = None;
        }
        match action {
            Action::Browse => {
                if self.modal.is_none() {
                    self.focused = Some(Field::File);
                    focus.focused = Some(self.id);
                }
                self.request_path(model)?;
            }
            Action::OAuth(op) => match op {
                OAuthAction::CopyUrl => {
                    if let Some(url) = &self.oauth.url {
                        self.intents.push(Intent::Clipboard {
                            action: FieldAction::Copy(url.clone()),
                            proof: None,
                        });
                    }
                }
                OAuthAction::ViewUrl | OAuthAction::PasteRedirect => {
                    let (title, value, purpose) = if op == OAuthAction::ViewUrl {
                        let Some(url) = self.oauth.url.clone() else {
                            return Ok(());
                        };
                        ("OAuth login URL", url, ModalPurpose::Display)
                    } else {
                        let Some(flow) = self.oauth.flow else {
                            return Ok(());
                        };
                        (
                            "Paste complete OAuth redirect URL",
                            String::new(),
                            ModalPurpose::Redirect(flow),
                        )
                    };
                    let mut dialog = EntryDialog::new(
                        title.into(),
                        editing::EntryValue {
                            key: "URL".into(),
                            value,
                            description: "Ctrl+A, Ctrl+C to copy; Enter to accept".into(),
                            kind: "string".into(),
                            enabled: true,
                        },
                        false,
                        "Hint",
                    )?;
                    dialog.fields[0].read_only = true;
                    dialog.fields[2].read_only = true;
                    dialog.fields[1].read_only = op == OAuthAction::ViewUrl;
                    dialog.focus = 1;
                    self.epoch = self.epoch.saturating_add(1);
                    self.modal = Some(Modal {
                        dialog,
                        purpose,
                        expected: model.export_collection(),
                        previous: focus.focused,
                    });
                    focus.focused = Some(self.id);
                }
                OAuthAction::PasteToken => {
                    let flow = self.oauth.flow.filter(|_| self.oauth.url.is_some());
                    let mut dialog = EntryDialog::new(
                        "Paste OAuth 2.0 access token".into(),
                        editing::EntryValue {
                            key: "Access token".into(),
                            value: String::new(),
                            description: "Used as Bearer; kept only in memory".into(),
                            kind: "secret".into(),
                            enabled: true,
                        },
                        false,
                        "Hint",
                    )?;
                    dialog.fields[0].read_only = true;
                    dialog.fields[1].masked = true;
                    dialog.fields[2].read_only = true;
                    dialog.focus = 1;
                    self.epoch = self.epoch.saturating_add(1);
                    self.modal = Some(Modal {
                        dialog,
                        purpose: ModalPurpose::AccessToken(flow),
                        expected: model.export_collection(),
                        previous: focus.focused,
                    });
                    focus.focused = Some(self.id);
                }
                _ => self.intents.push(Intent::OAuth(op)),
            },
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
            Action::RunHistory => self.intents.push(Intent::RunHistory),
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
            Action::InlineValue(kind, index) => {
                if kind == Rows::FormData {
                    return self.activate(Action::Edit(kind, index), model, focus);
                }
                if self
                    .inline
                    .as_ref()
                    .is_none_or(|inline| inline.kind != kind || inline.index != index)
                {
                    let entries = editing::rows(
                        model.node(&self.path).expect("checked node"),
                        &self.path,
                        kind,
                    )?;
                    let entry = entries
                        .get(index)
                        .ok_or_else(|| invalid("Selected row is no longer editable"))?;
                    let value = editing::entry_value(entry, kind);
                    let secret = entry.get("secret") == Some(&Value::Bool(true))
                        || value.kind == "secret"
                        || kind == Rows::Auth && kode_rest::auth::secret_field(&value.key);
                    let mut field = TextField::new(value.value);
                    field.framed = false;
                    field.masked = secret && !self.reveal;
                    self.inline = Some(InlineEdit { kind, index, field });
                }
                self.focused = None;
                focus.focused = Some(self.id);
            }
            Action::AuthTypeMenu => {
                self.auth_menu_open = !self.auth_menu_open;
                self.row_cache = None;
            }
            Action::Authenticate => {
                if self.oauth.url.is_some() {
                    let view = self.oauth.clone();
                    self.open_authenticate_dialog(&view, model.export_collection(), focus)?;
                } else {
                    self.oauth_dialog_pending = true;
                    self.intents.push(Intent::OAuth(OAuthAction::Acquire));
                }
            }
            Action::Auth(kind) => {
                editing::auth(model, &self.path, kind)?;
                self.auth_menu_open = false;
                self.row_cache = None;
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
                    || kind == Rows::Auth && kode_rest::auth::secret_field(&value.key);
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
                dialog.file_source = kind == Rows::FormData;
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
                    purpose: ModalPurpose::Row(kind, index),
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
    fn write_inline_value(
        &mut self,
        model: &mut Workspace,
        kind: Rows,
        index: usize,
        value: &str,
    ) -> Result<()> {
        let entries = editing::rows(
            model
                .node(&self.path)
                .ok_or_else(|| invalid("REST item no longer exists"))?,
            &self.path,
            kind,
        )?;
        let entry = entries
            .get(index)
            .ok_or_else(|| invalid("Selected row is no longer editable"))?;
        let mut next = editing::entry_value(entry, kind);
        next.value = value.into();
        editing::edit_row(model, &self.path, kind, index, &next)?;
        self.intents.push(Intent::Changed);
        Ok(())
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
            PasteTarget::Inline {
                kind,
                index,
                value,
                cursor,
                anchor,
            } => {
                let Some(inline) = self.inline.as_mut().filter(|inline| {
                    inline.kind == kind
                        && inline.index == index
                        && inline.field.value == value
                        && inline.field.cursor == cursor
                        && inline.field.anchor == anchor
                }) else {
                    return Ok(false);
                };
                let before = inline.field.value.clone();
                let action = inline.field.handle(&InputEvent::Paste(text));
                let next = inline.field.value.clone();
                if next != before {
                    self.write_inline_value(model, kind, index, &next)?;
                }
                if let Some(action) = action {
                    self.field_action(action, None);
                }
            }
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
            let browse = modal.dialog.take_browse();
            if browse {
                self.request_path(model)?;
            }
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
                        match modal.purpose {
                            ModalPurpose::Row(kind, index) => {
                                editing::edit_row(model, &self.path, kind, index, &value)?;
                                self.intents.push(Intent::Changed);
                            }
                            ModalPurpose::Redirect(flow) => {
                                self.intents.push(Intent::OAuthRedirect {
                                    flow,
                                    text: value.value,
                                })
                            }
                            ModalPurpose::AccessToken(flow) => {
                                self.intents.push(Intent::OAuthAccessToken {
                                    flow,
                                    text: value.value,
                                })
                            }
                            ModalPurpose::Display => {}
                        }
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
                        if let Action::InlineValue(kind, index) = hit.action
                            && let Some(inline) = self
                                .inline
                                .as_mut()
                                .filter(|inline| inline.kind == kind && inline.index == index)
                        {
                            inline.field.pointer(event, hit.area);
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
            if input.modifiers.alt
                && matches!(input.key, Key::Character('p' | 'P'))
                && self.focused == Some(Field::File)
            {
                self.request_path(model)?;
                return Ok(true);
            }
            if input.key == Key::Enter
                && input.modifiers.ctrl
                && !input.modifiers.alt
                && !input.modifiers.meta
                && crate::input::shortcuts::command_key(
                    crate::input::shortcuts::Scope::Request,
                    *input,
                )
                .is_some()
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
                    Key::Enter if self.focused.is_none() && self.inline.is_none() => {
                        if let Some((_, action)) = self.controls.get(self.selection).cloned() {
                            self.activate(action, model, focus)?;
                        }
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
        if let Some(inline) = self.inline.as_mut() {
            if matches!(event, InputEvent::Key(input) if input.key == Key::Enter) {
                self.inline = None;
                return Ok(true);
            }
            let proof = PasteProof {
                key: self.key.clone(),
                epoch: self.epoch,
                target: PasteTarget::Inline {
                    kind: inline.kind,
                    index: inline.index,
                    value: inline.field.value.clone(),
                    cursor: inline.field.cursor,
                    anchor: inline.field.anchor,
                },
            };
            let before = inline.field.value.clone();
            let action = inline.field.handle(event);
            let kind = inline.kind;
            let index = inline.index;
            let next = inline.field.value.clone();
            if next != before {
                self.write_inline_value(model, kind, index, &next)?;
            }
            if let Some(action) = action {
                self.field_action(action, Some(proof));
            }
            return Ok(true);
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
