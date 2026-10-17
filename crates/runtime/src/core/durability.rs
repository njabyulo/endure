//! Per-run operation boundary: durable intent before dispatch, result before JS.

use super::{
    journal::{Journal, OpIntent, OpRecord},
    modules::ModuleGraph,
    parking::RunLease,
};
use crate::{Policy, RunOutput};
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
}

impl Session {
    pub(crate) fn create(
        path: &Path,
        id: &str,
        policy: &Policy,
        graph: &ModuleGraph,
    ) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut journal = Journal::open(path).map_err(|e| e.to_string())?;
        let lease = RunLease::acquire(path, &format!("run:{id}"))?;
        let snapshot = serde_json::json!({"graph": graph, "net": policy.net});
        let input = serde_json::json!({"net": policy.net});
        journal
            .create_run(
                id,
                None,
                graph.code_hash(),
                &digest(&input),
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
        })
    }

    pub(crate) fn resume(path: &Path, id: &str) -> Result<(ModuleGraph, Policy, Self), String> {
        let journal = Journal::open(path).map_err(|e| e.to_string())?;
        let lease = RunLease::acquire(path, &format!("run:{id}"))?;
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

    fn intent(&mut self, name: &str, args: &Value, policy: &str) -> Result<u64, String> {
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
            if recorded.state == "pending" && recorded.policy != "safe" {
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
            policy: policy.into(),
        });
        if self.intents.len() >= 256 || self.last_flush.elapsed() >= Duration::from_millis(2) {
            self.flush()?;
        }
        Ok(seq)
    }

    pub(crate) fn register(
        &mut self,
        id: u32,
        name: &str,
        args: &Value,
        policy: &str,
    ) -> Result<(), String> {
        let seq = self.intent(name, args, policy)?;
        self.pending.insert(id, seq);
        Ok(())
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

    pub(crate) fn complete(&mut self, id: u32, result: &Value) -> Result<(), String> {
        self.flush()?;
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

    pub(crate) fn sync_op(
        &mut self,
        name: &str,
        args: &Value,
        produce: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        let seq = self.intent(name, args, "safe")?;
        self.flush()?;
        if let Some(recorded) = self.history.get(&seq) {
            if recorded.state == "completed" {
                let encoded = recorded.result.clone().ok_or("recorded result missing")?;
                self.consume(seq)?;
                return serde_json::from_str(&encoded).map_err(|e| e.to_string())?;
            }
        }
        let result = produce();
        let encoded = serde_json::to_string(&result).map_err(|e| e.to_string())?;
        self.journal
            .complete(&self.run_id, seq, &encoded)
            .map_err(|e| e.to_string())?;
        result
    }

    pub(crate) fn finish(&mut self, status: &str, output: &RunOutput) -> Result<(), String> {
        if status == "done" && (self.next_seq < self.history.len() as u64 || self.has_history()) {
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
    journal.get_run(run_id).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&journal.operations(run_id).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
