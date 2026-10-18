//! SQLite conversation context, lexical retrieval, and versioned shared state.
//! All reads and writes execute inside the operation-outcome transaction.
use crate::MAX_OPERATION_RESULT_BYTES;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const MAX_CONTEXT_MESSAGES: usize = 10_000;
const MAX_CONTEXT_BYTES: usize = MAX_OPERATION_RESULT_BYTES / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextLimits {
    pub max_messages: usize,
    pub max_bytes: usize,
    pub recent_messages: usize,
}
impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_messages: 128,
            max_bytes: 256 * 1024,
            recent_messages: 16,
        }
    }
}

impl ContextLimits {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_messages == 0 || self.max_bytes == 0 || self.recent_messages == 0 {
            return Err("context limits must be positive".into());
        }
        // Bytes count serialized messages. The remaining half of the outcome
        // limit covers array separators, the result envelope and archive count.
        // Even 10,000 maximal SQLite i64 archive IDs encode below 210 KiB;
        // those are stored separately from the returned context.
        if self.max_messages > MAX_CONTEXT_MESSAGES || self.max_bytes > MAX_CONTEXT_BYTES {
            return Err(format!("context limits cannot exceed {MAX_CONTEXT_MESSAGES} messages or {MAX_CONTEXT_BYTES} bytes"));
        }
        if self.recent_messages > self.max_messages {
            return Err("recent_messages cannot exceed max_messages".into());
        }
        Ok(())
    }
}

