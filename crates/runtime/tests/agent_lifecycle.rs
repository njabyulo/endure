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
    let waits = Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap();
    assert_eq!(waits[0].kind, "timer");
    assert_eq!(waits[0].answer.as_deref(), Some("null"));
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
    let p = Project::new("import {agent} from 'ondure'; export default agent('sleep',async ctx=>{ console.log('before'); await ctx.sleep('1h'); console.log('after'); });");
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
    let mut journal = Journal::open(&p.journal()).unwrap();
    let wait = journal.waits(&run.id).unwrap().remove(0);
    assert!(wait.wake_at.is_some());
    // Wake through the scheduler without a wall-clock race. Editing wake_at
    // alone would conflict with the separately recorded wait.deadline result.
    // Real deadline readiness is covered by the zero-duration agent above and
    // parking::future_timer_parks_and_only_wakes_after_its_persisted_deadline.
    journal.answer_wait(&run.id, wait.seq, "null").unwrap();
    let mut outcomes = wake_ready(&p.journal()).unwrap();
    assert_eq!(outcomes.len(), 1);
    let output = outcomes.remove(0).result.unwrap();
    assert_eq!(output.status, "done");
    assert_eq!(journal.waits(&run.id).unwrap()[0].wake_at, wait.wake_at);
    assert_eq!(
        journal.waits(&run.id).unwrap()[0].answer.as_deref(),
        Some("null")
    );
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

#[test]
fn oversized_note_is_rejected_before_committing_an_answer() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('review', async ctx=>{ console.log((await ctx.human.approve({action:'continue'})).note); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    let seq = Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap()[0].seq;
    let oversized = "x".repeat(runtime::MAX_OPERATION_RESULT_BYTES);
    assert!(approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        seq,
        true,
        Some(&oversized),
        RunLimits::default()
    )
    .is_err());
    assert!(
        Journal::open(&p.journal()).unwrap().waits(&run.id).unwrap()[0]
            .answer
            .is_none()
    );
    assert_eq!(
        approve_run_with_note_and_limits(
            &p.journal(),
            &run.id,
            seq,
            true,
            Some("accepted"),
            RunLimits::default()
        )
        .unwrap()
        .stdout,
        "accepted\n"
    );
}

#[test]
fn handler_failures_preserve_original_message_and_failed_history_on_replay() {
    for handler in [
        "() => { throw Error('missinginput'); }",
        "async () => { await Promise.resolve(); throw Error('missinginput'); }",
    ] {
        let project = Project::new(&format!(
            "import {{agent}} from 'ondure'; export default agent('failure', {handler});"
        ));
        let error = run_request(
            &Policy::default(),
            &project.graph(),
            &project.journal(),
            "failed-request",
            "null",
        )
        .unwrap_err();
        assert_eq!(error.message, "Error: missinginput");
        let journal = Journal::open(&project.journal()).unwrap();
        let original = journal.find_request("failed-request").unwrap().unwrap();
        assert_eq!(original.status, "failed");
        let replayed = replay_run(&project.journal(), &original.id, None).unwrap_err();
        assert_eq!(replayed.message, error.message);
        assert_eq!(journal.get_run(&original.id).unwrap(), original);
    }
}

#[test]
fn elapsed_relative_sleep_resumes_and_completed_replay_preserves_history() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('sleep',async ctx=>{ console.log('before'); await ctx.sleep('1s'); console.log('after'); });");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    assert_eq!(run.status, "parked");
    fs::remove_file(p.0.join("main.js")).unwrap();
    let journal = Journal::open(&p.journal()).unwrap();
    let waits = journal.waits(&run.id).unwrap();
    let before = journal.operations(&run.id).unwrap();
    let deadline: Result<serde_json::Value, String> = serde_json::from_str(
        before
            .iter()
            .find(|op| op.name == "wait.deadline")
            .unwrap()
            .result
            .as_ref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(deadline.unwrap().as_i64(), waits[0].wake_at);
    // Replaying a still-parked run must reuse the captured deadline without
    // changing the timer's identity or committing a replacement clock value.
    replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(journal.operations(&run.id).unwrap(), before);
    assert_eq!(journal.waits(&run.id).unwrap(), waits);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    std::thread::sleep(std::time::Duration::from_millis(
        (waits[0].wake_at.unwrap() - now).max(0) as u64 + 30,
    ));
    let outcomes = wake_ready(&p.journal()).unwrap();
    assert_eq!(outcomes.len(), 1);
    let completed = outcomes[0].result.as_ref().unwrap();
    assert_eq!(completed.status, "done");
    assert_eq!(completed.stdout, "before\nafter\n");
    let history = journal.operations(&run.id).unwrap();
    let completed_waits = journal.waits(&run.id).unwrap();
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        completed.stdout
    );
    assert_eq!(journal.operations(&run.id).unwrap(), history);
    assert_eq!(journal.waits(&run.id).unwrap(), completed_waits);
}

#[test]
fn timed_approval_replays_while_parked_and_after_operator_answer() {
    let p = Project::new("import {agent} from 'ondure'; export default agent('review',async ctx=>console.log(JSON.stringify(await ctx.human.approve({timeout:'1h'}))));");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    assert_eq!(run.status, "parked");
    fs::remove_file(p.0.join("main.js")).unwrap();
    let journal = Journal::open(&p.journal()).unwrap();
    let waits = journal.waits(&run.id).unwrap();
    let before = journal.operations(&run.id).unwrap();
    replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(journal.operations(&run.id).unwrap(), before);
    assert_eq!(journal.waits(&run.id).unwrap(), waits);
    let output = approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        waits[0].seq,
        true,
        Some("approved after restart"),
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(output.stdout.trim()).unwrap(),
        serde_json::json!({"approved":true,"note":"approved after restart"})
    );
    let history = journal.operations(&run.id).unwrap();
    let completed_waits = journal.waits(&run.id).unwrap();
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        output.stdout
    );
    assert_eq!(journal.operations(&run.id).unwrap(), history);
    assert_eq!(journal.waits(&run.id).unwrap(), completed_waits);
}

#[test]
fn script_approval_rejects_notes_without_committing_an_answer() {
    let p = Project::new("console.log(await ondure.approve({action:'continue'}));");
    let run = run_request(
        &Policy::default(),
        &p.graph(),
        &p.journal(),
        "request",
        "null",
    )
    .unwrap();
    assert_eq!(run.status, "parked");
    let journal = Journal::open(&p.journal()).unwrap();
    let wait = journal.waits(&run.id).unwrap().remove(0);
    let before = journal.operations(&run.id).unwrap();
    let error = approve_run_with_note_and_limits(
        &p.journal(),
        &run.id,
        wait.seq,
        true,
        Some("must not be lost"),
        RunLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.message, "approval notes require an agent approval");
    assert_eq!(journal.waits(&run.id).unwrap(), vec![wait.clone()]);
    assert_eq!(journal.operations(&run.id).unwrap(), before);
    assert_eq!(journal.get_run(&run.id).unwrap().status, "parked");
    assert_eq!(
        approve_run_with_note_and_limits(
            &p.journal(),
            &run.id,
            wait.seq,
            true,
            None,
            RunLimits::default()
        )
        .unwrap()
        .stdout,
        "true\n"
    );
}
