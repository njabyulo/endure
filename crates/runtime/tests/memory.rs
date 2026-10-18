use runtime::{
    replay_run, run_request_with_config, Capabilities, ContextLimits, HostConfig, Journal,
    ModuleGraph, RunLimits,
};
use std::{fs, path::PathBuf};
struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ondure-memory-api-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("main.js"), source).unwrap();
        Self(dir)
    }
    fn run(&self, request: &str, config: &HostConfig) -> runtime::RunRecord {
        run_request_with_config(
            config,
            &ModuleGraph::load(&self.0.join("main.js")).unwrap(),
            &self.0.join("journal.sqlite"),
            request,
            "null",
            RunLimits::default(),
        )
        .unwrap()
    }
    fn config() -> HostConfig {
        HostConfig {
            grants: Capabilities {
                memory_scopes: ["team".into()].into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn awaited_native_memory_and_state_replay_without_duplicate_writes() {
    let p = Project::new(
        r#"import {agent} from 'ondure'; export default agent('memory',{capabilities:{memory_scopes:['team']}},async ctx=>{
 console.log((await ctx.context.add({role:'system',content:'instructions'})).messages.length);
 console.log((await ctx.context.messages()).length);
 console.log((await ctx.memory.remember('olive trees',{scope:'team'})).scope);
 console.log((await ctx.memory.recall('olive',{scope:'team'})).length);
 console.log((await ctx.state.get('count',{scope:'team'}))===null);
 console.log((await ctx.state.compareAndSet('count',0,1,{scope:'team'})).updated);
 console.log((await ctx.state.compareAndSet('count',0,9,{scope:'team'})).updated);
 console.log((await ctx.state.set('count',2,{scope:'team'})).version);
 });"#,
    );
    let run = p.run("first", &Project::config());
    assert_eq!(run.stdout, "1\n1\nteam\n1\ntrue\ntrue\nfalse\n2\n");
    // A second run changes shared facts/state, but historical reads are reused.
    let second = p.run("second", &Project::config());
    assert!(second.stdout.contains("team\n2\nfalse\nfalse\nfalse\n3\n"));
    assert_eq!(
        replay_run(&p.0.join("journal.sqlite"), &run.id, None)
            .unwrap()
            .stdout,
        run.stdout
    );
    let conn = rusqlite::Connection::open(p.0.join("journal.sqlite")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT version FROM shared_state", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
}
#[test]
fn scopes_require_both_agent_request_and_operator_grant() {
    let p = Project::new(
        r#"import {agent} from 'ondure'; export default agent('denied',{capabilities:{memory_scopes:['secret']}},async ctx=>{for(const op of [()=>ctx.memory.remember('secret',{scope:'secret'}),()=>ctx.state.set('x',1,{scope:'team'})]) {try{await op();console.log('bad')}catch(e){console.log(e.message.includes('permission denied'))}}});"#,
    );
    assert_eq!(p.run("denied", &Project::config()).stdout, "true\ntrue\n");
    let conn = rusqlite::Connection::open(p.0.join("journal.sqlite")).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM memory_facts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn configured_compaction_and_protected_error_are_durable() {
    let p = Project::new(
        r#"import {agent} from 'ondure'; export default agent('context',async ctx=>{
 for(const [role,content] of [['system','keep'],['user','old'],['assistant','middle'],['user','recent']]) await ctx.context.add({role,content});
 console.log((await ctx.context.messages()).map(m=>m.content).join(','));
 try{await ctx.context.add({role:'system',content:'x'.repeat(1000)})}catch(e){console.log(e.message)}
 console.log((await ctx.context.messages()).length);
 });"#,
    );
    let config = HostConfig {
        context_limits: ContextLimits {
            max_messages: 3,
            max_bytes: 200,
            recent_messages: 1,
        },
        ..Default::default()
    };
    let run = p.run("compact", &config);
    assert_eq!(
        run.stdout,
        "keep,middle,recent\nprotected context exceeds configured limits\n3\n"
    );
    assert_eq!(
        replay_run(&p.0.join("journal.sqlite"), &run.id, None)
            .unwrap()
            .stdout,
        run.stdout
    );
    let ops = Journal::open(&p.0.join("journal.sqlite"))
        .unwrap()
        .operations(&run.id)
        .unwrap();
    assert!(ops.iter().any(|o| o
        .result
        .as_ref()
        .is_some_and(|s| s.contains("protected context"))));
}
#[test]
fn context_configuration_rejects_invalid_limits() {
    assert!(HostConfig::from_yaml("context_limits: {max_messages: 0}").is_err());
    assert!(
        HostConfig::from_yaml("context_limits: {max_messages: 3, recent_messages: 4}").is_err()
    );
    let config = HostConfig::from_yaml("context_limits: {max_messages: 64}").unwrap();
    assert_eq!(config.context_limits.max_bytes, 256 * 1024);
}
