mod support;

use runtime::{approve_run, run_durable, run_request, wake_ready, Journal, ModuleGraph, Policy};
use std::{
    fs,
    path::PathBuf,
    sync::Barrier,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use support::Witness;

struct Project(PathBuf);

impl Project {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ondure-parking-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("main.ts"), source).unwrap();
        Self(path)
    }

    fn graph(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.ts")).unwrap()
    }

    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn approval_survives_source_removal_without_repeating_completed_effects() {
    let witness = Witness::new(200, r#"{"ok":true}"#);
    let project = Project::new(&format!(
        "await fetch('{}'); const accepted = await ondure.approve({{action:'send email', to:'reader'}}); console.log('approved', accepted); if (accepted) await fetch('{}');",
        witness.url(), witness.url()
    ));
    run_durable(
        &Policy { net: true },
        &project.graph(),
        &project.journal(),
        "approval",
    )
    .unwrap();
    let journal = Journal::open(&project.journal()).unwrap();
    assert_eq!(journal.get_run("approval").unwrap().status, "parked");
    let waits = journal.waits("approval").unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].kind, "approval");
    let details: serde_json::Value = serde_json::from_str(&waits[0].details).unwrap();
    assert!(details.to_string().contains("send email"));
    assert_eq!(waits[0].answer, None);
    let seq = waits[0].seq;
    drop(journal);
    fs::remove_file(project.0.join("main.ts")).unwrap();
    let resumed = approve_run(&project.journal(), "approval", seq, true).unwrap();
    assert!(resumed.stdout.contains("approved true"));
    assert_eq!(
        Journal::open(&project.journal())
            .unwrap()
            .get_run("approval")
            .unwrap()
            .status,
        "done"
    );
    approve_run(&project.journal(), "approval", seq, true).unwrap();
    let error = approve_run(&project.journal(), "approval", seq, false).unwrap_err();
    assert!(error.message.contains("conflicting answer"));
    assert_eq!(witness.finish(), 2);
}

#[test]
fn rejected_approval_returns_false_and_skips_the_effect() {
    let witness = Witness::new(200, "{}");
    let project = Project::new(&format!(
        "const accepted = await ondure.approve({{action:'network'}}); console.log(accepted); if (accepted) await fetch('{}');",
        witness.url()
    ));
    run_durable(
        &Policy { net: true },
        &project.graph(),
        &project.journal(),
        "rejected",
    )
    .unwrap();
    let seq = Journal::open(&project.journal())
        .unwrap()
        .waits("rejected")
        .unwrap()[0]
        .seq;
    assert_eq!(
        approve_run(&project.journal(), "rejected", seq, false)
            .unwrap()
            .stdout,
        "false\n"
    );
    assert_eq!(witness.finish(), 0);
}

#[test]
fn future_timer_parks_and_only_wakes_after_its_persisted_deadline() {
    let project = Project::new("await ondure.sleepUntil(Date.now() + 1000); console.log('awake');");
    let output = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "timer",
    )
    .unwrap();
    assert!(output.stdout.is_empty());
    let journal = Journal::open(&project.journal()).unwrap();
    assert_eq!(journal.get_run("timer").unwrap().status, "parked");
    let wait = journal.waits("timer").unwrap().remove(0);
    assert_eq!(wait.kind, "timer");
    let deadline = wait.wake_at.unwrap();
    assert!(wake_ready(&project.journal()).unwrap().is_empty());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    thread::sleep(Duration::from_millis((deadline - now).max(0) as u64 + 50));
    fs::remove_file(project.0.join("main.ts")).unwrap();
    let runs = wake_ready(&project.journal()).unwrap();
    assert_eq!(runs.len(), 1);
    let run = runs[0].result.as_ref().unwrap();
    assert_eq!(run.status, "done");
    assert_eq!(run.stdout, "awake\n");
    assert!(wake_ready(&project.journal()).unwrap().is_empty());
    assert_eq!(
        Journal::open(&project.journal())
            .unwrap()
            .waits("timer")
            .unwrap()[0]
            .answer
            .as_deref(),
        Some("null")
    );
}