pub(crate) const SCHEMA: &str = "
CREATE TABLE memory_facts (id INTEGER PRIMARY KEY, scope TEXT NOT NULL, text TEXT NOT NULL, metadata TEXT NOT NULL);
CREATE VIRTUAL TABLE memory_search USING fts5(text, content='memory_facts', content_rowid='id');
CREATE TRIGGER memory_insert AFTER INSERT ON memory_facts BEGIN INSERT INTO memory_search(rowid,text) VALUES(new.id,new.text); END;
CREATE TABLE shared_state (scope TEXT NOT NULL, key TEXT NOT NULL, version INTEGER NOT NULL, value TEXT NOT NULL, PRIMARY KEY(scope,key));
CREATE TABLE context_messages (run_id TEXT NOT NULL REFERENCES runs(id), id INTEGER PRIMARY KEY, message TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
CREATE TABLE context_compactions (run_id TEXT NOT NULL REFERENCES runs(id), seq INTEGER NOT NULL, archived_ids TEXT NOT NULL, PRIMARY KEY(run_id,seq));
";
fn db(error: rusqlite::Error) -> String {
    format!("memory: {error}")
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}
fn scope(args: &Value) -> Result<&str, String> {
    let value = args.get("scope").map_or(Ok("default"), |v| {
        v.as_str().ok_or("scope must be a string")
    })?;
    if value.is_empty() || value.len() > 256 {
        return Err("invalid memory scope".into());
    }
    Ok(value)
}
pub(crate) fn execute(
    tx: &Transaction<'_>,
    run: &str,
    seq: u64,
    name: &str,
    args: &Value,
    limits: ContextLimits,
) -> Result<Value, String> {
    match name {
        "context.add" => {
            let message = args.get("message").ok_or("message required")?;
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .ok_or("message.role required")?;
            if !["system", "user", "assistant", "tool"].contains(&role)
                || !message.get("content").is_some_and(Value::is_string)
            {
                return Err("context message requires a supported role and string content".into());
            }
            tx.execute(
                "INSERT INTO context_messages(run_id,message) VALUES(?1,?2)",
                params![run, message.to_string()],
            )
            .map_err(db)?;
            let messages = messages(tx, run)?;
            let mut archived = Vec::new();
            let mut bytes: usize = messages.iter().map(|(_, v)| v.to_string().len()).sum();
            let mut count = messages.len();
            let protected_start = messages.len().saturating_sub(limits.recent_messages.max(1));
            for (index, (id, value)) in messages.iter().enumerate() {
                if count <= limits.max_messages && bytes <= limits.max_bytes {
                    break;
                }
                if value["role"] == "system" || index >= protected_start {
                    continue;
                }
                tx.execute("UPDATE context_messages SET archived=1 WHERE id=?1", [id])
                    .map_err(db)?;
                archived.push(*id);
                count -= 1;
                bytes -= value.to_string().len();
            }
            if count > limits.max_messages || bytes > limits.max_bytes {
                return Err("protected context exceeds configured limits".into());
            }
            if !archived.is_empty() {
                tx.execute(
                    "INSERT INTO context_compactions(run_id,seq,archived_ids) VALUES(?1,?2,?3)",
                    params![run, seq, serde_json::to_string(&archived).unwrap()],
                )
                .map_err(db)?;
            }
            Ok(
                json!({"messages":messages.into_iter().filter(|(id,_)|!archived.contains(id)).map(|(_,v)|v).collect::<Vec<_>>(),"archived":archived.len()}),
            )
        }
        "context.messages" => Ok(json!(messages(tx, run)?
            .into_iter()
            .map(|(_, v)| v)
            .collect::<Vec<_>>())),
        "memory.remember" => {
            let scope = scope(args)?;
            let value = text(args, "text")?;
            if value.is_empty() {
                return Err("memory text must not be empty".into());
            }
            tx.execute(
                "INSERT INTO memory_facts(scope,text,metadata) VALUES(?1,?2,?3)",
                params![
                    scope,
                    value,
                    args.get("metadata").unwrap_or(&Value::Null).to_string()
                ],
            )
            .map_err(db)?;
            Ok(json!({"id":tx.last_insert_rowid(),"scope":scope}))
        }
        "memory.recall" => {
            let scope = scope(args)?;
            let query = text(args, "query")?;
            let limit = args
                .get("limit")
                .map_or(Some(10), Value::as_u64)
                .filter(|n| *n > 0 && *n <= 100)
                .ok_or("recall limit must be 1..100")?;
            // Treat user input as lexical terms, not FTS operators or SQL.
            let terms: Vec<_> = query
                .split_whitespace()
                .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
                .collect();
            if terms.is_empty() {
                return Ok(json!([]));
            }
            let mut statement=tx.prepare("SELECT f.id,f.text,f.metadata FROM memory_search JOIN memory_facts f ON f.id=memory_search.rowid WHERE memory_search MATCH ?1 AND f.scope=?2 ORDER BY bm25(memory_search),f.id LIMIT ?3").map_err(db)?;
            let rows = statement
                .query_map(params![terms.join(" OR "), scope, limit], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(db)?;
            let mut values = Vec::new();
            for row in rows {
                let (id, text, metadata) = row.map_err(db)?;
                values.push(json!({"id":id,"text":text,"scope":scope,"metadata":serde_json::from_str::<Value>(&metadata).map_err(|e|e.to_string())?}));
            }
            Ok(json!(values))
        }
        "state.get" | "state.set" | "state.compareAndSet" => {
            let scope = scope(args)?;
            let key = text(args, "key")?;
            if key.is_empty() || key.len() > 1024 {
                return Err("invalid state key".into());
            }
            let old: Option<(u64, String)> = tx
                .query_row(
                    "SELECT version,value FROM shared_state WHERE scope=?1 AND key=?2",
                    params![scope, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db)?;
            if name == "state.get" {
                return old
                    .map(|(version, value)| {
                        serde_json::from_str::<Value>(&value)
                            .map(|value| json!({"version":version,"value":value}))
                            .map_err(|e| e.to_string())
                    })
                    .unwrap_or(Ok(Value::Null));
            }
            let current = old.as_ref().map_or(0, |(v, _)| *v);
            if name == "state.compareAndSet" {
                let expected = args
                    .get("expectedVersion")
                    .and_then(Value::as_u64)
                    .ok_or("expectedVersion must be an unsigned integer")?;
                if expected != current {
                    return Ok(json!({"updated":false,"version":current}));
                }
            }
            let value = args.get("value").ok_or("value required")?;
            let version = current
                .checked_add(1)
                .filter(|n| *n <= i64::MAX as u64)
                .ok_or("state version exhausted")?;
            tx.execute("INSERT INTO shared_state(scope,key,version,value) VALUES(?1,?2,?3,?4) ON CONFLICT(scope,key) DO UPDATE SET version=excluded.version,value=excluded.value",params![scope,key,version,value.to_string()]).map_err(db)?;
            Ok(json!({"updated":true,"version":version,"value":value}))
        }
        _ => Err("unknown memory operation".into()),
    }
}
fn messages(tx: &Transaction<'_>, run: &str) -> Result<Vec<(i64, Value)>, String> {
    let mut statement = tx
        .prepare(
            "SELECT id,message FROM context_messages WHERE run_id=?1 AND archived=0 ORDER BY id",
        )
        .map_err(db)?;
    let rows = statement
        .query_map([run], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map_err(db)?;
    rows.map(|r| {
        let (id, s) = r.map_err(db)?;
        Ok((id, serde_json::from_str(&s).map_err(|e| e.to_string())?))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        durability::digest,
        journal::{Journal, OpIntent},
    };
    struct Test {
        path: std::path::PathBuf,
        journal: Journal,
        seq: u64,
    }
    impl Test {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("ondure-memory-{}.sqlite", uuid::Uuid::new_v4()));
            let mut journal = Journal::open(&path).unwrap();
            journal
                .create_run("run", None, "hash", "input", "{}")
                .unwrap();
            Self {
                path,
                journal,
                seq: 0,
            }
        }
        fn op(&mut self, name: &str, args: Value, limits: ContextLimits) -> Result<Value, String> {
            let seq = self.seq;
            self.seq += 1;
            let intent = OpIntent {
                seq,
                name: name.into(),
                args_digest: digest(&args),
                args_json: Some(args.to_string()),
                policy: "safe".into(),
            };
            serde_json::from_value(
                self.journal
                    .memory_operation("run", &[intent], seq, name, &args, limits)
                    .unwrap(),
            )
            .unwrap()
        }
    }
    impl Drop for Test {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    #[test]
    fn maximum_accepted_context_and_compaction_results_fit_encoded_outcome_limit() {
        let limits = ContextLimits {
            max_messages: MAX_CONTEXT_MESSAGES,
            max_bytes: MAX_CONTEXT_BYTES,
            recent_messages: 1,
        };
        limits.validate().unwrap();
        let mut t = Test::new();
        let content = "x".repeat((MAX_CONTEXT_BYTES - 128) / 3);
        let mut result = Value::Null;
        for _ in 0..3 {
            result = t
                .op(
                    "context.add",
                    json!({"message":{"role":"user","content":content}}),
                    limits,
                )
                .unwrap();
        }
        let encoded = json!({"Ok":result}).to_string().len();
        assert!(encoded > MAX_CONTEXT_BYTES - 256);
        assert!(encoded < MAX_OPERATION_RESULT_BYTES);
        let messages = t.op("context.messages", json!({}), limits).unwrap();
        assert_eq!(messages.as_array().unwrap().len(), 3);
        assert!(json!({"Ok":messages}).to_string().len() < MAX_OPERATION_RESULT_BYTES);
        let compacted = t
            .op(
                "context.add",
                json!({"message":{"role":"user","content":"y".repeat(512)}}),
                limits,
            )
            .unwrap();
        assert_eq!(compacted["archived"], 1);
        assert!(json!({"Ok":compacted}).to_string().len() < MAX_OPERATION_RESULT_BYTES);
        // Account for the maximum separator/envelope and persisted ID overhead.
        const { assert!(MAX_CONTEXT_BYTES + MAX_CONTEXT_MESSAGES + 128 < MAX_OPERATION_RESULT_BYTES) };
        assert!(
            serde_json::to_vec(&vec![i64::MAX; MAX_CONTEXT_MESSAGES])
                .unwrap()
                .len()
                < MAX_OPERATION_RESULT_BYTES / 2
        );
    }

    #[test]
    fn session_replay_reuses_state_outcomes_without_incrementing_versions() {
        use crate::{core::durability::Session, ModuleGraph, Policy};
        let dir =
            std::env::temp_dir().join(format!("ondure-memory-replay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("main.js"), "console.log('memory')").unwrap();
        let graph = ModuleGraph::load(&dir.join("main.js")).unwrap();
        let path = dir.join("journal.sqlite");
        let limits = ContextLimits::default();
        let args = json!({"key":"counter","value":1});
        let mut session = Session::create(&path, "run", &Policy::default(), &graph).unwrap();
        let outcome = session.memory_op("state.set", &args, limits).unwrap();
        drop(session);
        let (_, _, mut resumed) = Session::resume(&path, "run").unwrap();
        assert_eq!(
            resumed.memory_op("state.set", &args, limits).unwrap(),
            outcome
        );
        assert_eq!(
            resumed
                .memory_op("state.get", &json!({"key":"counter"}), limits)
                .unwrap()["version"],
            1
        );
        drop(resumed);
        // A committed failure also replays as the same failure, without effects.
        let (_, _, mut resumed) = Session::resume(&path, "run").unwrap();
        resumed.memory_op("state.set", &args, limits).unwrap();
        resumed
            .memory_op("state.get", &json!({"key":"counter"}), limits)
            .unwrap();
        let bad = json!({"message":{"role":"system","content":"x".repeat(100)}});
        let small = ContextLimits {
            max_bytes: 32,
            ..limits
        };
        assert!(resumed
            .memory_op("context.add", &bad, small)
            .unwrap_err()
            .contains("protected context"));
        drop(resumed);
        let (_, _, mut resumed) = Session::resume(&path, "run").unwrap();
        resumed.memory_op("state.set", &args, limits).unwrap();
        resumed
            .memory_op("state.get", &json!({"key":"counter"}), limits)
            .unwrap();
        assert!(resumed
            .memory_op("context.add", &bad, limits)
            .unwrap_err()
            .contains("protected context"));
        drop(resumed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn version_two_migration_preserves_existing_operation_history() {
        let mut t = Test::new();
        t.op(
            "state.set",
            json!({"key":"a","value":1}),
            ContextLimits::default(),
        )
        .unwrap();
        let original = t.journal.operations("run").unwrap();
        let connection = rusqlite::Connection::open(&t.path).unwrap();
        connection.execute_batch("DROP TABLE context_compactions; DROP TABLE context_messages; DROP TABLE shared_state; DROP TRIGGER memory_insert; DROP TABLE memory_search; DROP TABLE memory_facts; PRAGMA user_version=2;").unwrap();
        drop(connection);
        let reopened = Journal::open(&t.path).unwrap();
        assert_eq!(reopened.operations("run").unwrap(), original);
        let connection = rusqlite::Connection::open(&t.path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn retrieval_is_scoped_and_recorded_across_shared_changes() {
        let mut t = Test::new();
        let l = ContextLimits::default();
        t.op(
            "memory.remember",
            json!({"text":"olive trees","scope":"a"}),
            l,
        )
        .unwrap();
        t.op(
            "memory.remember",
            json!({"text":"olive oil","scope":"b"}),
            l,
        )
        .unwrap();
        let result = t
            .op("memory.recall", json!({"query":"olive","scope":"a"}), l)
            .unwrap();
        assert_eq!(result.as_array().unwrap().len(), 1);
        assert_eq!(result[0]["text"], "olive trees");
        t.op(
            "memory.remember",
            json!({"text":"olive branches","scope":"a"}),
            l,
        )
        .unwrap();
        let stored = t.journal.operations("run").unwrap()[2]
            .result
            .clone()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Result<Value, String>>(&stored)
                .unwrap()
                .unwrap(),
            result
        );
        assert_eq!(
            Journal::open(&t.path).unwrap().operations("run").unwrap()[2]
                .result
                .as_deref(),
            Some(stored.as_str())
        );
    }
    #[test]
    fn compaction_archives_old_messages_and_protects_system_and_recent() {
        let mut t = Test::new();
        let l = ContextLimits {
            max_messages: 3,
            max_bytes: 1024,
            recent_messages: 1,
        };
        for (role, content) in [
            ("system", "instructions"),
            ("user", "old"),
            ("assistant", "middle"),
            ("user", "recent"),
        ] {
            t.op(
                "context.add",
                json!({"message":{"role":role,"content":content}}),
                l,
            )
            .unwrap();
        }
        let messages = t.op("context.messages", json!({}), l).unwrap();
        assert_eq!(messages.as_array().unwrap().len(), 3);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[2]["content"], "recent");
        let conn = rusqlite::Connection::open(&t.path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM context_messages WHERE archived=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM context_compactions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(t
            .op(
                "context.add",
                json!({"message":{"role":"system","content":"x".repeat(2000)}}),
                l
            )
            .is_err());
        assert_eq!(t.op("context.messages", json!({}), l).unwrap(), messages);
    }
    #[test]
    fn shared_state_cas_conflicts_and_outcome_commit_rolls_back_mutations() {
        let mut t = Test::new();
        let l = ContextLimits::default();
        assert_eq!(
            t.op("state.get", json!({"key":"count"}), l).unwrap(),
            Value::Null
        );
        assert_eq!(
            t.op(
                "state.compareAndSet",
                json!({"key":"count","expectedVersion":0,"value":1}),
                l
            )
            .unwrap()["version"],
            1
        );
        let mut other = Journal::open(&t.path).unwrap();
        other.create_run("other", None, "h", "i", "{}").unwrap();
        let args = json!({"key":"count","expectedVersion":0,"value":2});
        let outcome = other
            .memory_operation(
                "other",
                &[OpIntent {
                    seq: 0,
                    name: "state.compareAndSet".into(),
                    args_digest: digest(&args),
                    args_json: Some(args.to_string()),
                    policy: "safe".into(),
                }],
                0,
                "state.compareAndSet",
                &args,
                l,
            )
            .unwrap();
        assert_eq!(outcome["Ok"]["updated"], false);
        let conn = rusqlite::Connection::open(&t.path).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_outcome BEFORE UPDATE ON operations BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let args = json!({"key":"count","value":99});
        assert!(t
            .journal
            .memory_operation(
                "run",
                &[OpIntent {
                    seq: 2,
                    name: "state.set".into(),
                    args_digest: digest(&args),
                    args_json: Some(args.to_string()),
                    policy: "safe".into()
                }],
                2,
                "state.set",
                &args,
                l
            )
            .is_err());
        assert_eq!(
            conn.query_row(
                "SELECT value FROM shared_state WHERE key='count'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "1"
        );
    }
}
