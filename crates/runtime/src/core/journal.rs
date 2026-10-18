//! Durable run and operation records. Each mutation commits before returning.

use std::{fmt, path::Path, time::Duration};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct JournalError(String);

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for JournalError {}

impl From<rusqlite::Error> for JournalError {
    fn from(error: rusqlite::Error) -> Self {
        Self(format!("journal: {error}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub request_id: Option<String>,
    pub code_hash: String,
    pub input_digest: String,
    pub graph: String,
    pub status: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpIntent {
    pub seq: u64,
    pub name: String,
    pub args_digest: String,
    pub args_json: Option<String>,
    pub policy: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpRecord {
    pub seq: u64,
    pub name: String,
    pub args_digest: String,
    pub args_json: Option<String>,
    pub policy: String,
    pub state: String,
    pub result: Option<String>,
    pub completion_order: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRecord {
    pub run_id: String,
    pub seq: u64,
    pub kind: String,
    pub wake_at: Option<i64>,
    pub answer: Option<String>,
    pub details: String,
}

/// A dispatch boundary committed before invoking an external adapter.
/// An interrupted attempt may have produced external effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub seq: u64,
    pub attempt: u64,
    pub state: String,
    pub outcome: Option<String>,
}

pub struct Journal {
    connection: Connection,
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > 2 {
            return Err(JournalError(format!(
                "journal: unsupported schema version {version}"
            )));
        }
        if version < 1 {
            transaction.execute_batch(
            "
             CREATE TABLE IF NOT EXISTS runs (
                 id TEXT PRIMARY KEY NOT NULL,
                 request_id TEXT UNIQUE,
                 code_hash TEXT NOT NULL,
                 input_digest TEXT NOT NULL,
                 graph TEXT NOT NULL,
                 status TEXT NOT NULL,
                 stdout TEXT NOT NULL DEFAULT '',
                 stderr TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS operations (
                 run_id TEXT NOT NULL REFERENCES runs(id),
                 seq INTEGER NOT NULL CHECK(seq >= 0),
                 name TEXT NOT NULL,
                 args_digest TEXT NOT NULL,
                 policy TEXT NOT NULL,
                 state TEXT NOT NULL CHECK(state IN ('pending', 'completed')),
                 result TEXT,
                 completion_order INTEGER CHECK(completion_order >= 0),
                 PRIMARY KEY(run_id, seq),
                 UNIQUE(run_id, completion_order),
                 CHECK((state = 'pending' AND result IS NULL AND completion_order IS NULL)
                    OR (state = 'completed' AND result IS NOT NULL AND completion_order IS NOT NULL))
             );
             CREATE TABLE IF NOT EXISTS waits (
                 run_id TEXT NOT NULL,
                 seq INTEGER NOT NULL CHECK(seq >= 0),
                 kind TEXT NOT NULL,
                 wake_at INTEGER,
                 answer TEXT,
                 details TEXT NOT NULL,
                 PRIMARY KEY(run_id, seq),
                 FOREIGN KEY(run_id, seq) REFERENCES operations(run_id, seq)
             );
             ",
        )?;
            transaction.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            transaction.execute_batch(
                "ALTER TABLE operations ADD COLUMN args_json TEXT;
                 CREATE TABLE operation_attempts (
                    run_id TEXT NOT NULL,
                    seq INTEGER NOT NULL,
                    attempt INTEGER NOT NULL CHECK(attempt >= 1),
                    state TEXT NOT NULL CHECK(state IN ('dispatched', 'interrupted', 'completed')),
                    outcome TEXT,
                    PRIMARY KEY(run_id, seq, attempt),
                    FOREIGN KEY(run_id, seq) REFERENCES operations(run_id, seq),
                    CHECK((state = 'completed' AND outcome IS NOT NULL)
                       OR (state != 'completed' AND outcome IS NULL))
                );",
            )?;
            transaction.pragma_update(None, "user_version", 2)?;
        }
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// Commit an attempt before external dispatch. A prior unfinished attempt is
    /// ambiguous, even when the process died before making the actual request.
    /// The caller must enforce the operation's recovery policy before retrying.
    pub fn begin_attempt(&mut self, run_id: &str, seq: u64) -> Result<u64, JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq = sequence(seq)?;
        let state: String = transaction.query_row(
            "SELECT state FROM operations WHERE run_id = ?1 AND seq = ?2",
            params![run_id, seq],
            |row| row.get(0),
        )?;
        if state != "pending" {
            return Err(JournalError(
                "journal: cannot dispatch completed operation".into(),
            ));
        }
        transaction.execute(
            "UPDATE operation_attempts SET state = 'interrupted' WHERE run_id = ?1 AND seq = ?2 AND state = 'dispatched'",
            params![run_id, seq])?;
        let attempt: u64 = transaction.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM operation_attempts WHERE run_id = ?1 AND seq = ?2",
            params![run_id, seq], |row| row.get(0))?;
        transaction.execute(
            "INSERT INTO operation_attempts (run_id, seq, attempt, state) VALUES (?1, ?2, ?3, 'dispatched')",
            params![run_id, seq, sequence(attempt)?])?;
        transaction.commit()?;
        Ok(attempt)
    }

    pub fn attempts(&self, run_id: &str) -> Result<Vec<AttemptRecord>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT seq, attempt, state, outcome FROM operation_attempts WHERE run_id = ?1 ORDER BY seq, attempt")?;
        let records = statement.query_map([run_id], |row| {
            Ok(AttemptRecord {
                seq: row.get(0)?,
                attempt: row.get(1)?,
                state: row.get(2)?,
                outcome: row.get(3)?,
            })
        })?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn create_run(
        &mut self,
        id: &str,
        request_id: Option<&str>,
        code_hash: &str,
        input_digest: &str,
        graph: &str,
    ) -> Result<RunRecord, JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO runs (id, request_id, code_hash, input_digest, graph, status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running')",
            params![id, request_id, code_hash, input_digest, graph],
        )?;
        let record = transaction.query_row(
            "SELECT id, request_id, code_hash, input_digest, graph, status, stdout, stderr
             FROM runs WHERE id = ?1",
            [id],
            read_run,
        )?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn get_run(&self, id: &str) -> Result<RunRecord, JournalError> {
        self.connection
            .query_row(
                "SELECT id, request_id, code_hash, input_digest, graph, status, stdout, stderr
                 FROM runs WHERE id = ?1",
                [id],
                read_run,
            )
            .map_err(Into::into)
    }

    pub fn find_request(&self, request_id: &str) -> Result<Option<RunRecord>, JournalError> {
        self.connection
            .query_row(
                "SELECT id, request_id, code_hash, input_digest, graph, status, stdout, stderr
                 FROM runs WHERE request_id = ?1",
                [request_id],
                read_run,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn parked_runs(&self) -> Result<Vec<RunRecord>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, request_id, code_hash, input_digest, graph, status, stdout, stderr
             FROM runs WHERE status = 'parked' ORDER BY id",
        )?;
        let records = statement.query_map([], read_run)?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub(crate) fn recoverable_runs(&self) -> Result<Vec<RunRecord>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT id, request_id, code_hash, input_digest, graph, status, stdout, stderr
             FROM runs WHERE status IN ('parked', 'running') ORDER BY id",
        )?;
        let records = statement.query_map([], read_run)?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn set_status(&mut self, id: &str, status: &str) -> Result<(), JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_one(transaction.execute(
            "UPDATE runs SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn finish_run(
        &mut self,
        id: &str,
        status: &str,
        stdout: &str,
        stderr: &str,
    ) -> Result<(), JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_one(transaction.execute(
            "UPDATE runs SET status = ?2, stdout = ?3, stderr = ?4 WHERE id = ?1",
            params![id, status, stdout, stderr],
        )?)?;
        transaction.commit()?;
        Ok(())
    }

    /// Commit all intents together, before any corresponding operation is dispatched.
    pub fn intents(&mut self, run_id: &str, intents: &[OpIntent]) -> Result<(), JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_intents(&transaction, run_id, intents)?;
        transaction.commit()?;
        Ok(())
    }

    /// Store a result once; retries may repeat the exact same serialized result.
    pub fn complete(&mut self, run_id: &str, seq: u64, result: &str) -> Result<(), JournalError> {
        self.complete_batch(run_id, &[(seq, result.to_owned())])
    }

    /// Commit a completion batch in delivery order, with no partial writes on failure.
    pub fn complete_batch(
        &mut self,
        run_id: &str,
        results: &[(u64, String)],
    ) -> Result<(), JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_completions(&transaction, run_id, results)?;
        transaction.commit()?;
        Ok(())
    }

    /// Atomically commit safe synchronous intents and their observed result.
    /// No value is exposed to JavaScript before this transaction commits.
    pub(crate) fn complete_sync(
        &mut self,
        run_id: &str,
        intents: &[OpIntent],
        seq: u64,
        result: &str,
    ) -> Result<(), JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_intents(&transaction, run_id, intents)?;
        write_completions(&transaction, run_id, &[(seq, result.to_owned())])?;
        transaction.commit()?;
        Ok(())
    }

    /// Persist a wait. Only finalization publishes a quiescent parked run;
    /// JavaScript may still have work to register after creating this wait.
    pub fn park_wait(
        &mut self,
        run_id: &str,
        seq: u64,
        kind: &str,
        wake_at: Option<i64>,
        details: &str,
    ) -> Result<WaitRecord, JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored_seq = sequence(seq)?;
        let state: String = transaction.query_row(
            "SELECT state FROM operations WHERE run_id = ?1 AND seq = ?2",
            params![run_id, stored_seq],
            |row| row.get(0),
        )?;
        if state != "pending" {
            return Err(JournalError(format!(
                "journal: cannot park completed operation {seq}"
            )));
        }
        let existing = transaction.query_row(
            "SELECT run_id, seq, kind, wake_at, answer, details FROM waits WHERE run_id = ?1 AND seq = ?2",
            params![run_id, stored_seq],
            read_wait,
        ).optional()?;
        let record = if let Some(existing) = existing {
            if existing.kind != kind || existing.wake_at != wake_at || existing.details != details {
                return Err(JournalError(format!(
                    "journal: wait mismatch at operation {seq}"
                )));
            }
            existing
        } else {
            transaction.execute(
                "INSERT INTO waits (run_id, seq, kind, wake_at, details) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, stored_seq, kind, wake_at, details],
            )?;
            WaitRecord {
                run_id: run_id.into(),
                seq,
                kind: kind.into(),
                wake_at,
                answer: None,
                details: details.into(),
            }
        };
        transaction.commit()?;
        Ok(record)
    }

