//! Per-run operation boundary: durable intent before dispatch, result before JS.

use super::{
    journal::{Journal, OpIntent, OpRecord, WaitRecord},
    modules::ModuleGraph,
    operations::{
        validate_json, OperationIdentity, OperationSpec, RecoveryPolicy, MAX_OPERATION_RESULT_BYTES,
    },
    parking::RunLease,
};
use crate::{HostConfig, Policy, RunOutput};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(crate) struct Session {
    _lease: RunLease,
    pub(crate) journal: Journal,
    pub(crate) run_id: String,
    next_seq: u64,
    history: BTreeMap<u64, OpRecord>,
    consumed: BTreeSet<u64>,
    sealed: bool,
    failure: Option<String>,
    pending: BTreeMap<u32, u64>,
    intents: Vec<OpIntent>,
    last_flush: Instant,
    pub(crate) input: Value,
    pub(crate) host_config: Option<HostConfig>,
    waits: BTreeMap<u32, WaitRecord>,
}

impl Session {
    pub(crate) fn create(
        path: &Path,
        id: &str,
        policy: &Policy,
        graph: &ModuleGraph,
    ) -> Result<Self, String> {
        Self::create_request(path, id, policy, graph, None, Value::Null)
    }

    pub(crate) fn create_request(
        path: &Path,
        id: &str,
        policy: &Policy,
        graph: &ModuleGraph,
        request_id: Option<&str>,
        input: Value,
    ) -> Result<Self, String> {
        Self::create_request_config(path, id, policy, graph, request_id, input, None)
    }

