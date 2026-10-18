mod support;

use runtime::{
    replay_run, run_durable, Journal, ModuleGraph, Policy, MAX_OPERATION_ARGUMENT_BYTES,
    MAX_OPERATION_RESULT_BYTES,
};
use std::{fs, path::PathBuf};
use support::Witness;

struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("ondure-contract-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("main.js"), source).unwrap();
        Self(directory)
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
    fn run(&self) -> Result<runtime::RunOutput, runtime::HostError> {
        run_durable(
            &Policy { net: true },
            &ModuleGraph::load(&self.0.join("main.js")).unwrap(),
            &self.journal(),
            "run",
        )
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn completed_external_operations_keep_identity_and_do_not_dispatch_on_replay() {
    let witness = Witness::new(200, r#"{"ok":true}"#);
    let url = witness.url();
    let project = Project::new(&format!(
        "console.log((await (await fetch('{url}')).json()).ok);"
    ));
    let first = project.run().unwrap();
    let journal = Journal::open(&project.journal()).unwrap();
    let operations = journal.operations("run").unwrap();
    assert_eq!(operations[0].seq, 0);
    assert_eq!(
        operations[0].args_json.as_deref(),
        Some(serde_json::json!([url]).to_string().as_str())
    );
    let attempts = journal.attempts("run").unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(replay_run(&project.journal(), "run", None).unwrap(), first);
    assert_eq!(journal.attempts("run").unwrap(), attempts);
    assert_eq!(witness.finish(), 1);
}

#[test]
fn oversized_arguments_are_denied_before_any_external_dispatch() {
    let witness = Witness::new(200, "{}");
    let project = Project::new(&format!(
        "await fetch('{}' + 'x'.repeat({MAX_OPERATION_ARGUMENT_BYTES}));",
        witness.url()
    ));
    let failure = project.run().unwrap_err();
    assert!(
        failure.message.contains("arguments exceed"),
        "{}",
        failure.message
    );
    let journal = Journal::open(&project.journal()).unwrap();
    assert!(journal.operations("run").unwrap().is_empty());
    assert!(journal.attempts("run").unwrap().is_empty());
    assert_eq!(witness.finish(), 0);
}

#[test]
fn oversized_response_is_a_committed_error_and_replay_never_contacts_receiver() {
    let witness = Witness::new(200, &"x".repeat(MAX_OPERATION_RESULT_BYTES + 1));
    let project = Project::new(&format!("await fetch('{}');", witness.url()));
    let first = project.run().unwrap_err();
    assert!(
        first.message.contains("response exceeds"),
        "{}",
        first.message
    );
    let journal = Journal::open(&project.journal()).unwrap();
    let ops = journal.operations("run").unwrap();
    assert_eq!(ops[0].state, "completed");
    assert!(ops[0].result.as_ref().unwrap().contains("Err"));
    let replayed = replay_run(&project.journal(), "run", None).unwrap_err();
    assert_eq!(replayed.message, first.message);
    assert_eq!(journal.attempts("run").unwrap().len(), 1);
    assert_eq!(witness.finish(), 1);
}

#[test]
fn encoded_response_limit_counts_json_escaping() {
    let witness = Witness::new(200, &"\"".repeat(MAX_OPERATION_RESULT_BYTES / 2));
    let project = Project::new(&format!("await fetch('{}');", witness.url()));
    let first = project.run().unwrap_err();
    assert!(first.message.contains("result exceed"), "{}", first.message);
    assert_eq!(
        replay_run(&project.journal(), "run", None)
            .unwrap_err()
            .message,
        first.message
    );
    assert_eq!(witness.finish(), 1);
}

#[test]
fn historical_completed_responses_are_not_subject_to_new_size_limits() {
    let witness = Witness::new(200, "{}");
    let project = Project::new(&format!(
        "console.log((await fetch('{}')).ok);",
        witness.url()
    ));
    let first = project.run().unwrap();
    let connection = rusqlite::Connection::open(project.journal()).unwrap();
    // Model a legacy journal whose valid committed result exceeds the new cap.
    let result =
        serde_json::json!({"Ok": [200, "x".repeat(MAX_OPERATION_RESULT_BYTES + 1)]}).to_string();
    connection
        .execute(
            "UPDATE operations SET result = ?1, args_json = NULL WHERE run_id = 'run' AND seq = 0",
            [&result],
        )
        .unwrap();
    drop(connection);
    assert_eq!(replay_run(&project.journal(), "run", None).unwrap(), first);
    assert_eq!(witness.finish(), 1);
}

#[test]
fn approval_answers_are_bounded_before_commit_and_invalid_legacy_answers_finish_as_errors() {
    let project = Project::new("console.log(await ondure.approve({action:'continue'}));");
    project.run().unwrap();
    let mut journal = Journal::open(&project.journal()).unwrap();
    let wait = journal.waits("run").unwrap().remove(0);
    let oversized = serde_json::to_string(&"x".repeat(MAX_OPERATION_RESULT_BYTES - 8)).unwrap();
    assert!(oversized.len() <= MAX_OPERATION_RESULT_BYTES);
    let decoded: serde_json::Value = serde_json::from_str(&oversized).unwrap();
    assert!(
        serde_json::to_vec(&serde_json::json!({"Ok":decoded}))
            .unwrap()
            .len()
            > MAX_OPERATION_RESULT_BYTES
    );
    for answer in [oversized.as_str(), "not-json"] {
        assert!(journal.answer_wait("run", wait.seq, answer).is_err());
        assert!(journal.waits("run").unwrap()[0].answer.is_none());
    }
    assert!(journal.answer_wait("run", wait.seq, "true").unwrap());
    assert_eq!(
        replay_run(&project.journal(), "run", None).unwrap().stdout,
        "true\n"
    );

    for answer in [oversized, "not-json".to_string()] {
        let project = Project::new("console.log(await ondure.approve({action:'legacy'}));");
        project.run().unwrap();
        // Simulate data committed through the former unvalidated journal API.
        let db = rusqlite::Connection::open(project.journal()).unwrap();
        db.execute("UPDATE waits SET answer=?1 WHERE run_id='run'", [&answer])
            .unwrap();
        drop(db);
        let failure = replay_run(&project.journal(), "run", None).unwrap_err();
        assert!(
            failure.message.contains("result exceed") || failure.message.contains("valid JSON")
        );
        let journal = Journal::open(&project.journal()).unwrap();
        let operation = journal.operations("run").unwrap().remove(0);
        assert_eq!(operation.state, "completed");
        let outcome = operation.result.unwrap();
        assert!(outcome.len() < 256);
        assert!(
            serde_json::from_str::<Result<serde_json::Value, String>>(&outcome)
                .unwrap()
                .is_err()
        );
        assert_eq!(
            replay_run(&project.journal(), "run", None)
                .unwrap_err()
                .message,
            failure.message
        );
    }
}
