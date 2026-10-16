use super::terminal::{Key, KeyInput};
use kode_core::commands::{EditorCommand, Movement};

pub fn editor_command(input: KeyInput) -> Option<EditorCommand> {
    let input = if matches!(input.key, Key::Character(_))
        && !input.modifiers.ctrl
        && !input.modifiers.alt
    {
        input
    } else {
        super::shortcuts::command_key(super::shortcuts::Scope::Editor, input)?
    };
    let modifiers = input.modifiers;
    if modifiers.ctrl
        && modifiers.shift
        && !modifiers.alt
        && !modifiers.meta
        && matches!(input.key, Key::Up | Key::Down)
    {
        return Some(EditorCommand::AddCursor {
            down: input.key == Key::Down,
        });
    }
    if input.key == Key::Escape && modifiers == Default::default() {
        return Some(EditorCommand::ClearSecondaryCursors);
    }
    let extend = modifiers.shift;
    let movement = match input.key {
        Key::Left => Some(if modifiers.ctrl {
            Movement::WordLeft
        } else {
            Movement::Left
        }),
        Key::Right => Some(if modifiers.ctrl {
            Movement::WordRight
        } else {
            Movement::Right
        }),
        Key::Up => Some(Movement::Up),
        Key::Down => Some(Movement::Down),
        Key::Home => Some(Movement::LineStart),
        Key::End => Some(Movement::LineEnd),
        Key::PageUp => Some(Movement::PageUp),
        Key::PageDown => Some(Movement::PageDown),
        _ => None,
    };
    if let Some(movement) = movement {
        return Some(EditorCommand::Move { movement, extend });
    }
    if let Key::Character(ch) = input.key {
        if modifiers.ctrl || modifiers.alt {
            return match ch.to_ascii_lowercase() {
                'c' => Some(EditorCommand::Copy),
                'x' => Some(EditorCommand::Cut),
                'v' => Some(EditorCommand::Paste),
                'a' if modifiers.ctrl => Some(EditorCommand::SelectAll),
                's' if modifiers.ctrl => Some(EditorCommand::Save),
                'f' if modifiers.ctrl => Some(EditorCommand::OpenSearch),
                ' ' if modifiers.ctrl => Some(EditorCommand::RequestCompletion),
                'u' if modifiers.alt && !modifiers.ctrl => Some(EditorCommand::Undo),
                'r' if modifiers.alt && !modifiers.ctrl => Some(EditorCommand::Redo),
                _ => None,
            };
        }
        if !modifiers.meta {
            return Some(EditorCommand::InsertText(ch.to_string()));
        }
    }
    match input.key {
        Key::Enter if !modifiers.ctrl && !modifiers.alt && !modifiers.meta => {
            Some(EditorCommand::InsertNewline)
        }
        Key::Tab if !modifiers.ctrl && !modifiers.alt && !modifiers.meta => {
            Some(EditorCommand::InsertTab)
        }
        Key::Backspace => Some(EditorCommand::DeleteBackward),
        Key::Delete => Some(EditorCommand::DeleteForward),
        _ => None,
    }
}