    pub fn waits(&self, run_id: &str) -> Result<Vec<WaitRecord>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT run_id, seq, kind, wake_at, answer, details FROM waits WHERE run_id = ?1 ORDER BY seq",
        )?;
        let records = statement.query_map([run_id], read_wait)?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Record the first answer. Repeating it is harmless; replacing it is rejected.
    pub fn answer_wait(
        &mut self,
        run_id: &str,
        seq: u64,
        answer: &str,
    ) -> Result<bool, JournalError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq = sequence(seq)?;
        let existing: Option<String> = transaction.query_row(
            "SELECT answer FROM waits WHERE run_id = ?1 AND seq = ?2",
            params![run_id, seq],
            |row| row.get(0),
        )?;
        let changed = if let Some(existing) = existing {
            if existing != answer {
                return Err(JournalError(format!(
                    "journal: conflicting answer at operation {seq}"
                )));
            }
            false
        } else {
            transaction.execute(
                "UPDATE waits SET answer = ?3 WHERE run_id = ?1 AND seq = ?2",
                params![run_id, seq, answer],
            )?;
            true
        };
        transaction.commit()?;
        Ok(changed)
    }

    pub fn operations(&self, run_id: &str) -> Result<Vec<OpRecord>, JournalError> {
        let mut statement = self.connection.prepare(
            "SELECT seq, name, args_digest, policy, state, result, completion_order, args_json
             FROM operations WHERE run_id = ?1 ORDER BY seq",
        )?;
        let records = statement.query_map([run_id], |row| {
            Ok(OpRecord {
                seq: row.get(0)?,
                name: row.get(1)?,
                args_digest: row.get(2)?,
                args_json: row.get(7)?,
                policy: row.get(3)?,
                state: row.get(4)?,
                result: row.get(5)?,
                completion_order: row.get(6)?,
            })
        })?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn read_wait(row: &rusqlite::Row<'_>) -> rusqlite::Result<WaitRecord> {
    Ok(WaitRecord {
        run_id: row.get(0)?,
        seq: row.get(1)?,
        kind: row.get(2)?,
        wake_at: row.get(3)?,
        answer: row.get(4)?,
        details: row.get(5)?,
    })
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: row.get(0)?,
        request_id: row.get(1)?,
        code_hash: row.get(2)?,
        input_digest: row.get(3)?,
        graph: row.get(4)?,
        status: row.get(5)?,
        stdout: row.get(6)?,
        stderr: row.get(7)?,
    })
}

