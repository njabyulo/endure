mod support;
use runtime::{run_durable_with_config, Capabilities, HostConfig, ModuleGraph, RunLimits};
use std::fs;
use support::Witness;

#[test]
fn agent_requests_cannot_elevate_grants_and_top_level_code_starts_denied() {
    let allowed = Witness::new(200, "{}");
    let denied = Witness::new(200, "{}");
    let root =
        std::env::temp_dir().join(format!("ondure-agent-authority-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let origin = url::Url::parse(&allowed.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    let denied_origin = url::Url::parse(&denied.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    fs::write(root.join("main.js"), format!(r#"
        import {{agent}} from 'ondure';
        try {{ await fetch('{}'); throw Error('top-level allowed'); }} catch(e) {{ if(!String(e).includes('permission denied')) throw e; }}
        export default agent('limited', {{capabilities: {{network_origins: ['{origin}', '{denied_origin}']}}}}, async()=>{{
            try {{await fetch('{}'); throw Error('escalated');}} catch(e) {{if(!String(e).includes('permission denied')) throw e;}}
            console.log((await fetch('{}')).ok);
        }});
    "#, allowed.url(), denied.url(), allowed.url())).unwrap();
    let config = HostConfig {
        grants: Capabilities {
            network_origins: [origin].into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let result = run_durable_with_config(
        &config,
        &ModuleGraph::load(&root.join("main.js")).unwrap(),
        &root.join("journal.sqlite3"),
        "run",
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(result.stdout, "true\n");
    assert_eq!(allowed.finish(), 1);
    assert_eq!(denied.finish(), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn broad_legacy_network_permission_does_not_grant_agent_authority() {
    let denied = Witness::new(200, "{}");
    let root = std::env::temp_dir().join(format!("ondure-agent-legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let origin = url::Url::parse(&denied.url())
        .unwrap()
        .origin()
        .ascii_serialization();
    fs::write(root.join("main.js"), format!("import {{agent}} from 'ondure'; export default agent('limited', {{capabilities: {{network_origins: ['{origin}']}}}}, async()=>await fetch('{}'));", denied.url())).unwrap();
    let error = runtime::run_durable(
        &runtime::Policy { net: true },
        &ModuleGraph::load(&root.join("main.js")).unwrap(),
        &root.join("journal.sqlite3"),
        "run",
    )
    .unwrap_err();
    assert!(error.message.contains("permission denied"));
    assert_eq!(denied.finish(), 0);
    fs::remove_dir_all(root).unwrap();
}