    pub(crate) fn create_request_config(
        path: &Path,
        id: &str,
        policy: &Policy,
        graph: &ModuleGraph,
        request_id: Option<&str>,
        input: Value,
        host_config: Option<&HostConfig>,
    ) -> Result<Self, String> {
        if let Some(config) = host_config {
            config.validate()?;
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lease = RunLease::acquire(path, &format!("run:{id}"))?;
        let mut journal = Journal::open(path).map_err(|e| e.to_string())?;
        let mut snapshot = serde_json::json!({"graph": graph, "net": policy.net, "input": input});
        if let Some(config) = host_config {
            snapshot["host_config"] = serde_json::to_value(config).map_err(|e| e.to_string())?;
        }
        let mut identity = if request_id.is_some() {
            serde_json::json!({"net": policy.net, "input": input})
        } else {
            serde_json::json!({"net": policy.net})
        };
        if let Some(config) = host_config {
            identity["host_config"] = serde_json::to_value(config).map_err(|e| e.to_string())?;
        }
        journal
            .create_run(
                id,
                request_id,
                graph.code_hash(),
                &digest(&identity),
                &snapshot.to_string(),
            )
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _lease: lease,
            journal,
            run_id: id.into(),
            next_seq: 0,
            history: BTreeMap::new(),
            consumed: BTreeSet::new(),
            sealed: false,
            failure: None,
            pending: BTreeMap::new(),
            intents: Vec::new(),
            last_flush: Instant::now(),
            input,
            host_config: host_config.cloned(),
            waits: BTreeMap::new(),
        })
    }

    pub(crate) fn resume(path: &Path, id: &str) -> Result<(ModuleGraph, Policy, Self), String> {
        let lease = RunLease::acquire(path, &format!("run:{id}"))?;
        Self::resume_with_lease(path, id, lease)
    }

    pub(crate) fn resume_with_lease(
        path: &Path,
        id: &str,
        lease: RunLease,
    ) -> Result<(ModuleGraph, Policy, Self), String> {
        let journal = Journal::open(path).map_err(|e| e.to_string())?;
        let record = journal.get_run(id).map_err(|e| e.to_string())?;
        let snapshot: Value = serde_json::from_str(&record.graph).map_err(|e| e.to_string())?;
        let graph: ModuleGraph =
            serde_json::from_value(snapshot["graph"].clone()).map_err(|e| e.to_string())?;
        graph.verify_integrity().map_err(|e| e.to_string())?;
        if graph.code_hash() != record.code_hash {
            return Err("recorded code hash does not match snapshot".into());
        }
        let policy = Policy {
            net: snapshot["net"].as_bool().ok_or("invalid recorded policy")?,
        };
        let host_config: Option<HostConfig> = snapshot
            .get("host_config")
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|e| e.to_string())?;
        if let Some(config) = &host_config {
            config.validate()?;
        }
        let operations = journal.operations(id).map_err(|e| e.to_string())?;
        for (index, op) in operations.iter().enumerate() {
            if op.seq != index as u64 {
                return Err("journal operation sequence has a gap".into());
            }
        }
        let history = operations.into_iter().map(|op| (op.seq, op)).collect();
        let session = Self {
            _lease: lease,
            journal,
            run_id: id.into(),
            next_seq: 0,
            history,
            consumed: BTreeSet::new(),
            sealed: matches!(record.status.as_str(), "done" | "failed"),
            failure: None,
            pending: BTreeMap::new(),
            intents: Vec::new(),
            last_flush: Instant::now(),
            input: snapshot.get("input").cloned().unwrap_or(Value::Null),
            host_config,
            waits: BTreeMap::new(),
        };
        Ok((graph, policy, session))
    }

    fn diverge<T>(&mut self, message: impl Into<String>) -> Result<T, String> {
        let message = format!("replay divergence: {}", message.into());
        self.failure = Some(message.clone());
        Err(message)
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.sealed
    }

    pub(crate) fn check_idle(&mut self) -> Result<(), String> {
        if self.has_history() {
            return self.diverge("execution is idle before recorded completion was registered");
        }
        Ok(())
    }

    pub(crate) fn has_history(&self) -> bool {
        self.history
            .values()
            .any(|op| op.state == "completed" && !self.consumed.contains(&op.seq))
    }

    fn consume(&mut self, seq: u64) -> Result<(), String> {
        let expected = self
            .history
            .values()
            .filter(|op| op.state == "completed" && !self.consumed.contains(&op.seq))
            .min_by_key(|op| op.completion_order)
            .map(|op| op.seq);
        if expected != Some(seq) {
            return self.diverge(format!(
                "completion order mismatch at operation {seq}, expected {expected:?}"
            ));
        }
        self.consumed.insert(seq);
        Ok(())
    }

    pub(crate) fn is_cached(&self, id: u32) -> bool {
        self.pending
            .get(&id)
            .and_then(|seq| self.history.get(seq))
            .is_some_and(|op| op.state == "completed")
    }

    pub(crate) fn next_cached(&mut self) -> Result<Option<(u32, Value)>, String> {
        let Some(op) = self
            .history
            .values()
            .filter(|op| op.state == "completed" && !self.consumed.contains(&op.seq))
            .min_by_key(|op| op.completion_order)
        else {
            return Ok(None);
        };
        let seq = op.seq;
        let result = op.result.clone().ok_or("recorded result missing")?;
        let id = self
            .pending
            .iter()
            .find(|(_, pending)| **pending == seq)
            .map(|(&id, _)| id);
        let Some(id) = id else {
            // Idle rejection rendering can run JS that registers this operation.
            // The event loop validates remaining history after that final step.
            return Ok(None);
        };
        Ok(Some((
            id,
            serde_json::from_str(&result).map_err(|e| e.to_string())?,
        )))
    }

    fn intent(&mut self, spec: OperationSpec<'_>, args: &Value) -> Result<u64, String> {
        // Historical values predate adapter limits; verify identity without
        // imposing new bounds on a committed replay.
        if !self.history.contains_key(&self.next_seq) {
            spec.validate(args)?;
        }
        let name = spec.name;
        let policy = spec.recovery.journal_name();
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        if let Some(recorded) = self.history.get(&seq) {
            if recorded.name != name
                || recorded.args_digest != digest(args)
                || recorded.policy != policy
            {
                return self.diverge(format!("operation {seq} name or arguments changed"));
            }
            if recorded.state == "pending"
                && !RecoveryPolicy::recorded_is_retryable(&recorded.policy)
            {
                return self.diverge(format!(
                    "InterruptedError: operation {seq} cannot be retried"
                ));
            }
            return Ok(seq);
        }
        if self.sealed {
            return self.diverge(format!("unexpected operation {seq} after recorded end"));
        }
        self.intents.push(OpIntent {
            seq,
            name: name.into(),
            args_digest: digest(args),
            args_json: Some(args.to_string()),
            policy: policy.into(),
        });
        Ok(seq)
    }

    pub(crate) fn register(
        &mut self,
        id: u32,
        name: &str,
        args: &Value,
        recovery: RecoveryPolicy,
    ) -> Result<(), String> {
        let seq = self.intent(OperationSpec::new(name, recovery), args)?;
        self.pending.insert(id, seq);
        if self.intents.len() >= 256 || self.last_flush.elapsed() >= Duration::from_millis(2) {
            self.flush()?;
        }
        Ok(())
    }

    pub(crate) fn register_wait(
        &mut self,
        id: u32,
        kind: &str,
        args: &Value,
        wake_at: Option<i64>,
    ) -> Result<(), String> {
        self.register(
            id,
            if kind == "approval" {
                "wait.approval"
            } else {
                "wait.timer"
            },
            args,
            RecoveryPolicy::RetrySafe,
        )?;
        if self.is_cached(id) {
            return Ok(());
        }
        self.flush()?;
        let seq = self.pending[&id];
        let wait = self
            .journal
            .park_wait(&self.run_id, seq, kind, wake_at, &args.to_string())
            .map_err(|e| e.to_string())?;
        self.waits.insert(id, wait);
        Ok(())
    }

    pub(crate) fn ready_wait(&mut self) -> Result<Option<(u32, Value)>, String> {
        let now = epoch_millis()?;
        let ready = self
            .waits
            .iter()
            .find(|(_, wait)| wait.answer.is_some() || wait.wake_at.is_some_and(|at| at <= now));
        let Some((&id, wait)) = ready else {
            return Ok(None);
        };
        let value = if wait.kind == "timer" {
            self.journal
                .answer_wait(&self.run_id, wait.seq, "null")
                .map_err(|e| e.to_string())?;
            Value::Null
        } else {
            let answer = if let Some(answer) = &wait.answer {
                answer.clone()
            } else {
                self.journal
                    .timeout_wait(
                        &self.run_id,
                        wait.seq,
                        r#"{"approved":false,"note":"Approval timed out"}"#,
                        now,
                    )
                    .map_err(|e| e.to_string())?
            };
            let outcome = super::operations::approval_outcome(&answer)
                .unwrap_or_else(|error| serde_json::json!({"Err":error}));
            return Ok(Some((id, outcome)));
        };
        Ok(Some((id, serde_json::json!({"Ok": value}))))
    }

    pub(crate) fn next_wake(&self) -> Option<i64> {
        self.waits.values().filter_map(|wait| wait.wake_at).min()
    }

    pub(crate) fn should_park(&self) -> bool {
        !self.waits.is_empty()
    }

    pub(crate) fn flush(&mut self) -> Result<(), String> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        if !self.intents.is_empty() {
            self.journal
                .intents(&self.run_id, &self.intents)
                .map_err(|e| e.to_string())?;
            self.intents.clear();
            self.last_flush = Instant::now();
        }
        Ok(())
    }

    /// Commit a dispatch attempt before contacting an external adapter.
    pub(crate) fn begin_dispatch(&mut self, id: u32) -> Result<OperationIdentity, String> {
        self.flush()?;
        let sequence = *self
            .pending
            .get(&id)
            .ok_or("operation missing from journal session")?;
        if self.is_cached(id) {
            return Err("cannot dispatch a completed operation".into());
        }
        self.journal
            .begin_attempt(&self.run_id, sequence)
            .map_err(|e| e.to_string())?;
        Ok(OperationIdentity {
            run_id: self.run_id.clone(),
            sequence,
        })
    }

    pub(crate) fn complete(&mut self, id: u32, result: &Value) -> Result<(), String> {
        if !self.is_cached(id) {
            validate_json(result, MAX_OPERATION_RESULT_BYTES, "result")?;
        }
        self.flush()?;
        self.waits.remove(&id);
        let seq = self
            .pending
            .remove(&id)
            .ok_or("operation missing from journal session")?;
        if let Some(recorded) = self.history.get(&seq) {
            if recorded.state == "completed" {
                let encoded = result.to_string();
                if recorded.result.as_deref() != Some(encoded.as_str()) {
                    return self.diverge(format!("result mismatch at operation {seq}"));
                }
                return self.consume(seq);
            }
        }
        self.journal
            .complete(&self.run_id, seq, &result.to_string())
            .map_err(|e| e.to_string())
    }

    pub(crate) fn cancel(&mut self, id: u32) -> Result<(), String> {
        self.sync_op("timer.cancel", &serde_json::json!([id]), || Ok(Value::Null))?;
        if self.pending.contains_key(&id) {
            self.complete(id, &serde_json::json!({"canceled": true}))?;
        }
        Ok(())
    }

    /// Capture a safe synchronous value and commit intent plus result together.
    /// Producers must have no external effects: a crash before commit can retry
    /// them, and JavaScript cannot observe the value until commit succeeds.
    pub(crate) fn sync_op(
        &mut self,
        name: &str,
        args: &Value,
        produce: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        let seq = self.intent(OperationSpec::new(name, RecoveryPolicy::RetrySafe), args)?;
        if let Some(recorded) = self.history.get(&seq) {
            if recorded.state == "completed" {
                let encoded = recorded.result.clone().ok_or("recorded result missing")?;
                self.consume(seq)?;
                // Decode the same Result envelope written below, returning its value
                // or recorded error rather than exposing the envelope to callers.
                return serde_json::from_str::<Result<Value, String>>(&encoded)
                    .map_err(|e| e.to_string())?;
            }
        }
        let result = produce();
        let value = serde_json::to_value(&result).map_err(|e| e.to_string())?;
        validate_json(&value, MAX_OPERATION_RESULT_BYTES, "result")?;
        let encoded = value.to_string();
        self.journal
            .complete_sync(&self.run_id, &self.intents, seq, &encoded)
            .map_err(|e| e.to_string())?;
        self.intents.clear();
        self.last_flush = Instant::now();
        result
    }

    pub(crate) fn finish(&mut self, status: &str, output: &RunOutput) -> Result<(), String> {
        let status = if status == "done" && self.should_park() {
            "parked"
        } else {
            status
        };
        if matches!(status, "done" | "parked")
            && (self.next_seq < self.history.len() as u64 || self.has_history())
        {
            self.failure =
                Some("replay divergence: execution ended before recorded history".into());
        }
        if self.sealed {
            // Replays cannot replace the original terminal status or output,
            // even if this replay diverges or is killed before finishing.
            return self.failure.clone().map_or(Ok(()), Err);
        }
        if let Some(error) = self.failure.clone() {
            self.journal
                .finish_run(&self.run_id, "diverged", &output.stdout, &output.stderr)
                .map_err(|e| e.to_string())?;
            return Err(error);
        }
        self.flush()?;
        self.journal
            .finish_run(&self.run_id, status, &output.stdout, &output.stderr)
            .map_err(|e| e.to_string())
    }
}

