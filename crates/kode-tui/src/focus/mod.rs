use crate::input::terminal::InputEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FocusId(pub u64);
#[derive(Debug, Default)]
pub struct FocusState {
    pub focused: Option<FocusId>,
    pub hovered: Option<FocusId>,
    pub captured: Option<FocusId>,
}

pub trait EventTarget {
    fn dispatch(&mut self, event: &InputEvent) -> bool;
}
pub struct Route<'a> {
    pub modal: Option<&'a mut dyn EventTarget>,
    pub capture_or_popup: Option<&'a mut dyn EventTarget>,
    pub child: Option<&'a mut dyn EventTarget>,
    pub panel: Option<&'a mut dyn EventTarget>,
    pub global: Option<&'a mut dyn EventTarget>,
}
impl Route<'_> {
    pub fn dispatch(self, event: &InputEvent) -> bool {
        if let Some(modal) = self.modal {
            modal.dispatch(event);
            return true;
        }
        for target in [self.capture_or_popup, self.child, self.panel, self.global]
            .into_iter()
            .flatten()
        {
            if target.dispatch(event) {
                return true;
            }
        }
        false
    }
}

#[derive(Debug)]
pub struct ModalEntry<T> {
    pub id: FocusId,
    pub value: T,
    previous_focus: Option<FocusId>,
}
#[derive(Debug)]
pub struct ModalStack<T> {
    entries: Vec<ModalEntry<T>>,
}
impl<T> Default for ModalStack<T> {
    fn default() -> Self {
        Self { entries: vec![] }
    }
}
impl<T> ModalStack<T> {
    pub fn push(&mut self, id: FocusId, value: T, focus: &mut FocusState) {
        let previous_focus = focus.focused;
        focus.focused = Some(id);
        focus.captured = None;
        self.entries.push(ModalEntry {
            id,
            value,
            previous_focus,
        });
    }
    pub fn pop(&mut self, focus: &mut FocusState) -> Option<T> {
        let entry = self.entries.pop()?;
        focus.focused = entry.previous_focus;
        focus.captured = None;
        Some(entry.value)
    }
    pub fn top(&mut self) -> Option<&mut T> {
        self.entries.last_mut().map(|entry| &mut entry.value)
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
