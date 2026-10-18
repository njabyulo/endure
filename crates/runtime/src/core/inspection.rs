//! Read recorded inputs and outcomes without invoking adapters or model inference.
use super::journal::Journal;
use serde_json::{json, Value};
use std::path::Path;

/// Inspect recorded application data. Supplied rationale may appear in inputs or
/// outcomes; this never reconstructs or requests hidden model reasoning.
pub fn inspect_run(path: &Path, run_id: &str) -> Result<Value, String> {
    let journal = Journal::open(path).map_err(|e| e.to_string())?;
    let run = journal.get_run(run_id).map_err(|e| e.to_string())?;
    let snapshot: Value = serde_json::from_str(&run.graph).map_err(|e| e.to_string())?;
    let resources = journal.resource_report(run_id)?;
    let operations = journal.operations(run_id).map_err(|e| e.to_string())?;
    let attempts = journal.attempts(run_id).map_err(|e| e.to_string())?;
    let waits = journal.waits(run_id).map_err(|e| e.to_string())?;
    Ok(json!({
        "id": run.id, "status": run.status, "request_id": run.request_id,
        "code_hash": run.code_hash, "input": snapshot.get("input"),
        "permissions": { "legacy_net": snapshot.get("net"),
            "grants": snapshot.pointer("/host_config/grants") },
        "resources": resources, "operations": operations, "attempts": attempts, "waits": waits,
        "stdout": run.stdout, "stderr": run.stderr,
    }))
}

/// Compare recorded registrations by sequence; this does not execute either run
/// and does not imply that two external effects are interchangeable.
pub fn diff_runs(path: &Path, first: &str, second: &str) -> Result<Value, String> {
    let a = inspect_run(path, first)?;
    let b = inspect_run(path, second)?;
    let left = a["operations"].as_array().ok_or("missing operations")?;
    let right = b["operations"].as_array().ok_or("missing operations")?;
    let mut changes = Vec::new();
    for seq in 0..left.len().max(right.len()) {
        let before = left.get(seq);
        let after = right.get(seq);
        if before != after {
            changes.push(json!({"sequence": seq, "before": before, "after": after}));
        }
    }
    let mut metadata = Vec::new();
    for field in [
        "status",
        "code_hash",
        "input",
        "permissions",
        "stdout",
        "stderr",
        "attempts",
        "waits",
        "resources",
    ] {
        let before = comparable(field, &a[field]);
        let after = comparable(field, &b[field]);
        if before != after {
            metadata.push(json!({"field": field, "before": before, "after": after}));
        }
    }
    Ok(
        json!({"run_a": first, "run_b": second, "identical": changes.is_empty() && metadata.is_empty(), "operations": changes, "metadata": metadata}),
    )
}

// Ownership and database insertion timestamps identify records, not differences
// in outcomes. Full values remain available through inspect_run.
fn comparable(field: &str, value: &Value) -> Value {
    let mut value = value.clone();
    if field == "waits" {
        if let Some(waits) = value.as_array_mut() {
            for wait in waits {
                if let Some(wait) = wait.as_object_mut() {
                    wait.remove("run_id");
                }
            }
        }
    } else if field == "resources" {
        if let Some(report) = value.as_object_mut() {
            let duration = report
                .get("deadline_at")
                .and_then(Value::as_i64)
                .zip(report.get("started_at").and_then(Value::as_i64))
                .and_then(|(deadline, start)| deadline.checked_sub(start));
            report.remove("started_at");
            report.remove("deadline_at");
            report.insert("elapsed_deadline_ms".into(), json!(duration));
            if let Some(reservations) = report.get_mut("reservations").and_then(Value::as_array_mut)
            {
                for reservation in reservations {
                    if let Some(row) = reservation.as_object_mut() {
                        row.remove("id");
                        row.remove("admitted_at");
                    }
                }
            }
        }
    }
    value
}
