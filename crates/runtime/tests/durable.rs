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
