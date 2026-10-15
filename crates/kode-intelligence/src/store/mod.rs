//! Rebuildable SQLite cache; all connection work belongs to its dedicated worker.
mod compatibility;
mod sqlite;
mod worker;
use crate::{ids::FileId, snapshot::SemanticSnapshot};
pub use compatibility::*;
use std::path::PathBuf;
pub use worker::{StoreClient, StoreReply, StoreTicket, StoreWorker};

#[derive(Debug, Clone)]
pub enum StoreLocation {
    Project(PathBuf),
    Memory,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    Fresh,
    Ready,
    Rebuilt { previous_version: u32 },
}
#[derive(Debug, Clone)]
pub struct StoredIndex {
    pub snapshot: SemanticSnapshot,
    pub compatibility: Vec<CompatibilityFile>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReceipt {
    pub revision: u64,
    pub changed_files: Vec<FileId>,
    pub removed_files: Vec<FileId>,
    pub reused: bool,
}