#[test]
fn repeated_requests_return_cached_results_and_reject_identity_changes() {
    let witness = Witness::new(200, "{}");
    let project = Project::new(&format!(
        "await fetch('{}'); console.log(ondure.input.name);",
        witness.url()
    ));
    let graph = project.graph();
    let policy = Policy { net: true };
    let first = run_request(
        &policy,
        &graph,
        &project.journal(),
        "request",
        r#"{"name":"reader"}"#,
    )
    .unwrap();
    assert_eq!(first.status, "done");
    assert_eq!(first.stdout, "reader\n");
    assert_eq!(first.request_id.as_deref(), Some("request"));
    let repeated = run_request(
        &policy,
        &graph,
        &project.journal(),
        "request",
        r#"{ "name": "reader" }"#,
    )
    .unwrap();
    assert_eq!(repeated, first);
    for (policy, input) in [
        (Policy { net: true }, r#"{"name":"other"}"#),
        (Policy::default(), r#"{"name":"reader"}"#),
    ] {
        let error = run_request(&policy, &graph, &project.journal(), "request", input).unwrap_err();
        assert!(error.message.contains("conflicts"));
    }
    fs::write(project.0.join("main.ts"), "console.log('changed');").unwrap();
    let error = run_request(
        &policy,
        &project.graph(),
        &project.journal(),
        "request",
        r#"{"name":"reader"}"#,
    )
    .unwrap_err();
    assert!(error.message.contains("conflicts"));
    // A cached done request must not replay JavaScript, even if its operation history is corrupt.
    let connection = rusqlite::Connection::open(project.journal()).unwrap();
    connection
        .execute(
            "UPDATE operations SET args_digest = 'corrupt' WHERE run_id = ?1",
            [&first.id],
        )
        .unwrap();
    assert_eq!(
        run_request(
            &policy,
            &graph,
            &project.journal(),
            "request",
            r#"{"name":"reader"}"#
        )
        .unwrap(),
        first
    );
    assert_eq!(witness.finish(), 1);
}

#[test]
fn concurrent_requests_never_execute_the_effect_twice() {
    let witness = Witness::new(200, "{}");
    let project = Project::new(&format!("await new Promise(resolve => setTimeout(resolve, 200)); await fetch('{}'); console.log('done');", witness.url()));
    let graph = project.graph();
    let path = project.journal();
    drop(Journal::open(&path).unwrap());
    let barrier = Barrier::new(2);
    let results = thread::scope(|scope| {
        let one = scope.spawn(|| {
            barrier.wait();
            run_request(&Policy { net: true }, &graph, &path, "request", "null")
        });
        let two = scope.spawn(|| {
            barrier.wait();
            run_request(&Policy { net: true }, &graph, &path, "request", "null")
        });
        [one.join().unwrap(), two.join().unwrap()]
    });
    assert!(results.iter().any(Result::is_ok));
    for error in results.iter().filter_map(|result| result.as_ref().err()) {
        assert!(error.message.contains("already running"), "{error}");
    }
    let stored = run_request(&Policy { net: true }, &graph, &path, "request", "null").unwrap();
    assert_eq!(stored.status, "done");
    assert_eq!(stored.stdout, "done\n");
    assert!(results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .all(|run| run.id == stored.id));
    assert_eq!(witness.finish(), 1);
}

#[test]
fn parked_runs_still_reject_a_changed_replacement_graph() {
    let project = Project::new("await ondure.approve({action:'continue'});");
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "parked",
    )
    .unwrap();
    fs::write(project.0.join("main.ts"), "console.log('changed');").unwrap();
    let error =
        runtime::replay_run(&project.journal(), "parked", Some(&project.graph())).unwrap_err();
    assert!(error.message.contains("code hash mismatch"));
}

#[test]
fn durable_deadline_fires_while_an_ordinary_timer_is_pending() {
    let project = Project::new("const winner = await Promise.race([ondure.sleepUntil(Date.now()+50).then(()=>'durable'), new Promise(resolve=>setTimeout(()=>resolve('ordinary'),500))]); console.log(winner);");
    let output = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "race",
    )
    .unwrap();
    assert_eq!(output.stdout, "durable\n");
}

#[test]
fn wake_batch_keeps_successes_and_continues_after_a_failed_run() {
    let project = Project::new("await ondure.sleepUntil(Date.now()+200); console.log('first');");
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "a",
    )
    .unwrap();
    fs::write(
        project.0.join("main.ts"),
        "await ondure.sleepUntil(Date.now()+200); console.log('second'); throw Error('boom');",
    )
    .unwrap();
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "b",
    )
    .unwrap();
    fs::write(
        project.0.join("main.ts"),
        "await ondure.sleepUntil(Date.now()+200); console.log('third');",
    )
    .unwrap();
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "c",
    )
    .unwrap();
    thread::sleep(Duration::from_millis(250));
    let outcomes = wake_ready(&project.journal()).unwrap();
    assert_eq!(outcomes.len(), 3);
    assert_eq!(outcomes[0].result.as_ref().unwrap().stdout, "first\n");
    assert_eq!(
        outcomes[1].result.as_ref().unwrap_err().output.stdout,
        "second\n"
    );
    assert_eq!(outcomes[2].result.as_ref().unwrap().stdout, "third\n");
}
