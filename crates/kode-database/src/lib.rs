//! Native database models/configuration. JDBC execution belongs to the external
//! helper; loading this crate never starts Java or opens a connection.
pub mod bridge;
pub mod catalog;
pub mod config;
pub mod connections;
pub mod credentials;
pub mod drivers;
pub mod execution;
pub mod model;
pub mod security;
