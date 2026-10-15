//! External Java helper protocol/supervision. Construction is lazy; native
//! editor startup never probes Java or launches a helper.
pub mod client;
pub mod protocol;
pub mod runtime;
pub mod wire;
