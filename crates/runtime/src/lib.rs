//! Public Rust entry points; host.rs coordinates execution.

mod capabilities;
mod core;
mod host;
mod integrations;

pub use capabilities::policy::{Policy, PolicyError};
pub use core::durability::{approve_run, wake_ready, WakeOutcome};
pub use core::durability::{journal_log, new_run_id};
pub use core::journal::{Journal, JournalError, OpIntent, OpRecord, RunRecord, WaitRecord};
pub use core::modules::{ModuleError, ModuleGraph};
pub use host::{
    replay_run, run_durable, run_module, run_request, run_source, HostError, RunOutput,
};

pub use core::limits::RunLimits;
pub use host::{
    replay_run_with_limits, run_durable_with_limits, run_module_with_limits,
    run_request_with_limits, run_source_with_limits,
};

pub use core::durability::{approve_run_with_limits, wake_ready_with_limits};

pub use integrations::io::MAX_OUTPUT_BYTES;
