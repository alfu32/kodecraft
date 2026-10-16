use crate::{editor::host::OwnerSaveJob, focus::FocusId, widgets::dialog::TextDialog};
use kode_core::{RequestId, Result};
#[derive(Clone, Copy)]
pub enum Mode {
    Save,
    Catalog,
    Schema,
}
pub struct SessionDialog {
    pub nonce: RequestId,
    pub console: String,
    pub mode: Mode,
    pub dialog: TextDialog,
    pub save: Option<OwnerSaveJob>,
    pub previous_focus: Option<FocusId>,
    pub busy: bool,
}
#[derive(Clone)]
pub struct PasteProof {
    pub nonce: RequestId,
    pub value: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
}
impl SessionDialog {
    pub fn new(
        console: String,
        mode: Mode,
        title: &str,
        initial: String,
        previous_focus: Option<FocusId>,
        save: Option<OwnerSaveJob>,
    ) -> Result<Self> {
        Ok(Self {
            nonce: RequestId::fresh()?,
            console,
            mode,
            dialog: TextDialog::new(title.into(), initial),
            save,
            previous_focus,
            busy: false,
        })
    }
    pub fn proof(&self) -> PasteProof {
        PasteProof {
            nonce: self.nonce,
            value: self.dialog.field.value.clone(),
            cursor: self.dialog.field.cursor,
            anchor: self.dialog.field.anchor,
        }
    }
    pub fn paste(&mut self, proof: PasteProof, text: String) {
        if self.nonce == proof.nonce
            && self.dialog.field.value == proof.value
            && self.dialog.field.cursor == proof.cursor
            && self.dialog.field.anchor == proof.anchor
        {
            self.dialog
                .field
                .handle(&crate::input::terminal::InputEvent::Paste(text));
        }
    }
}
pub fn default_filename(title: &str) -> String {
    let title = title.split_once('@').map_or(title, |(_, tail)| tail).trim();
    let mut name = String::new();
    let mut invalid = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            name.push(ch);
            invalid = false;
        } else if !invalid {
            name.push('_');
            invalid = true;
        }
        if name.len() >= 160 {
            break;
        }
    }
    if name.is_empty() {
        name = "session".into();
    }
    if !name.to_ascii_lowercase().ends_with(".sql") {
        name.push_str(".sql");
    }
    name
}