/// Generate an opaque identifier for a new journaled run.
pub fn new_run_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Inspect durable operation records without executing the run.
pub fn journal_log(path: &Path, run_id: &str) -> Result<String, String> {
    let journal = Journal::open(path).map_err(|e| e.to_string())?;
    let run = journal.get_run(run_id).map_err(|e| e.to_string())?;
    let ops = journal.operations(run_id).map_err(|e| e.to_string())?;
    let waits = journal.waits(run_id).map_err(|e| e.to_string())?;
    let attempts = journal.attempts(run_id).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&serde_json::json!({"id": run.id, "status": run.status, "request_id": run.request_id, "code_hash": run.code_hash, "operations": ops, "attempts": attempts, "waits": waits}))
        .map_err(|e| e.to_string())
}

pub(crate) fn epoch_millis() -> Result<i64, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    i64::try_from(elapsed.as_millis()).map_err(|_| "wall clock exceeds supported range".into())
}

pub(crate) fn wait_ready(wait: &WaitRecord, now: i64) -> bool {
    wait.answer.is_some() || wait.wake_at.is_some_and(|at| at <= now)
}

pub(crate) fn can_remain_parked(journal: &Journal, run_id: &str) -> Result<bool, String> {
    let now = epoch_millis()?;
    let pending: Vec<_> = journal
        .operations(run_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|op| op.state == "pending")
        .collect();
    let waits = journal.waits(run_id).map_err(|e| e.to_string())?;
    Ok(!pending.is_empty()
        && pending.iter().all(|op| {
            waits
                .iter()
                .find(|wait| wait.seq == op.seq)
                .is_some_and(|wait| !wait_ready(wait, now))
        }))
}

