//! Public Rust entry points; host.rs coordinates execution.

mod capabilities;
mod core;
mod host;
mod integrations;

pub use capabilities::policy::{Policy, PolicyError};
pub use core::durability::{journal_log, new_run_id};
pub use core::journal::{Journal, JournalError, OpIntent, OpRecord, RunRecord};
pub use core::modules::{ModuleError, ModuleGraph};
pub use host::{run_durable, run_module, run_source, HostError, RunOutput};
