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
