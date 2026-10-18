//! Per-run estimates are reservations, not provider billing guarantees.
use super::journal::Journal;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceLimits {
    pub token_limit: Option<u64>,
    pub dollar_limit_micros: Option<u64>,
    pub model_concurrency: usize,
    pub tool_concurrency: usize,
    pub model_rate: Option<RateLimit>,
    pub tool_rate: Option<RateLimit>,
    pub elapsed_deadline_ms: Option<u64>,
    pub model_input_token_estimate: u64,
    pub input_price_micros_per_million_tokens: u64,
    pub output_price_micros_per_million_tokens: u64,
    pub tool_estimate_micros: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub max_calls: u64,
    pub window_ms: u64,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            token_limit: None,
            dollar_limit_micros: None,
            model_concurrency: 4,
            tool_concurrency: 16,
            model_rate: None,
            tool_rate: None,
            elapsed_deadline_ms: None,
            model_input_token_estimate: 4096,
            input_price_micros_per_million_tokens: 0,
            output_price_micros_per_million_tokens: 0,
            tool_estimate_micros: 0,
        }
    }
}
impl ResourceLimits {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.model_concurrency == 0 || self.tool_concurrency == 0 {
            return Err("resource concurrency must be positive".into());
        }
        for rate in [&self.model_rate, &self.tool_rate].into_iter().flatten() {
            if rate.max_calls == 0
                || rate.window_ms == 0
                || rate.max_calls > i64::MAX as u64
                || rate.window_ms > i64::MAX as u64
            {
                return Err("rate limits must be positive".into());
            }
        }
        for n in [
            self.token_limit,
            self.dollar_limit_micros,
            self.elapsed_deadline_ms,
            Some(self.model_input_token_estimate),
            Some(self.input_price_micros_per_million_tokens),
            Some(self.output_price_micros_per_million_tokens),
            Some(self.tool_estimate_micros),
        ]
        .into_iter()
        .flatten()
        {
            if n > i64::MAX as u64 {
                return Err("resource limit exceeds SQLite range".into());
            }
        }
        Ok(())
    }
}
pub(crate) const SCHEMA:&str="CREATE TABLE run_resources(run_id TEXT PRIMARY KEY REFERENCES runs(id),started_at INTEGER NOT NULL,deadline_at INTEGER);
CREATE TABLE resource_reservations(id INTEGER PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id),seq INTEGER NOT NULL,category TEXT NOT NULL,tokens INTEGER NOT NULL,micros INTEGER NOT NULL,state TEXT NOT NULL CHECK(state IN ('active','interrupted','settled')),admitted_at INTEGER NOT NULL,input_price INTEGER NOT NULL,output_price INTEGER NOT NULL);
CREATE INDEX reservation_run ON resource_reservations(run_id,category,admitted_at);";
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    Ready,
    /// Capacity can change only when an owned operation completes; no timer.
    Blocked,
    /// Rolling rate window opens at this absolute epoch millisecond.
    Deferred(i64),
    Denied(String),
}
fn db(e: rusqlite::Error) -> String {
    format!("resources: {e}")
}
fn price(tokens: u64, per_million: u64) -> Result<u64, String> {
    let value = (u128::from(tokens) * u128::from(per_million)).div_ceil(1_000_000);
    u64::try_from(value)
        .ok()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or("resource price overflow".into())
}
impl Journal {
    /// Inspect conservative per-run accounting; micros are millionths of a dollar.
    pub fn resource_report(&self, run: &str) -> Result<Value, String> {
        let clock: Option<(i64, Option<i64>)> = self
            .connection
            .query_row(
                "SELECT started_at,deadline_at FROM run_resources WHERE run_id=?1",
                [run],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        let mut statement=self.connection.prepare("SELECT id,seq,category,tokens,micros,state,admitted_at FROM resource_reservations WHERE run_id=?1 ORDER BY id").map_err(db)?;
        let rows=statement.query_map([run],|r|Ok(serde_json::json!({"id":r.get::<_,i64>(0)?,"seq":r.get::<_,u64>(1)?,"category":r.get::<_,String>(2)?,"tokens":r.get::<_,u64>(3)?,"dollar_micros":r.get::<_,u64>(4)?,"state":r.get::<_,String>(5)?,"admitted_at":r.get::<_,i64>(6)?}))).map_err(db)?;
        let rows = rows.collect::<Result<Vec<_>, _>>().map_err(db)?;
        let tokens: u128 = rows
            .iter()
            .map(|v| u128::from(v["tokens"].as_u64().unwrap()))
            .sum();
        let micros: u128 = rows
            .iter()
            .map(|v| u128::from(v["dollar_micros"].as_u64().unwrap()))
            .sum();
        Ok(
            serde_json::json!({"started_at":clock.map(|v|v.0),"deadline_at":clock.and_then(|v|v.1),"reserved_tokens":u64::try_from(tokens).map_err(|_|"accounting overflow")?,"reserved_dollar_micros":u64::try_from(micros).map_err(|_|"accounting overflow")?,"reservations":rows}),
        )
    }
    pub(crate) fn initialize_resources(
        &mut self,
        run: &str,
        limits: &ResourceLimits,
        now: i64,
        recover: bool,
    ) -> Result<(), String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        tx.execute(
            "INSERT OR IGNORE INTO run_resources(run_id,started_at) VALUES(?1,?2)",
            params![run, now],
        )
        .map_err(db)?;
        if let Some(duration) = limits.elapsed_deadline_ms {
            let start: i64 = tx
                .query_row(
                    "SELECT started_at FROM run_resources WHERE run_id=?1",
                    [run],
                    |r| r.get(0),
                )
                .map_err(db)?;
            let deadline = start
                .checked_add(i64::try_from(duration).map_err(|_| "deadline overflow")?)
                .ok_or("deadline overflow")?;
            tx.execute(
                "UPDATE run_resources SET deadline_at=COALESCE(deadline_at,?2) WHERE run_id=?1",
                params![run, deadline],
            )
            .map_err(db)?;
        }
        if recover {
            tx.execute("UPDATE resource_reservations SET state='interrupted' WHERE run_id=?1 AND state='active'",[run]).map_err(db)?;
        }
        tx.commit().map_err(db)
    }
    pub(crate) fn resource_deadline(&self, run: &str) -> Result<Option<i64>, String> {
        self.connection
            .query_row(
                "SELECT deadline_at FROM run_resources WHERE run_id=?1",
                [run],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)
            .map(Option::flatten)
    }
    pub(crate) fn admit_resource(
        &mut self,
        run: &str,
        seq: u64,
        name: &str,
        args: &Value,
        limits: &ResourceLimits,
        now: i64,
    ) -> Result<Admission, String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let deadline: Option<i64> = tx
            .query_row(
                "SELECT deadline_at FROM run_resources WHERE run_id=?1",
                [run],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?
            .flatten();
        if deadline.is_some_and(|at| now >= at) {
            return Ok(Admission::Denied(
                "BudgetError: elapsed deadline exceeded".into(),
            ));
        }
        let category = if name == "model.call" {
            "model"
        } else if name == "agent.run" {
            "child"
        } else {
            "tool"
        };
        let (concurrency, rate) = if category == "model" {
            (limits.model_concurrency, &limits.model_rate)
        } else {
            (limits.tool_concurrency, &limits.tool_rate)
        };
        let active:u64=tx.query_row("SELECT COUNT(*) FROM resource_reservations WHERE run_id=?1 AND ((?2='model' AND category='model') OR (?2!='model' AND category!='model')) AND state='active'",params![run,category],|r|r.get(0)).map_err(db)?;
        if active >= concurrency as u64 {
            return Ok(Admission::Blocked);
        }
        if let Some(rate) = rate {
            let cutoff = now
                .saturating_sub(i64::try_from(rate.window_ms).map_err(|_| "rate window overflow")?);
            let (count,earliest):(u64,Option<i64>)=tx.query_row("SELECT COUNT(*),MIN(admitted_at) FROM resource_reservations WHERE run_id=?1 AND ((?2='model' AND category='model') OR (?2!='model' AND category!='model')) AND admitted_at>?3",params![run,category,cutoff],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db)?;
            if count >= rate.max_calls {
                return Ok(Admission::Deferred(
                    earliest
                        .unwrap_or(now)
                        .saturating_add(rate.window_ms as i64),
                ));
            }
        }
        let (tokens, micros) = if category == "model" {
            let output = args["request"]["max_tokens"]
                .as_u64()
                .ok_or("model output estimate missing")?;
            let tokens = limits
                .model_input_token_estimate
                .checked_add(output)
                .ok_or("token estimate overflow")?;
            let micros = price(
                limits.model_input_token_estimate,
                limits.input_price_micros_per_million_tokens,
            )?
            .checked_add(price(
                output,
                limits.output_price_micros_per_million_tokens,
            )?)
            .ok_or("price overflow")?;
            (tokens, micros)
        } else if category == "child" {
            (
                args["allocation"]["tokens"]
                    .as_u64()
                    .ok_or("child token allocation missing")?,
                args["allocation"]["dollar_micros"]
                    .as_u64()
                    .ok_or("child dollar allocation missing")?,
            )
        } else {
            (0, limits.tool_estimate_micros)
        };
        let (used_tokens,used_micros):(u64,u64)=tx.query_row("SELECT COALESCE(SUM(tokens),0),COALESCE(SUM(micros),0) FROM resource_reservations WHERE run_id=?1",[run],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db)?;
        if limits
            .token_limit
            .is_some_and(|max| used_tokens.checked_add(tokens).is_none_or(|n| n > max))
        {
            return Ok(Admission::Denied(
                "BudgetError: token reservation exhausted".into(),
            ));
        }
        if limits
            .dollar_limit_micros
            .is_some_and(|max| used_micros.checked_add(micros).is_none_or(|n| n > max))
        {
            return Ok(Admission::Denied(
                "BudgetError: dollar reservation exhausted".into(),
            ));
        }
        if tokens > i64::MAX as u64 || micros > i64::MAX as u64 {
            return Err("reservation exceeds SQLite range".into());
        }
        tx.execute("INSERT INTO resource_reservations(run_id,seq,category,tokens,micros,state,admitted_at,input_price,output_price) VALUES(?1,?2,?3,?4,?5,'active',?6,?7,?8)",params![run,seq,category,tokens,micros,now,limits.input_price_micros_per_million_tokens,limits.output_price_micros_per_million_tokens]).map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(Admission::Ready)
    }
}
pub(crate) fn settle(
    tx: &Transaction<'_>,
    run: &str,
    seq: u64,
    outcome: &str,
) -> Result<(), String> {
    let row:Option<(i64,String,u64,u64,u64,u64)>=tx.query_row("SELECT id,category,tokens,micros,input_price,output_price FROM resource_reservations WHERE run_id=?1 AND seq=?2 AND state='active' ORDER BY id DESC LIMIT 1",params![run,seq],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db)?;
    let Some((id, category, mut tokens, mut micros, input_price, output_price)) = row else {
        return Ok(());
    };
    let value: Value = serde_json::from_str(outcome).map_err(|e| e.to_string())?;
    if category == "model" {
        let usage = &value["Ok"]["usage"];
        if let Some(total) = usage["total_tokens"].as_u64().or_else(|| {
            usage["prompt_tokens"]
                .as_u64()?
                .checked_add(usage["completion_tokens"].as_u64()?)
        }) {
            tokens = total.min(i64::MAX as u64);
        }
        if let (Some(input), Some(output)) = (
            usage["prompt_tokens"].as_u64(),
            usage["completion_tokens"].as_u64(),
        ) {
            micros = price(input, input_price)?
                .checked_add(price(output, output_price)?)
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or("usage price overflow")?;
        }
    }
    if category == "child" && value["Ok"]["status"] == "done" {
        if let (Some(total), Some(spent)) = (
            value["Ok"]["usage"]["total_tokens"].as_u64(),
            value["Ok"]["spent_dollar_micros"].as_u64(),
        ) {
            if total <= i64::MAX as u64 && spent <= i64::MAX as u64 {
                tokens = total;
                micros = spent;
            }
        }
    }
    tx.execute(
        "UPDATE resource_reservations SET state='settled',tokens=?2,micros=?3 WHERE id=?1",
        params![id, tokens, micros],
    )
    .map_err(db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn journal() -> (std::path::PathBuf, Journal) {
        let path = std::env::temp_dir().join(format!("ondure-resources-{}", uuid::Uuid::new_v4()));
        let mut journal = Journal::open(&path).unwrap();
        journal.create_run("run", None, "h", "i", "{}").unwrap();
        (path, journal)
    }
    fn args() -> Value {
        serde_json::json!({"request":{"max_tokens":10}})
    }
    #[test]
    fn full_concurrency_is_untimed_until_a_completion_releases_its_slot() {
        let (path, mut j) = journal();
        let limits = ResourceLimits {
            model_concurrency: 1,
            ..Default::default()
        };
        j.initialize_resources("run", &limits, 0, false).unwrap();
        assert_eq!(
            j.admit_resource("run", 0, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        for now in [100, 110, 100_000] {
            assert_eq!(
                j.admit_resource("run", 1, "model.call", &args(), &limits, now)
                    .unwrap(),
                Admission::Blocked
            );
        }
        assert_eq!(
            j.connection
                .query_row("SELECT COUNT(*) FROM resource_reservations", [], |row| row
                    .get::<_, u64>(
                    0
                ))
                .unwrap(),
            1
        );
        let tx = j.connection.transaction().unwrap();
        settle(&tx, "run", 0, r#"{"Ok":{"usage":null}}"#).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            j.admit_resource("run", 1, "model.call", &args(), &limits, 100_000)
                .unwrap(),
            Admission::Ready
        );
        drop(j);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_competing_reservations_cannot_overspend() {
        let (path, mut j) = journal();
        let limits = ResourceLimits {
            token_limit: Some(20),
            model_input_token_estimate: 10,
            ..Default::default()
        };
        j.initialize_resources("run", &limits, 0, false).unwrap();
        drop(j);
        let handles: Vec<_> = (0..2)
            .map(|seq| {
                let path = path.clone();
                let limits = limits.clone();
                std::thread::spawn(move || {
                    Journal::open(&path)
                        .unwrap()
                        .admit_resource("run", seq, "model.call", &args(), &limits, 100)
                        .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| **r == Admission::Ready).count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Admission::Denied(_)))
                .count(),
            1
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn restart_releases_slots_but_retains_unknown_reservations() {
        let (path, mut j) = journal();
        let limits = ResourceLimits {
            token_limit: Some(40),
            model_concurrency: 1,
            model_input_token_estimate: 10,
            ..Default::default()
        };
        j.initialize_resources("run", &limits, 0, false).unwrap();
        assert_eq!(
            j.admit_resource("run", 0, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        assert!(matches!(
            j.admit_resource("run", 1, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Blocked
        ));
        drop(j);
        let mut j = Journal::open(&path).unwrap();
        j.initialize_resources("run", &limits, 100, true).unwrap();
        assert_eq!(
            j.admit_resource("run", 0, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        let tx = j.connection.transaction().unwrap();
        settle(&tx, "run", 0, r#"{"Ok":{"usage":null}}"#).unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            j.admit_resource("run", 1, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Denied(_)
        ));
        drop(j);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn known_usage_reconciles_prices_and_rate_window_includes_restart() {
        let (path, mut j) = journal();
        let limits = ResourceLimits {
            model_input_token_estimate: 10,
            input_price_micros_per_million_tokens: 1_000_000,
            output_price_micros_per_million_tokens: 2_000_000,
            model_rate: Some(RateLimit {
                max_calls: 1,
                window_ms: 100,
            }),
            ..Default::default()
        };
        j.initialize_resources("run", &limits, 0, false).unwrap();
        assert_eq!(
            j.admit_resource("run", 0, "model.call", &args(), &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        let tx = j.connection.transaction().unwrap();
        settle(
            &tx,
            "run",
            0,
            r#"{"Ok":{"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}}"#,
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            j.connection
                .query_row("SELECT tokens,micros FROM resource_reservations", [], |r| {
                    Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
                })
                .unwrap(),
            (5, 8)
        );
        drop(j);
        let mut j = Journal::open(&path).unwrap();
        j.initialize_resources("run", &limits, 150, true).unwrap();
        assert_eq!(
            j.admit_resource("run", 1, "model.call", &args(), &limits, 150)
                .unwrap(),
            Admission::Deferred(200)
        );
        assert_eq!(
            j.admit_resource("run", 1, "model.call", &args(), &limits, 200)
                .unwrap(),
            Admission::Ready
        );
        drop(j);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn child_allocation_reserves_parent_budget_and_reconciles_completed_child() {
        let (path, mut journal) = journal();
        let limits = ResourceLimits {
            token_limit: Some(100),
            dollar_limit_micros: Some(50),
            tool_concurrency: 1,
            ..Default::default()
        };
        journal
            .initialize_resources("run", &limits, 0, false)
            .unwrap();
        let child = serde_json::json!({"allocation":{"tokens":80,"dollar_micros":40}});
        assert_eq!(
            journal
                .admit_resource("run", 0, "agent.run", &child, &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        assert!(matches!(
            journal
                .admit_resource("run", 1, "fetch.get", &Value::Null, &limits, 100)
                .unwrap(),
            Admission::Blocked
        ));
        let tx = journal.connection.transaction().unwrap();
        settle(
            &tx,
            "run",
            0,
            r#"{"Ok":{"status":"done","usage":{"total_tokens":5},"spent_dollar_micros":3}}"#,
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            journal.resource_report("run").unwrap()["reserved_tokens"],
            5
        );
        assert_eq!(
            journal
                .admit_resource("run", 1, "agent.run", &child, &limits, 100)
                .unwrap(),
            Admission::Ready
        );
        let tx = journal.connection.transaction().unwrap();
        settle(&tx, "run", 1, r#"{"Ok":{"status":"parked"}}"#).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            journal.resource_report("run").unwrap()["reserved_tokens"],
            85
        );
        assert!(matches!(
            journal
                .admit_resource("run", 2, "agent.run", &child, &limits, 100)
                .unwrap(),
            Admission::Denied(_)
        ));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn reconciliation_and_completed_outcome_commit_together() {
        let (path, mut journal) = journal();
        let limits = ResourceLimits {
            model_input_token_estimate: 10,
            ..Default::default()
        };
        journal
            .initialize_resources("run", &limits, 0, false)
            .unwrap();
        journal
            .intents(
                "run",
                &[super::super::journal::OpIntent {
                    seq: 0,
                    name: "model.call".into(),
                    args_digest: "digest".into(),
                    args_json: None,
                    policy: "never".into(),
                }],
            )
            .unwrap();
        journal
            .admit_resource("run", 0, "model.call", &args(), &limits, 100)
            .unwrap();
        journal.connection.execute_batch("CREATE TRIGGER fail_outcome BEFORE UPDATE ON operations BEGIN SELECT RAISE(ABORT,'injected');END;").unwrap();
        assert!(journal
            .complete(
                "run",
                0,
                r#"{"Ok":{"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}}"#
            )
            .is_err());
        let report = journal.resource_report("run").unwrap();
        assert_eq!(report["reserved_tokens"], 20);
        assert_eq!(report["reservations"][0]["state"], "active");
        assert_eq!(journal.operations("run").unwrap()[0].state, "pending");
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn schema_three_migration_preserves_history() {
        let (path, mut journal) = journal();
        journal
            .intents(
                "run",
                &[super::super::journal::OpIntent {
                    seq: 0,
                    name: "clock.date".into(),
                    args_digest: "digest".into(),
                    args_json: None,
                    policy: "safe".into(),
                }],
            )
            .unwrap();
        journal.complete("run", 0, "42").unwrap();
        let before = journal.operations("run").unwrap();
        journal
            .connection
            .execute_batch(
                "DROP TABLE resource_reservations;DROP TABLE run_resources;PRAGMA user_version=3;",
            )
            .unwrap();
        drop(journal);
        let journal = Journal::open(&path).unwrap();
        assert_eq!(journal.operations("run").unwrap(), before);
        assert_eq!(
            journal.resource_report("run").unwrap()["reserved_tokens"],
            0
        );
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn dollar_admission_and_deadline_are_independent() {
        let (path, mut j) = journal();
        let start: i64 = j
            .connection
            .query_row(
                "SELECT started_at FROM run_resources WHERE run_id='run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let limits = ResourceLimits {
            dollar_limit_micros: Some(2),
            tool_estimate_micros: 3,
            elapsed_deadline_ms: Some(5),
            ..Default::default()
        };
        j.initialize_resources("run", &limits, start, false)
            .unwrap();
        assert!(
            matches!(j.admit_resource("run",0,"fetch.get",&Value::Null,&limits,start).unwrap(),Admission::Denied(e) if e.contains("dollar"))
        );
        assert!(
            matches!(j.admit_resource("run",1,"fetch.get",&Value::Null,&limits,start+6).unwrap(),Admission::Denied(e) if e.contains("deadline"))
        );
        drop(j);
        std::fs::remove_file(path).unwrap();
    }
}
