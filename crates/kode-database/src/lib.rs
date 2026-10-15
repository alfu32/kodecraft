//! Native database models/configuration. JDBC execution belongs to the external
//! helper; loading this crate never starts Java or opens a connection.
pub mod catalog;
pub mod model;
pub mod security;
