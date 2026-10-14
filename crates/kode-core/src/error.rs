use crate::Revision;
use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DomainError>;

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("invalid range {start}..{end}")]
    InvalidRange { start: usize, end: usize },
    #[error("invalid UTF-8 boundary at byte {0}")]
    InvalidBoundary(usize),
    #[error("invalid {unit} coordinate {index} (maximum {limit})")]
    InvalidCoordinate {
        unit: &'static str,
        index: usize,
        limit: usize,
    },
    #[error("stale revision: expected {expected:?}, current {current:?}")]
    StaleRevision {
        expected: Revision,
        current: Revision,
    },
    #[error("document is read-only")]
    ReadOnly,
    #[error("no save target for virtual document")]
    NoSaveTarget,
    #[error("file changed on disk: {0}")]
    SaveConflict(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    #[error("queue capacity exceeded")]
    QueueFull,
    #[error("resource not found: {0}")]
    MissingResource(String),
    #[error("{operation} failed for {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid {format}: {message}")]
    Parse {
        format: &'static str,
        message: String,
    },
    #[error("Git {operation} failed: {message}")]
    Git { operation: String, message: String },
    #[error("HTTP transport failure: {0}")]
    HttpTransport(String),
    #[error("SQL {sqlstate:?} (vendor {vendor_code}): {message}")]
    Sql {
        sqlstate: Option<String>,
        vendor_code: i32,
        message: String,
    },
    #[error("process {program} failed: {message}")]
    Process { program: String, message: String },
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}