fn sequence(seq: u64) -> Result<i64, JournalError> {
    i64::try_from(seq).map_err(|_| {
        JournalError("journal: operation sequence exceeds SQLite integer range".into())
    })
}

fn require_one(changed: usize) -> Result<(), JournalError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(JournalError("journal: run not found".into()))
    }
}

fn write_intents(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    intents: &[OpIntent],
) -> Result<(), JournalError> {
    for intent in intents {
        let seq = sequence(intent.seq)?;
        let existing = transaction
            .query_row(
                "SELECT name, args_digest, policy, args_json FROM operations WHERE run_id = ?1 AND seq = ?2",
                params![run_id, seq],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;
        if let Some((name, args_digest, policy, args_json)) = existing {
            if name != intent.name
                || args_digest != intent.args_digest
                || policy != intent.policy
                || args_json
                    .as_ref()
                    .is_some_and(|args| Some(args) != intent.args_json.as_ref())
            {
                return Err(JournalError(format!(
                    "journal: intent mismatch at operation {}",
                    intent.seq
                )));
            }
            continue;
        }
        transaction.execute(
            "INSERT INTO operations (run_id, seq, name, args_digest, policy, state, args_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            params![
                run_id,
                seq,
                intent.name,
                intent.args_digest,
                intent.policy,
                intent.args_json
            ],
        )?;
    }
    Ok(())
}

