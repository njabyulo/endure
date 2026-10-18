use runtime::{diff_runs, inspect_run, Journal, OpIntent};
use serde_json::json;

#[test]
fn compares_recorded_outcomes_without_dispatch_or_exposing_snapshot_credentials() {
    let path = std::env::temp_dir().join(format!(
        "ondure-inspection-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let mut journal = Journal::open(&path).unwrap();
    for (id, value) in [("a", "first"), ("b", "second")] {
        journal.create_run(id, None, "hash", "input", &json!({"input": {"task": "compare"}, "net": false, "host_config": {"grants": {"tools": ["mock"]}, "credentials": {"api": "ENV_NAME"}}}).to_string()).unwrap();
        journal
            .intents(
                id,
                &[OpIntent {
                    seq: 0,
                    name: "tool.mock".into(),
                    args_digest: "args".into(),
                    args_json: Some("{}".into()),
                    policy: "never".into(),
                }],
            )
            .unwrap();
        journal.begin_attempt(id, 0).unwrap();
        journal
            .complete(
                id,
                0,
                &json!({"Ok": {"text": value, "usage": {"total_tokens": 3}}}).to_string(),
            )
            .unwrap();
        journal.finish_run(id, "done", "", "").unwrap();
    }
    let before = journal.attempts("a").unwrap();
    let inspection = inspect_run(&path, "a").unwrap();
    assert_eq!(inspection["input"]["task"], "compare");
    assert_eq!(inspection["permissions"]["grants"]["tools"][0], "mock");
    assert!(!inspection.to_string().contains("ENV_NAME"));
    assert_eq!(inspection["operations"][0]["args_json"], "{}");
    let different = diff_runs(&path, "a", "b").unwrap();
    assert_eq!(different["identical"], false);
    assert_eq!(different["operations"].as_array().unwrap().len(), 1);
    assert_eq!(diff_runs(&path, "a", "a").unwrap()["identical"], true);
    assert!(diff_runs(&path, "a", "missing").is_err());
    assert_eq!(journal.attempts("a").unwrap(), before);
    drop(journal);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn resource_only_changes_are_reported() {
    let path = std::env::temp_dir().join(format!(
        "ondure-resource-diff-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let mut journal = Journal::open(&path).unwrap();
    for id in ["a", "b"] {
        journal.create_run(id, None, "hash", "input", "{}").unwrap();
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("INSERT INTO resource_reservations(run_id,seq,category,tokens,micros,state,admitted_at,input_price,output_price) VALUES('b',0,'model',8,1,'settled',0,0,0)", []).unwrap();
    let diff = diff_runs(&path, "a", "b").unwrap();
    assert_eq!(diff["identical"], false);
    assert!(diff["operations"].as_array().unwrap().is_empty());
    assert!(diff["metadata"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["field"] == "resources"));
    drop(connection);
    drop(journal);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn matching_approvals_and_accounting_ignore_record_ownership_and_timestamps() {
    let path = std::env::temp_dir().join(format!(
        "ondure-semantic-diff-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let mut journal = Journal::open(&path).unwrap();
    for id in ["a", "b"] {
        journal.create_run(id, None, "hash", "input", "{}").unwrap();
        journal
            .intents(
                id,
                &[OpIntent {
                    seq: 0,
                    name: "wait.approval".into(),
                    args_digest: "args".into(),
                    args_json: Some("{}".into()),
                    policy: "safe".into(),
                }],
            )
            .unwrap();
        journal.park_wait(id, 0, "approval", None, "{}").unwrap();
        journal.answer_wait(id, 0, "true").unwrap();
        journal.complete(id, 0, r#"{"Ok":true}"#).unwrap();
        journal.finish_run(id, "done", "", "").unwrap();
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    for (id, start) in [("a", 1000), ("b", 3000)] {
        connection
            .execute(
                "UPDATE run_resources SET started_at=?2,deadline_at=?2+1000 WHERE run_id=?1",
                rusqlite::params![id, start],
            )
            .unwrap();
        connection.execute("INSERT INTO resource_reservations(run_id,seq,category,tokens,micros,state,admitted_at,input_price,output_price) VALUES(?1,0,'tool',5,2,'settled',?2,0,0)", rusqlite::params![id,start]).unwrap();
    }
    assert_eq!(diff_runs(&path, "a", "b").unwrap()["identical"], true);
    assert_ne!(
        inspect_run(&path, "a").unwrap()["waits"],
        inspect_run(&path, "b").unwrap()["waits"]
    );
    drop(connection);
    drop(journal);
    std::fs::remove_file(path).unwrap();
}
