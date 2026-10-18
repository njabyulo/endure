use runtime::{
    approve_run_with_note_and_limits, replay_run, run_request, wake_ready, Journal, ModuleGraph,
    Policy, RunLimits,
};
use std::{fs, path::PathBuf};
struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ondure-agent-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("main.js"), source).unwrap();
        Self(p)
    }
    fn graph(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.js")).unwrap()
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
fn invokes_only_default_agent_after_evaluation_with_persisted_input() {
    let p = Project::new("import {agent} from 'ondure'; agent('unused',()=>{throw Error('unused')}); await Promise.resolve(); export default agent('hello',{},async(ctx,input)=>{ console.log(input.name); await ctx.sleep('0ms'); console.log('done'); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        r#"{"name":"Ada"}"#,
    )
    .unwrap();
    assert_eq!(run.stdout, "Ada\ndone\n");
    fs::remove_file(p.0.join("main.js")).unwrap();
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
}
#[test]
fn approval_survives_restart_and_returns_operator_note() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('review',async(ctx,input)=>{ const answer = await ctx.human.approve({action:input.action,recipient:'local operator'}); console.log(JSON.stringify(answer)); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        r#"{"action":"send mock email"}"#,
    )
    .unwrap();
    assert_eq!(run.status, "parked");
    let seq = Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap()[0].seq;
    fs::remove_file(p.0.join("main.js")).unwrap();
    let output = approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        seq,
        true,
        Some("Reviewed locally"),
        RunLimits::default(),
    )
    .unwrap();
    let answer: serde_json::Value = serde_json::from_str(output.stdout.trim()).unwrap();
    assert_eq!(
        answer,
        serde_json::json!({"approved":true,"note":"Reviewed locally"})
    );
    assert!(approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        seq,
        false,
        None,
        RunLimits::default()
    )
    .is_err());
}
#[test]
fn committed_timeout_wins_over_later_approval() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('review',async ctx=>{ console.log(JSON.stringify(await ctx.human.approve({timeout:0,onTimeout:'reject'}))); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    let answer: serde_json::Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(answer["approved"], false);
    let seq = Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap()[0].seq;
    assert!(approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        seq,
        true,
        None,
        RunLimits::default()
    )
    .is_err());
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
    assert!(wake_ready(&p.journal()).unwrap().is_empty());
}
#[test]
fn operator_answer_committed_before_timeout_is_reused() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('review',async ctx=>console.log(JSON.stringify(await ctx.human.approve({timeout:'1h'}))));");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    let seq = Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap()[0].seq;
    let output = approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        seq,
        true,
        None,
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.stdout.trim()).unwrap()["approved"],
        true
    );
}
#[test]
fn malformed_agent_or_duration_fails() {
    for source in ["import {agent} from 'ondure'; export default {};", "import {agent} from 'ondure'; export default agent('x',ctx=>ctx.sleep(-1));", "import {agent} from 'ondure'; export default agent('x',ctx=>ctx.human.approve({timeout:0,onTimeout:'approve'}));"] {
        let p = Project::new(source);
        assert!(run_request(&Policy::default(), &p.graph(), &p.journal(), "request", "null").is_err());
    }
}

#[test]
fn relative_sleep_parks_and_reuses_recorded_deadline_after_restart() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('sleep',async ctx=>{ console.log('before'); await ctx.sleep('30ms'); console.log('after'); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    assert_eq!(run.status, "parked");
    assert_eq!(run.stdout, "before\n");
    std::thread::sleep(std::time::Duration::from_millis(40));
    let output = replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(output.stdout, "before\nafter\n");
    assert_eq!(
        Journal::open(&p.journal())
            .unwrap()
            .waits(&run.id)
            .unwrap()
            .len(),
        1
    );
}
