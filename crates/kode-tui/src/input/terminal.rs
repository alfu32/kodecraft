use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}
impl From<KeyModifiers> for Modifiers {
    fn from(value: KeyModifiers) -> Self {
        Self {
            ctrl: value.contains(KeyModifiers::CONTROL),
            alt: value.contains(KeyModifiers::ALT),
            shift: value.contains(KeyModifiers::SHIFT),
            meta: value.intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Escape,
    Insert,
    Function(u8),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub modifiers: Modifiers,
    pub repeat: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAction {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    Move,
    Scroll { horizontal: i16, vertical: i16 },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerInput {
    pub x: u16,
    pub y: u16,
    pub action: PointerAction,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyInput),
    Pointer(PointerInput),
    Paste(String),
    Resize { width: u16, height: u16 },
    Focus(bool),
}

/// Normalization happens once. Printable case/modifiers and source paste bytes
/// are retained; release events cannot cause duplicate edits.
pub fn normalize(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(event) => {
            if event.kind == KeyEventKind::Release {
                return None;
            }
            let mut modifiers = Modifiers::from(event.modifiers);
            let key = match event.code {
                KeyCode::Char('\r' | '\n') | KeyCode::Enter => Key::Enter,
                KeyCode::Null => {
                    modifiers.ctrl = true;
                    Key::Character(' ')
                }
                KeyCode::Char(ch) => Key::Character(ch),
                KeyCode::Tab => Key::Tab,
                KeyCode::BackTab => {
                    modifiers.shift = true;
                    Key::Tab
                }
                KeyCode::Backspace => Key::Backspace,
                KeyCode::Delete => Key::Delete,
                KeyCode::Left => Key::Left,
                KeyCode::Right => Key::Right,
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Home => Key::Home,
                KeyCode::End => Key::End,
                KeyCode::PageUp => Key::PageUp,
                KeyCode::PageDown => Key::PageDown,
                KeyCode::Esc => Key::Escape,
                KeyCode::Insert => Key::Insert,
                KeyCode::F(number) => Key::Function(number),
                _ => return None,
            };
            Some(InputEvent::Key(KeyInput {
                key,
                modifiers,
                repeat: event.kind == KeyEventKind::Repeat,
            }))
        }
        Event::Mouse(event) => {
            let action = match event.kind {
                MouseEventKind::Down(button) => PointerAction::Down(button),
                MouseEventKind::Up(button) => PointerAction::Up(button),
                MouseEventKind::Drag(button) => PointerAction::Drag(button),
                MouseEventKind::Moved => PointerAction::Move,
                MouseEventKind::ScrollUp => PointerAction::Scroll {
                    horizontal: 0,
                    vertical: -1,
                },
                MouseEventKind::ScrollDown => PointerAction::Scroll {
                    horizontal: 0,
                    vertical: 1,
                },
                MouseEventKind::ScrollLeft => PointerAction::Scroll {
                    horizontal: -1,
                    vertical: 0,
                },
                MouseEventKind::ScrollRight => PointerAction::Scroll {
                    horizontal: 1,
                    vertical: 0,
                },
            };
            Some(InputEvent::Pointer(PointerInput {
                x: event.column,
                y: event.row,
                action,
                modifiers: event.modifiers.into(),
            }))
        }
        Event::Paste(text) => Some(InputEvent::Paste(text)),
        Event::Resize(width, height) => Some(InputEvent::Resize { width, height }),
        Event::FocusGained => Some(InputEvent::Focus(true)),
        Event::FocusLost => Some(InputEvent::Focus(false)),
    }
}
