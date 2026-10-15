//! Local Git operations. Open handles stay inside F03 blocking workers; only
//! owned models and repository locations cross the application boundary.
pub mod diff;
pub mod model;
pub mod read;
pub mod repository;
pub mod status;
pub mod write;