/// Answer an approval once, then wake the run using its persisted snapshot.
pub fn approve_run(
    path: &Path,
    run_id: &str,
    seq: u64,
    answer: bool,
) -> Result<RunOutput, crate::HostError> {
    approve_run_with_limits(
        path,
        run_id,
        seq,
        answer,
        super::limits::RunLimits::default(),
    )
}

pub fn approve_run_with_limits(
    path: &Path,
    run_id: &str,
    seq: u64,
    answer: bool,
    limits: super::limits::RunLimits,
) -> Result<RunOutput, crate::HostError> {
    approve_run_with_note_and_limits(path, run_id, seq, answer, None, limits)
}

/// Answer as the trusted local operator; recipient labels are descriptive only.
pub fn approve_run_with_note_and_limits(
    path: &Path,
    run_id: &str,
    seq: u64,
    answer: bool,
    note: Option<&str>,
    limits: super::limits::RunLimits,
) -> Result<RunOutput, crate::HostError> {
    let record_answer = || -> Result<(), String> {
        let _lease = RunLease::acquire(path, &format!("run:{run_id}"))?;
        let mut journal = Journal::open(path).map_err(|e| e.to_string())?;
        let wait = journal
            .waits(run_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|w| w.seq == seq)
            .ok_or("unknown approval wait")?;
        if wait.kind != "approval" {
            return Err("wait is not an approval".into());
        }
        let agent = serde_json::from_str::<Value>(&wait.details)
            .ok()
            .and_then(|v| v.get("resultFormat").cloned())
            == Some(serde_json::json!("agent-v1"));
        let value = if agent {
            serde_json::json!({"approved":answer,"note":note}).to_string()
        } else if note.is_some() {
            return Err("approval notes require an agent approval".into());
        } else {
            answer.to_string()
        };
        journal
            .answer_wait(run_id, seq, &value)
            .map_err(|e| e.to_string())?;
        Ok(())
    };
    record_answer().map_err(|message| crate::HostError {
        filename: run_id.into(),
        message,
        output: RunOutput::default(),
    })?;
    crate::replay_run_with_limits(path, run_id, None, limits)
}

