use crate::{ByteOffset, TextRange};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    DocumentStart,
    DocumentEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditKind {
    Typing,
    Newline,
    Paste,
    Delete,
    Cut,
    Replace,
    ReplaceAll,
    Completion,
    Programmatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EditorCommand {
    Move {
        movement: Movement,
        extend: bool,
    },
    MoveTo {
        position: ByteOffset,
        extend: bool,
    },
    InsertText(String),
    PasteText(String),
    AddCursor {
        down: bool,
    },
    ClearSecondaryCursors,
    InsertNewline,
    InsertTab,
    DeleteBackward,
    DeleteForward,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Save,
    OpenSearch,
    FindNext,
    FindAll,
    ReplaceCurrent,
    ReplaceAll,
    CloseSearch,
    ToggleFold,
    ExpandAllFolds,
    RequestCompletion,
    AcceptCompletion,
    Definitions,
    References,
    DismissPopup,
    Replace {
        range: TextRange,
        text: String,
        kind: EditKind,
    },
}
