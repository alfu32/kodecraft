use kode_core::intelligence::{DocumentSnapshot, NavigationTarget};
use kode_core::{DocumentOrigin, JobTag};

/// Reducers describe work. The composition layer executes effects outside drawing.
#[derive(Debug, Clone)]
pub enum Effect {
    Open {
        origin: DocumentOrigin,
        tag: JobTag,
    },
    Save {
        snapshot: DocumentSnapshot,
        tag: JobTag,
    },
    ClipboardWrite(String),
    ClipboardRead,
    Navigate(NavigationTarget),
    CancelOwner(u64),
    PersistSession,
    Repaint,
    Quit,
}
