mod support;
use runtime::{run_durable, Journal, ModuleGraph, Policy};
use std::{fs, path::PathBuf};
use support::Witness;
struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ondure-durable-{}", uuid::Uuid::new_v4()));
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
fn durable_fetch_and_timer_results_precede_successful_run() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let project = Project::new(&format!("console.log((await (await fetch('{}')).json()).hit); await new Promise(resolve => setTimeout(resolve, 1));", witness.url()));
    let output = run_durable(
        &Policy { net: true },
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    assert_eq!(output.stdout, "1\n");
    assert_eq!(witness.finish(), 1);
    let journal = Journal::open(&project.journal()).unwrap();
    assert_eq!(journal.get_run("run").unwrap().status, "done");
    let ops = journal.operations("run").unwrap();
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0].name, "fetch.get");
    assert_eq!(ops[1].name, "timer.sleep");
    assert!(ops.iter().all(|op| op.state == "completed"));
    assert_eq!(ops[0].completion_order, Some(0));
    assert_eq!(ops[1].completion_order, Some(1));
}

#[test]
fn clearing_non_timer_ids_never_cancels_fetches_or_adds_journal_entries() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let project = Project::new(&format!(
        "const pending = fetch('{}'); clearTimeout(1); clearTimeout(999); console.log((await (await pending).json()).hit);",
        witness.url()
    ));
    let output = run_durable(
        &Policy { net: true },
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    assert_eq!(output.stdout, "1\n");
    assert_eq!(witness.finish(), 1);
    let journal = Journal::open(&project.journal()).unwrap();
    let ops = journal.operations("run").unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].name, "fetch.get");
    assert_eq!(ops[0].state, "completed");
    assert!(ops[0].result.as_ref().unwrap().contains("hit"));
}

#[test]
fn failures_and_canceled_timers_are_persisted() {
    let project = Project::new("const id = setTimeout(() => console.log('wrong'), 10000); clearTimeout(id); console.log('starting'); throw Error('boom');");
    let error = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap_err();
    assert!(error.message.contains("boom"));
    let journal = Journal::open(&project.journal()).unwrap();
    let run = journal.get_run("run").unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(run.stdout, "starting\n");
    assert!(journal
        .operations("run")
        .unwrap()
        .iter()
        .all(|op| op.state == "completed"));
}