/// Result of attempting one ready run. A failed run does not stop the batch.
#[derive(Debug)]
pub struct WakeOutcome {
    pub run_id: String,
    pub result: Result<super::journal::RunRecord, crate::HostError>,
}

/// Recover abandoned runs and wake ready waits, skipping runs with live owners.
pub fn wake_ready(path: &Path) -> Result<Vec<WakeOutcome>, crate::HostError> {
    wake_ready_with_limits(path, super::limits::RunLimits::default())
}

pub fn wake_ready_with_limits(
    path: &Path,
    limits: super::limits::RunLimits,
) -> Result<Vec<WakeOutcome>, crate::HostError> {
    let fail = |message| crate::HostError {
        filename: path.display().to_string(),
        message,
        output: RunOutput::default(),
    };
    let journal = Journal::open(path).map_err(|e| fail(e.to_string()))?;
    let mut awakened = Vec::new();
    for run in journal
        .recoverable_runs()
        .map_err(|e| fail(e.to_string()))?
    {
        let result = match wake_run(path, &journal, &run.id, limits) {
            Ok(None) => continue,
            Ok(Some(run)) => Ok(run),
            Err(error) => Err(error),
        };
        awakened.push(WakeOutcome {
            run_id: run.id,
            result,
        });
    }
    Ok(awakened)
}

fn wake_run(
    path: &Path,
    journal: &Journal,
    run_id: &str,
    limits: super::limits::RunLimits,
) -> Result<Option<super::journal::RunRecord>, crate::HostError> {
    let fail = |message| crate::HostError {
        filename: run_id.into(),
        message,
        output: RunOutput::default(),
    };
    let Some(lease) = RunLease::try_acquire(path, &format!("run:{run_id}")).map_err(fail)? else {
        return Ok(None);
    };
    // The scan is only a hint: another owner may have finished since it ran.
    let current = journal.get_run(run_id).map_err(|e| fail(e.to_string()))?;
    if !matches!(current.status.as_str(), "parked" | "running") {
        return Ok(None);
    }
    if current.status == "parked" && can_remain_parked(journal, run_id).map_err(fail)? {
        return Ok(None);
    }
    let (graph, policy, session) = Session::resume_with_lease(path, run_id, lease).map_err(fail)?;
    crate::host::replay_session(graph, policy, session, None, limits)?;
    journal
        .get_run(run_id)
        .map(Some)
        .map_err(|e| fail(e.to_string()))
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;

    #[test]
    fn stale_candidates_are_checked_again_after_claiming_the_lease() {
        let directory = std::env::temp_dir().join(format!("ondure-wake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("journal.sqlite3");
        let mut journal = Journal::open(&path).unwrap();
        // Invalid snapshots deliberately prove terminal candidates are never replayed.
        for status in ["done", "failed", "diverged"] {
            journal
                .create_run(status, None, "hash", "input", "{}")
                .unwrap();
            assert!(journal
                .recoverable_runs()
                .unwrap()
                .iter()
                .any(|run| run.id == status));
            journal.finish_run(status, status, "saved", "").unwrap();
            assert!(wake_run(
                &path,
                &journal,
                status,
                super::super::limits::RunLimits::default()
            )
            .unwrap()
            .is_none());
            assert_eq!(journal.get_run(status).unwrap().status, status);
        }
        drop(journal);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn live_owners_are_skipped_without_batch_errors() {
        let directory = std::env::temp_dir().join(format!("ondure-wake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("journal.sqlite3");
        let mut journal = Journal::open(&path).unwrap();
        journal
            .create_run("live", None, "hash", "input", "{}")
            .unwrap();
        let lease = RunLease::acquire(&path, "run:live").unwrap();
        assert!(wake_ready(&path).unwrap().is_empty());
        assert_eq!(journal.get_run("live").unwrap().status, "running");
        drop((journal, lease));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
