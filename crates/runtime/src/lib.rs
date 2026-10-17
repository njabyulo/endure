//! Public Rust entry points; host.rs coordinates execution.

mod capabilities;
mod core;
mod host;
mod integrations;

pub use capabilities::policy::{Policy, PolicyError};
pub use host::{run_source, HostError, RunOutput};
