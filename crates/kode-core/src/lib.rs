//! UI-independent identities, coordinates, editor commands and service boundaries.
pub mod commands;
pub mod coordinates;
pub mod document;
pub mod editor;
pub mod error;
pub mod ids;
pub mod intelligence;
pub mod services;

pub use coordinates::{
    ByteOffset, CellColumn, CharOffset, LineIndex, Selection, TextRange, Utf16Column,
};
pub use error::{DomainError, Result};
pub use ids::{DocumentId, DocumentOrigin, JobTag, RequestId, Revision, WorkspaceGeneration};
