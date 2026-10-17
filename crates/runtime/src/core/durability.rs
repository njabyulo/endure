//! Per-run operation boundary: durable intent before dispatch, result before JS.

use super::{
    journal::{Journal, OpIntent},
    modules::ModuleGraph,
};
use crate::{Policy, RunOutput};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(crate) struct Session {
    pub(crate) journal: Journal,
    pub(crate) run_id: String,
    next_seq: u64,
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
            journal,
            run_id: id.into(),
            next_seq: 0,
            pending: BTreeMap::new(),
            intents: Vec::new(),
            last_flush: Instant::now(),
        })
    }

    fn intent(&mut self, name: &str, args: &Value, policy: &str) -> Result<u64, String> {
        let seq = self.next_seq;
        self.next_seq += 1;
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
        let result = produce();
        let encoded = serde_json::to_string(&result).map_err(|e| e.to_string())?;
        self.journal
            .complete(&self.run_id, seq, &encoded)
            .map_err(|e| e.to_string())?;
        result
    }

    pub(crate) fn finish(&mut self, status: &str, output: &RunOutput) -> Result<(), String> {
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