#[test]
fn completed_operations_replay_without_network_or_timer_wait() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let project = Project::new(&format!("console.log((await (await fetch('{}')).json()).hit); await new Promise(resolve => setTimeout(resolve, 10));", witness.url()));
    let graph = project.graph();
    let first = run_durable(&Policy { net: true }, &graph, &project.journal(), "run").unwrap();
    for _ in 0..3 {
        let replayed = runtime::replay_run(&project.journal(), "run", None).unwrap();
        assert_eq!(first, replayed);
    }
    assert_eq!(witness.finish(), 1);
    assert_eq!(
        Journal::open(&project.journal())
            .unwrap()
            .operations("run")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn replay_checks_code_and_detects_divergent_operation_arguments() {
    let project =
        Project::new("await new Promise(resolve => setTimeout(resolve, 1)); console.log('done');");
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    fs::write(project.0.join("main.ts"), "console.log('changed');").unwrap();
    let error = runtime::replay_run(&project.journal(), "run", Some(&project.graph())).unwrap_err();
    assert!(error.message.contains("code hash mismatch"));
    assert!(error.output.stdout.is_empty());
    // An inconsistent historic argument digest must fail before callbacks run.
    let connection = rusqlite::Connection::open(project.journal()).unwrap();
    connection
        .execute(
            "UPDATE operations SET args_digest = 'different' WHERE run_id = 'run' AND seq = 0",
            [],
        )
        .unwrap();
    let error = runtime::replay_run(&project.journal(), "run", None).unwrap_err();
    assert!(error.message.contains("replay divergence"));
    assert!(error.output.stdout.is_empty());
    assert_eq!(
        Journal::open(&project.journal())
            .unwrap()
            .get_run("run")
            .unwrap()
            .status,
        "done"
    );
}

#[test]
fn divergent_replays_keep_terminal_status_output_and_history_sealed() {
    let project =
        Project::new("console.log('original'); setTimeout(() => {}, 0); setTimeout(() => {}, 0);");
    run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    // An inconsistent recorded prefix must never become an extensible run after
    // replay fails. This deterministically exercises an unexpected extra op.
    rusqlite::Connection::open(project.journal())
        .unwrap()
        .execute(
            "DELETE FROM operations WHERE run_id = 'run' AND seq = 1",
            [],
        )
        .unwrap();
    let journal = Journal::open(&project.journal()).unwrap();
    let original = journal.get_run("run").unwrap();
    let history = journal.operations("run").unwrap();
    for _ in 0..2 {
        let error = runtime::replay_run(&project.journal(), "run", None).unwrap_err();
        assert!(
            error.message.contains("unexpected operation"),
            "{}",
            error.message
        );
        assert_eq!(journal.get_run("run").unwrap(), original);
        assert_eq!(journal.operations("run").unwrap(), history);
    }
}

#[test]
fn rejection_stringification_registers_its_operations_before_replay_delivery() {
    let project = Project::new(
        "Promise.reject({toString() { setTimeout(() => console.log('done'), 0); return 'bad'; }});",
    );
    let first = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap_err();
    assert_eq!(first.message, "unhandled promise rejection: bad");
    assert_eq!(first.output.stdout, "done\n");
    let journal = Journal::open(&project.journal()).unwrap();
    let original = journal.get_run("run").unwrap();
    let replayed = runtime::replay_run(&project.journal(), "run", None).unwrap_err();
    assert_eq!(replayed.message, first.message);
    assert_eq!(replayed.output, first.output);
    assert_eq!(journal.get_run("run").unwrap(), original);
}

#[test]
fn replay_preserves_cancellation_and_completion_order() {
    let project = Project::new("const id = setTimeout(() => console.log('wrong'), 1000); clearTimeout(id); const a = new Promise(resolve => setTimeout(() => {console.log('slow'); resolve(1);}, 10)); const b = new Promise(resolve => setTimeout(() => {console.log('fast'); resolve(2);}, 0)); console.log(await Promise.race([a,b])); await a;");
    let first = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    assert_eq!(first.stdout, "fast\n2\nslow\n");
    assert_eq!(
        runtime::replay_run(&project.journal(), "run", None).unwrap(),
        first
    );
}

#[test]
fn hundred_replays_return_identical_time_randomness_and_operation_sequence() {
    let project = Project::new("console.log(JSON.stringify([Date.now(), new Date().getTime(), Date(), performance.now(), Math.random(), crypto.randomUUID()])); await new Promise(resolve => setTimeout(resolve, 1)); console.log(Math.random(), typeof WeakRef, typeof FinalizationRegistry);");
    let graph = project.graph();
    let first = run_durable(&Policy::default(), &graph, &project.journal(), "run").unwrap();
    let history = Journal::open(&project.journal())
        .unwrap()
        .operations("run")
        .unwrap();
    for _ in 0..100 {
        assert_eq!(
            runtime::replay_run(&project.journal(), "run", None).unwrap(),
            first
        );
        assert_eq!(
            Journal::open(&project.journal())
                .unwrap()
                .operations("run")
                .unwrap(),
            history
        );
    }
}

#[test]
fn changing_reflect_construct_cannot_expose_native_date() {
    let project = Project::new("let leaked; const original = Reflect.construct; Reflect.construct = (target, args, newTarget) => { leaked = target; return original(target, args, newTarget); }; console.log(new Date().getTime() > 0, typeof leaked, new Date(0).toISOString());");
    let first = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    assert_eq!(first.stdout, "true undefined 1970-01-01T00:00:00.000Z\n");
    assert_eq!(
        runtime::replay_run(&project.journal(), "run", None).unwrap(),
        first
    );
}

#[test]
fn random_samples_round_trip_without_losing_float_bits() {
    let project = Project::new(
        "console.log(JSON.stringify(Array.from({length: 1000}, () => Math.random())));",
    );
    let first = run_durable(
        &Policy::default(),
        &project.graph(),
        &project.journal(),
        "run",
    )
    .unwrap();
    assert_eq!(
        runtime::replay_run(&project.journal(), "run", None).unwrap(),
        first
    );
}
