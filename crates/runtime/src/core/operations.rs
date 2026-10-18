//! Shared contract for registered host operations.
//!
//! Identity is a run plus its deterministic registration sequence, never a
//! transient promise ID. Only explicitly retry-safe interrupted work is retried.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub const MAX_OPERATION_ARGUMENT_BYTES: usize = 256 * 1024;
pub const MAX_OPERATION_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RecoveryPolicy {
    /// Reissuing an unfinished operation is explicitly safe for this adapter.
    RetrySafe,
    /// An unfinished operation requires operator reconciliation, not retry.
    Interrupt,
}

impl RecoveryPolicy {
    pub(crate) fn journal_name(self) -> &'static str {
        match self {
            Self::RetrySafe => "safe",
            Self::Interrupt => "never",
        }
    }

    /// Unknown legacy policies must never accidentally acquire retry authority.
    pub(crate) fn recorded_is_retryable(policy: &str) -> bool {
        policy == "safe"
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationIdentity {
    pub run_id: String,
    pub sequence: u64,
}

/// Adapter-selected operation name and explicit recovery policy.
pub(crate) struct OperationSpec<'a> {
    pub name: &'a str,
    pub recovery: RecoveryPolicy,
}

impl<'a> OperationSpec<'a> {
    pub(crate) fn new(name: &'a str, recovery: RecoveryPolicy) -> Self {
        Self { name, recovery }
    }

    pub(crate) fn validate(&self, args: &Value) -> Result<(), String> {
        if self.name.is_empty()
            || self.name.len() > 128
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err("invalid host operation name".into());
        }
        validate_json(args, MAX_OPERATION_ARGUMENT_BYTES, "arguments")
    }
}

pub(crate) fn validate_json(value: &Value, limit: usize, kind: &str) -> Result<(), String> {
    let encoded = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if encoded.len() > limit {
        return Err(format!("host operation {kind} exceed {limit} bytes"));
    }
    Ok(())
}

/// Cooperative adapter cancellation. Cancellation cannot undo external effects.
#[derive(Clone, Default)]
pub struct OperationCancellation(Arc<AtomicBool>);

impl OperationCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("host operation canceled".into())
        } else {
            Ok(())
        }
    }
}

/// Event-loop ownership ends adapter work even when a run exits with an error.
pub(crate) struct CancellationGuard(pub OperationCancellation);
impl Drop for CancellationGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_names_and_json_limits() {
        assert!(OperationSpec::new("fetch.get", RecoveryPolicy::RetrySafe)
            .validate(&serde_json::json!([]))
            .is_ok());
        assert!(OperationSpec::new("", RecoveryPolicy::RetrySafe)
            .validate(&Value::Null)
            .is_err());
        assert!(OperationSpec::new("bad/name", RecoveryPolicy::RetrySafe)
            .validate(&Value::Null)
            .is_err());
        assert!(validate_json(&serde_json::json!("x".repeat(20)), 20, "arguments").is_err());
    }
    #[test]
    fn unknown_policies_never_retry() {
        assert!(RecoveryPolicy::recorded_is_retryable("safe"));
        for policy in ["never", "unsafe", "retry", ""] {
            assert!(!RecoveryPolicy::recorded_is_retryable(policy));
        }
    }
    #[test]
    fn cancellation_propagates_to_adapter_and_on_exit() {
        let cancellation = OperationCancellation::default();
        let adapter = cancellation.clone();
        assert!(adapter.check().is_ok());
        drop(CancellationGuard(cancellation));
        assert!(adapter.is_cancelled());
        assert!(adapter.check().is_err());
    }
}
