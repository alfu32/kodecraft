//! Application-thread SQL state. Documents own live text; persistence captures
//! cheap rope snapshots and materializes strings on a worker.
use kode_core::DocumentId;
use kode_database::{console::Manager, execution::Snapshot};
use kode_platform::jobs::CancellationToken;
use std::collections::HashMap;
use tokio::sync::watch;
#[derive(Clone, Copy)]
pub enum Control {
    AutoCommit,
    Commit,
    Rollback,
}
pub struct Highlight {
    pub snapshot: kode_core::document::TextSnapshot,
    pub selection: kode_core::Selection,
    pub dialect: kode_database::statement::Dialect,
}

pub struct Running {
    pub tag: kode_core::JobTag,
    pub cancel: CancellationToken,
    pub progress: watch::Receiver<Snapshot>,
    pub cancel_retry: Option<kode_core::JobTag>,
}
pub struct SqlConsoles {
    pub manager: Manager,
    pub documents: HashMap<DocumentId, String>,
    pub running: HashMap<String, Running>,
    pub results: HashMap<String, super::results::ResultsPanel>,
    pub persistence_enabled: bool,
    pub dialog: Option<super::session_dialog::SessionDialog>,
    pub menu: Option<super::statement_menu::StatementMenu>,
    pub toolbar: [ratatui::layout::Rect; 6],
    pub contexts: [ratatui::layout::Rect; 2],
    pub connections: HashMap<String, kode_database::connections::Snapshot>,
    pub diagnostics: HashMap<String, String>,
    pub controls: HashMap<String, (kode_core::JobTag, CancellationToken)>,
    pub saved: HashMap<String, (std::path::PathBuf, kode_core::services::DiskIdentity)>,
    pub hover: Option<(DocumentId, kode_core::Revision, kode_core::Selection)>,
}
impl Default for SqlConsoles {
    fn default() -> Self {
        Self {
            manager: Manager::default(),
            documents: HashMap::new(),
            running: HashMap::new(),
            results: HashMap::new(),
            persistence_enabled: true,
            dialog: None,
            menu: None,
            toolbar: [ratatui::layout::Rect::default(); 6],
            contexts: [ratatui::layout::Rect::default(); 2],
            connections: HashMap::new(),
            controls: HashMap::new(),
            diagnostics: HashMap::new(),
            saved: HashMap::new(),
            hover: None,
        }
    }
}
impl Drop for SqlConsoles {
    fn drop(&mut self) {
        for running in self.running.values() {
            running.cancel.cancel();
        }
        for (_, cancel) in self.controls.values() {
            cancel.cancel();
        }
    }
}