fn write_completions(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    results: &[(u64, String)],
) -> Result<(), JournalError> {
    for (seq, result) in results {
        let seq = sequence(*seq)?;
        let existing: Option<String> = transaction.query_row(
            "SELECT result FROM operations WHERE run_id = ?1 AND seq = ?2",
            params![run_id, seq],
            |row| row.get(0),
        )?;
        if let Some(existing) = existing {
            if existing != *result {
                return Err(JournalError(format!(
                    "journal: result mismatch at operation {seq}"
                )));
            }
        } else {
            transaction.execute(
                "UPDATE operation_attempts SET state = 'completed', outcome = ?3
                 WHERE run_id = ?1 AND seq = ?2 AND state = 'dispatched'",
                params![run_id, seq, result],
            )?;
            transaction.execute(
                "UPDATE operations SET state = 'completed', result = ?3,
                         completion_order = (SELECT COALESCE(MAX(completion_order), -1) + 1
                                             FROM operations WHERE run_id = ?1)
                     WHERE run_id = ?1 AND seq = ?2",
                params![run_id, seq, result],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ondure-journal-{}.sqlite", uuid::Uuid::new_v4()))
    }

    fn intent(seq: u64) -> OpIntent {
        OpIntent {
            seq,
            name: "fetch".into(),
            args_digest: format!("args-{seq}"),
            args_json: None,
            policy: "net".into(),
        }
    }

    fn create(journal: &mut Journal) {
        journal
            .create_run("run", Some("request"), "hash", "input", "graph")
            .unwrap();
    }

    #[test]
    fn migrates_unversioned_journals_without_replacing_history() {
        let path = temporary_path();
        {
            let mut journal = Journal::open(&path).unwrap();
            create(&mut journal);
            journal.intents("run", &[intent(0), intent(1)]).unwrap();
            journal.complete("run", 0, "original").unwrap();
            journal
                .park_wait("run", 1, "approval", None, "details")
                .unwrap();
            journal.connection.execute_batch(
                "DROP TABLE operation_attempts; ALTER TABLE operations DROP COLUMN args_json; PRAGMA user_version = 0;"
            ).unwrap();
        }
        let journal = Journal::open(&path).unwrap();
        assert_eq!(
            journal.get_run("run").unwrap().request_id.as_deref(),
            Some("request")
        );
        let operations = journal.operations("run").unwrap();
        assert_eq!(operations[0].result.as_deref(), Some("original"));
        assert_eq!(operations[0].args_json, None);
        let waits = journal.waits("run").unwrap();
        assert_eq!(waits.len(), 1);
        assert_eq!(waits[0].details, "details");
        assert_eq!(operations[1].state, "pending");
        assert!(journal.attempts("run").unwrap().is_empty());
        assert_eq!(
            journal
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            2
        );
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn refuses_future_schema_without_changing_it() {
        let path = temporary_path();
        let connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        drop(connection);
        assert!(Journal::open(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("unsupported schema version 99"));
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            99
        );
        drop(connection);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn dispatch_attempts_survive_restart_and_complete_with_outcomes() {
        let path = temporary_path();
        {
            let mut journal = Journal::open(&path).unwrap();
            create(&mut journal);
            journal.intents("run", &[intent(0)]).unwrap();
            assert_eq!(journal.begin_attempt("run", 0).unwrap(), 1);
        }
        let mut journal = Journal::open(&path).unwrap();
        assert_eq!(journal.attempts("run").unwrap()[0].state, "dispatched");
        assert_eq!(journal.begin_attempt("run", 0).unwrap(), 2);
        journal.complete("run", 0, "result").unwrap();
        let attempts = journal.attempts("run").unwrap();
        assert_eq!(attempts[0].state, "interrupted");
        assert_eq!(attempts[1].state, "completed");
        assert_eq!(attempts[1].outcome.as_deref(), Some("result"));
        assert!(journal.begin_attempt("run", 0).is_err());
        journal.complete("run", 0, "result").unwrap();
        assert_eq!(journal.attempts("run").unwrap(), attempts);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn outcome_commit_failure_preserves_ambiguous_attempt() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        journal.intents("run", &[intent(0)]).unwrap();
        journal.begin_attempt("run", 0).unwrap();
        journal.connection.execute_batch(
            "CREATE TRIGGER fail_outcome BEFORE UPDATE ON operations BEGIN SELECT RAISE(ABORT, 'injected'); END;"
        ).unwrap();
        assert!(journal.complete("run", 0, "result").is_err());
        assert_eq!(journal.operations("run").unwrap()[0].state, "pending");
        assert_eq!(journal.attempts("run").unwrap()[0].state, "dispatched");
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn synchronous_result_and_intents_commit_or_roll_back_together() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        journal
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_completion BEFORE UPDATE ON operations
             BEGIN SELECT RAISE(ABORT, 'injected completion failure'); END;",
            )
            .unwrap();
        assert!(journal
            .complete_sync("run", &[intent(0), intent(1)], 1, "value")
            .is_err());
        assert!(journal.operations("run").unwrap().is_empty());
        journal
            .connection
            .execute_batch("DROP TRIGGER fail_completion")
            .unwrap();
        journal
            .complete_sync("run", &[intent(0), intent(1)], 1, "value")
            .unwrap();
        let operations = Journal::open(&path).unwrap().operations("run").unwrap();
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].state, "pending");
        assert_eq!(operations[1].state, "completed");
        assert_eq!(operations[1].result.as_deref(), Some("value"));
        assert_eq!(operations[1].completion_order, Some(0));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn records_survive_reopening_and_use_durable_pragmas() {
        let path = temporary_path();
        {
            let mut journal = Journal::open(&path).unwrap();
            create(&mut journal);
            journal.intents("run", &[intent(0), intent(1)]).unwrap();
            journal.complete("run", 1, "second").unwrap();
            journal.complete("run", 0, "first").unwrap();
            journal
                .finish_run("run", "completed", "out", "err")
                .unwrap();
            assert_eq!(
                journal
                    .connection
                    .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                    .unwrap(),
                "wal"
            );
            assert_eq!(
                journal
                    .connection
                    .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        let journal = Journal::open(&path).unwrap();
        let run = journal.get_run("run").unwrap();
        assert_eq!(run.status, "completed");
        assert_eq!((run.stdout.as_str(), run.stderr.as_str()), ("out", "err"));
        assert_eq!(journal.find_request("request").unwrap(), Some(run));
        assert_eq!(journal.find_request("absent").unwrap(), None);
        let operations = journal.operations("run").unwrap();
        assert_eq!(operations[0].completion_order, Some(1));
        assert_eq!(operations[1].completion_order, Some(0));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn intent_batches_rollback_on_a_mismatch() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        journal.intents("run", &[intent(0)]).unwrap();
        let mut changed = intent(0);
        changed.args_digest = "different".into();
        assert!(journal.intents("run", &[intent(1), changed]).is_err());
        let operations = journal.operations("run").unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].state, "pending");
        assert_eq!(operations[0].result, None);
        assert!(journal.complete("run", 1, "no-intent").is_err());
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn completed_results_are_immutable_and_retries_do_not_reorder() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        journal.intents("run", &[intent(0), intent(1)]).unwrap();
        journal.complete("run", 0, "original").unwrap();
        journal.complete("run", 1, "next").unwrap();
        journal.complete("run", 0, "original").unwrap();
        journal.intents("run", &[intent(0)]).unwrap();
        assert!(journal.complete("run", 0, "replacement").is_err());
        let operations = journal.operations("run").unwrap();
        assert_eq!(operations[0].result.as_deref(), Some("original"));
        assert_eq!(operations[0].completion_order, Some(0));
        assert_eq!(operations[1].completion_order, Some(1));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn result_batches_commit_together_in_delivery_order() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        journal
            .intents("run", &[intent(0), intent(1), intent(2)])
            .unwrap();
        assert!(journal
            .complete_batch("run", &[(1, "one".into()), (99, "missing".into())])
            .is_err());
        assert!(journal
            .operations("run")
            .unwrap()
            .iter()
            .all(|op| op.result.is_none()));
        journal
            .complete_batch(
                "run",
                &[(2, "two".into()), (0, "zero".into()), (1, "one".into())],
            )
            .unwrap();
        let operations = journal.operations("run").unwrap();
        assert_eq!(
            operations
                .iter()
                .map(|op| op.completion_order)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(0)]
        );
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn parked_waits_and_first_answers_survive_restart() {
        let path = temporary_path();
        {
            let mut journal = Journal::open(&path).unwrap();
            create(&mut journal);
            journal.intents("run", &[intent(0), intent(1)]).unwrap();
            let approval = journal
                .park_wait("run", 0, "approval", None, "send email")
                .unwrap();
            assert_eq!(
                journal
                    .park_wait("run", 0, "approval", None, "send email")
                    .unwrap(),
                approval
            );
            journal
                .park_wait("run", 1, "timer", Some(1234), "pause")
                .unwrap();
            assert_eq!(journal.get_run("run").unwrap().status, "running");
            journal.finish_run("run", "parked", "", "").unwrap();
            assert!(journal.answer_wait("run", 0, "yes").unwrap());
        }
        let mut journal = Journal::open(&path).unwrap();
        assert!(!journal.answer_wait("run", 0, "yes").unwrap());
        assert!(journal.answer_wait("run", 0, "no").is_err());
        let waits = journal.waits("run").unwrap();
        assert_eq!(waits.len(), 2);
        assert_eq!(waits[0].answer.as_deref(), Some("yes"));
        assert_eq!(waits[0].details, "send email");
        assert!(journal
            .park_wait("run", 0, "approval", None, "delete file")
            .is_err());
        assert_eq!(journal.parked_runs().unwrap().len(), 1);
        journal.set_status("run", "running").unwrap();
        assert!(journal.parked_runs().unwrap().is_empty());
        assert_eq!(waits[1].wake_at, Some(1234));
        assert_eq!(waits[1].answer, None);
        assert!(journal
            .park_wait("run", 1, "timer", Some(5678), "pause")
            .is_err());
        assert_eq!(journal.waits("run").unwrap()[1].wake_at, Some(1234));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn only_pending_operations_can_park() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        assert!(journal
            .park_wait("run", 0, "approval", None, "send email")
            .is_err());
        assert_eq!(journal.get_run("run").unwrap().status, "running");
        journal.intents("run", &[intent(0)]).unwrap();
        journal.complete("run", 0, "done").unwrap();
        assert!(journal
            .park_wait("run", 0, "approval", None, "send email")
            .is_err());
        assert!(journal.waits("run").unwrap().is_empty());
        assert!(journal.answer_wait("run", 0, "missing").is_err());
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn request_ids_are_unique_and_unknown_runs_cannot_be_updated() {
        let path = temporary_path();
        let mut journal = Journal::open(&path).unwrap();
        create(&mut journal);
        assert!(journal
            .create_run("other", Some("request"), "hash", "input", "graph")
            .is_err());
        assert!(journal.get_run("other").is_err());
        assert!(journal.set_status("missing", "completed").is_err());
        assert!(journal.intents("missing", &[intent(0)]).is_err());
        journal.set_status("run", "interrupted").unwrap();
        assert_eq!(journal.get_run("run").unwrap().status, "interrupted");
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
}
