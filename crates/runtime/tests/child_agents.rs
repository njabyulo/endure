mod support;
use runtime::{
    approve_run, replay_run, run_request_with_config, wake_ready, Capabilities, ChildAllocation,
    ChildConfig, HostConfig, Journal, ModuleGraph, RunLimits,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::PathBuf};
struct Project(PathBuf);
impl Project {
    fn new(parent: &str, child: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ondure-children-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("main.js"), parent).unwrap();
        fs::write(p.join("child.js"), child).unwrap();
        Self(p)
    }
    fn graph(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.js")).unwrap()
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
    fn config(&self) -> HostConfig {
        HostConfig {
            grants: Capabilities {
                child_agents: ["worker".into()].into(),
                ..Default::default()
            },
            agents: BTreeMap::from([(
                "worker".into(),
                ChildConfig {
                    module: "child.js".into(),
                    capabilities: Capabilities::default(),
                    allocation: ChildAllocation {
                        tokens: 100,
                        dollar_micros: 1000,
                    },
                },
            )]),
            ..Default::default()
        }
    }
    fn run(&self, config: &HostConfig) -> runtime::RunRecord {
        run_request_with_config(
            config,
            &self.graph(),
            &self.journal(),
            "parent",
            "null",
            RunLimits::default(),
        )
        .unwrap()
    }
    fn child(&self, parent: &str) -> runtime::RunRecord {
        let j = Journal::open(&self.journal()).unwrap();
        let seq = j
            .operations(parent)
            .unwrap()
            .iter()
            .find(|o| o.name == "agent.run")
            .unwrap()
            .seq;
        j.find_request(&format!("child:{parent}:{seq}"))
            .unwrap()
            .unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
const PARENT:&str="import {agent} from 'ondure';export default agent('parent',{capabilities:{child_agents:['worker']}},async ctx=>{console.log(JSON.stringify(await ctx.agents.run('worker',{name:'Ada'})));});";
#[test]
fn child_returns_json_and_completed_identity_is_reused_after_parent_restart() {
    let p=Project::new(PARENT,"import {agent} from 'ondure';export default agent('child',async(ctx,input)=>({hello:input.name}));");
    let mut config = p.config();
    config.resources.token_limit = Some(100);
    let run = p.run(&config);
    assert_eq!(
        serde_json::from_str::<Value>(run.stdout.trim()).unwrap(),
        json!({"hello":"Ada"})
    );
    let child = p.child(&run.id);
    assert_eq!(child.status, "done");
    fs::remove_file(p.0.join("child.js")).unwrap();
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
    let db = rusqlite::Connection::open(p.journal()).unwrap();
    let seq = Journal::open(&p.journal())
        .unwrap()
        .operations(&run.id)
        .unwrap()
        .iter()
        .find(|o| o.name == "agent.run")
        .unwrap()
        .seq;
    db.execute(
        "DELETE FROM operations WHERE run_id=?1 AND seq>?2",
        rusqlite::params![run.id, seq],
    )
    .unwrap();
    db.execute("UPDATE operations SET state='pending',result=NULL,completion_order=NULL WHERE run_id=?1 AND seq=?2",rusqlite::params![run.id,seq]).unwrap();
    db.execute(
        "UPDATE runs SET status='running',stdout='' WHERE id=?1",
        [&run.id],
    )
    .unwrap();
    db.execute("UPDATE resource_reservations SET state='active',tokens=100,micros=1000 WHERE run_id=?1 AND seq=?2",rusqlite::params![run.id,seq]).unwrap();
    drop(db);
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
    assert_eq!(p.child(&run.id).id, child.id);
    assert_eq!(
        Journal::open(&p.journal())
            .unwrap()
            .resource_report(&run.id)
            .unwrap()["reservations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        Journal::open(&p.journal())
            .unwrap()
            .attempts(&child.id)
            .unwrap()
            .len(),
        0
    );
}
#[test]
fn child_approval_parks_parent_and_parent_wakes_after_child_finishes() {
    let p=Project::new(PARENT,"import {agent} from 'ondure';export default agent('child',async(ctx,input)=>{const answer=await ctx.human.approve({action:input.name});return {approved:answer.approved};});");
    let run = p.run(&p.config());
    assert_eq!(run.status, "parked");
    let child = p.child(&run.id);
    assert_eq!(child.status, "parked");
    let journal = Journal::open(&p.journal()).unwrap();
    let wait = journal.waits(&child.id).unwrap()[0].seq;
    assert!(journal
        .waits(&run.id)
        .unwrap()
        .iter()
        .any(|w| w.kind == "child"));
    fs::remove_file(p.0.join("child.js")).unwrap();
    fs::remove_file(p.0.join("main.js")).unwrap();
    approve_run(&p.journal(), &child.id, wait, true).unwrap();
    let outcomes = wake_ready(&p.journal()).unwrap();
    assert!(outcomes
        .iter()
        .any(|o| o.run_id == run.id && o.result.as_ref().is_ok_and(|r| r.status == "done")));
    assert_eq!(
        Journal::open(&p.journal())
            .unwrap()
            .get_run(&run.id)
            .unwrap()
            .stdout,
        "{\"approved\":true}\n"
    );
}
#[test]
fn child_authority_is_intersection_of_parent_and_configured_delegation() {
    let witness = support::Witness::new(200, "{}");
    let origin = url::Url::parse(&witness.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    let child=format!("import {{agent}} from 'ondure';export default agent('child',{{capabilities:{{network_origins:[{origin:?}]}}}},async()=>{{try{{await fetch({url:?});return 'bad';}}catch(e){{return 'denied';}}}});",url=witness.url());
    let p = Project::new(PARENT, &child);
    let mut config = p.config();
    config.grants.network_origins.insert(origin.clone());
    config
        .agents
        .get_mut("worker")
        .unwrap()
        .capabilities
        .network_origins
        .insert(origin);
    let run = p.run(&config);
    assert_eq!(run.stdout, "\"denied\"\n");
    assert_eq!(witness.finish(), 0);
}
#[test]
fn missing_child_grants_and_module_escape_are_denied() {
    let p = Project::new(
        PARENT,
        "import {agent} from 'ondure';export default agent('child',()=>1);",
    );
    let mut config = p.config();
    config.grants.child_agents.clear();
    assert!(run_request_with_config(
        &config,
        &p.graph(),
        &p.journal(),
        "denied",
        "null",
        RunLimits::default()
    )
    .is_err());
    assert!(Journal::open(&p.journal())
        .unwrap()
        .find_request("child:missing:0")
        .unwrap()
        .is_none());
    let mut config = p.config();
    config.agents.get_mut("worker").unwrap().module = "../outside.js".into();
    assert!(run_request_with_config(
        &config,
        &p.graph(),
        &p.journal(),
        "escape",
        "null",
        RunLimits::default()
    )
    .is_err());
}
#[test]
fn child_failures_reject_parent_and_non_json_returns_fail() {
    for child in ["import {agent} from 'ondure';export default agent('child',()=>{throw Error('child boom')});","import {agent} from 'ondure';export default agent('child',()=>{const x={};x.x=x;return x;});"]{let p=Project::new(PARENT,child);assert!(run_request_with_config(&p.config(),&p.graph(),&p.journal(),"parent","null",RunLimits::default()).is_err());}
}

#[test]
fn child_allocations_are_reserved_before_launch_and_reconciled_from_child_usage() {
    let witness = support::Witness::new(
        200,
        r#"{"choices":[{"message":{"content":"hello"}}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
    );
    let origin = url::Url::parse(&witness.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    let parent = format!("import {{agent}} from 'ondure';export default agent('parent',{{capabilities:{{child_agents:['worker'],network_origins:[{origin:?}]}}}},async ctx=>console.log(await ctx.agents.run('worker',null)));");
    let child = format!("import {{agent}} from 'ondure';export default agent('worker',{{capabilities:{{network_origins:[{origin:?}]}}}},async ctx=>(await ctx.model.call('hello',{{max_output_tokens:4}})).text);");
    let p = Project::new(&parent, &child);
    let mut config = p.config();
    config.grants.network_origins.insert(origin.clone());
    config
        .agents
        .get_mut("worker")
        .unwrap()
        .capabilities
        .network_origins
        .insert(origin);
    config.agents.get_mut("worker").unwrap().allocation = ChildAllocation {
        tokens: 5,
        dollar_micros: 5_000_000,
    };
    config.resources.token_limit = Some(5);
    config.resources.dollar_limit_micros = Some(5_000_000);
    config.resources.model_input_token_estimate = 1;
    config.resources.input_price_micros_per_million_tokens = 1_000_000_000_000;
    config.resources.output_price_micros_per_million_tokens = 1_000_000_000_000;
    config.model = Some(runtime::ModelConfig {
        endpoint: witness.url(),
        model: "fixture".into(),
        credential: None,
        max_output_tokens: 4,
        retry_interrupted: false,
    });
    let run = p.run(&config);
    assert_eq!(run.stdout, "hello\n");
    let journal = Journal::open(&p.journal()).unwrap();
    let child = p.child(&run.id);
    let parent_usage = journal.resource_report(&run.id).unwrap();
    let child_usage = journal.resource_report(&child.id).unwrap();
    assert_eq!(parent_usage["reserved_tokens"], 2);
    assert_eq!(parent_usage["reserved_dollar_micros"], 2_000_000);
    assert_eq!(child_usage["reserved_tokens"], 2);
    assert_eq!(child_usage["reserved_dollar_micros"], 2_000_000);
    let snapshot: Value = serde_json::from_str(&child.graph).unwrap();
    assert_eq!(snapshot["host_config"]["resources"]["token_limit"], 5);
    let denied = Project::new(&parent, &child_source(&child.graph)); // capture another independent parent with the same module source
    config.resources.token_limit = Some(4);
    assert!(run_request_with_config(
        &config,
        &denied.graph(),
        &denied.journal(),
        "denied",
        "null",
        RunLimits::default()
    )
    .is_err());
    let overflow_source=format!("import {{agent}} from 'ondure';export default agent('worker',{{capabilities:{{network_origins:[{origin:?}]}}}},async ctx=>{{await ctx.model.call('first',{{max_output_tokens:4}});await ctx.model.call('second',{{max_output_tokens:4}});}});",origin=url::Url::parse(&witness.url()).unwrap().origin().ascii_serialization());
    let overflow = Project::new(&parent, &overflow_source);
    config.resources.token_limit = Some(5);
    let error = run_request_with_config(
        &config,
        &overflow.graph(),
        &overflow.journal(),
        "overflow",
        "null",
        RunLimits::default(),
    )
    .unwrap_err();
    assert!(error.message.contains("token reservation exhausted"));
    assert_eq!(witness.finish(), 2);
}
fn child_source(snapshot: &str) -> String {
    let snapshot: Value = serde_json::from_str(snapshot).unwrap();
    snapshot["graph"]["modules"]["child.js"]["source"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn parked_child_retains_allocation_and_inherits_exact_parent_deadline() {
    let p=Project::new(PARENT,"import {agent} from 'ondure';export default agent('worker',async ctx=>ctx.human.approve({action:'review'}));");
    let mut config = p.config();
    config.resources.token_limit = Some(100);
    config.resources.elapsed_deadline_ms = Some(10_000);
    let run = p.run(&config);
    let child = p.child(&run.id);
    let journal = Journal::open(&p.journal()).unwrap();
    let parent_report = journal.resource_report(&run.id).unwrap();
    let child_report = journal.resource_report(&child.id).unwrap();
    assert_eq!(parent_report["reserved_tokens"], 100);
    assert_eq!(parent_report["deadline_at"], child_report["deadline_at"]);
    let db = rusqlite::Connection::open(p.journal()).unwrap();
    db.execute(
        "UPDATE run_resources SET deadline_at=deadline_at+3600000 WHERE run_id=?1",
        [&child.id],
    )
    .unwrap();
    db.execute("UPDATE runs SET status='running' WHERE id=?1", [&child.id])
        .unwrap();
    drop(db);
    replay_run(&p.journal(), &child.id, None).unwrap();
    assert_eq!(
        journal.resource_report(&child.id).unwrap()["deadline_at"],
        parent_report["deadline_at"]
    );
    let approval = journal.waits(&child.id).unwrap()[0].seq;
    approve_run(&p.journal(), &child.id, approval, true).unwrap();
    replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(
        Journal::open(&p.journal())
            .unwrap()
            .resource_report(&run.id)
            .unwrap()["reserved_tokens"],
        100
    );
}

#[test]
fn old_agent_snapshots_without_return_capture_marker_replay_unchanged() {
    let p = Project::new(
        "import {agent} from 'ondure';export default agent('old',()=>console.log('legacy'));",
        "import {agent} from 'ondure';export default agent('unused',()=>null);",
    );
    let run = p.run(&HostConfig::default());
    let mut snapshot: Value = serde_json::from_str(&run.graph).unwrap();
    snapshot.as_object_mut().unwrap().remove("agent_returns");
    let db = rusqlite::Connection::open(p.journal()).unwrap();
    db.execute(
        "DELETE FROM operations WHERE run_id=?1 AND name='agent.return'",
        [&run.id],
    )
    .unwrap();
    db.execute(
        "UPDATE runs SET graph=?2 WHERE id=?1",
        rusqlite::params![run.id, snapshot.to_string()],
    )
    .unwrap();
    drop(db);
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        "legacy\n"
    );
}

#[test]
fn child_process_worker() {
    let Ok(root) = std::env::var("ONDURE_CHILD_TEST_PROJECT") else {
        return;
    };
    let p = Project(PathBuf::from(root));
    let config: HostConfig =
        serde_json::from_slice(&fs::read(p.0.join("config.json")).unwrap()).unwrap();
    p.run(&config);
    std::mem::forget(p);
}

#[test]
fn killed_parent_rejoins_completed_child_without_repeating_effect() {
    let witness = support::Witness::new(200, "{}");
    let origin = url::Url::parse(&witness.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    let parent=format!("import {{agent}} from 'ondure';export default agent('parent',{{capabilities:{{child_agents:['worker'],network_origins:[{origin:?}]}}}},async ctx=>{{const output=await ctx.agents.run('worker',null);await new Promise(resolve=>setTimeout(resolve,500));console.log(output);}});");
    let child_source=format!("import {{agent}} from 'ondure';export default agent('worker',{{capabilities:{{network_origins:[{origin:?}]}}}},async()=>{{await fetch({url:?});return 'ok';}});",url=witness.url());
    let p = Project::new(&parent, &child_source);
    let mut config = p.config();
    config.grants.network_origins.insert(origin.clone());
    config
        .agents
        .get_mut("worker")
        .unwrap()
        .capabilities
        .network_origins
        .insert(origin);
    fs::write(
        p.0.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let mut process = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_process_worker", "--nocapture"])
        .env("ONDURE_CHILD_TEST_PROJECT", &p.0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut identities = None;
    for _ in 0..1000 {
        if let Ok(journal) = Journal::open(&p.journal()) {
            if let Some(parent) = journal.find_request("parent").unwrap() {
                let launch = journal
                    .operations(&parent.id)
                    .unwrap()
                    .into_iter()
                    .find(|o| o.name == "agent.run");
                if let Some(launch) = launch {
                    if let Some(child) = journal
                        .find_request(&format!("child:{}:{}", parent.id, launch.seq))
                        .unwrap()
                    {
                        if child.status == "done" {
                            identities = Some((parent.id, child.id));
                            break;
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let (parent_id, child_id) = identities.expect("child completed before process kill");
    process.kill().unwrap();
    process.wait().unwrap();
    fs::remove_file(p.0.join("child.js")).unwrap();
    fs::remove_file(p.0.join("main.js")).unwrap();
    assert_eq!(
        replay_run(&p.journal(), &parent_id, None).unwrap().stdout,
        "ok\n"
    );
    assert_eq!(p.child(&parent_id).id, child_id);
    assert_eq!(witness.finish(), 1);
}
