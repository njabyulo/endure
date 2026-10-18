mod support;
use runtime::{
    replay_run, run_durable_with_config, run_request_with_config, Capabilities, HostConfig,
    Journal, ModuleGraph, RunLimits,
};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    thread,
};
use support::Witness;

struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!("ondure-grants-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("main.js"), source).unwrap();
        Self(path)
    }
    fn graph(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.js")).unwrap()
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
    fn run(&self, config: &HostConfig) -> Result<runtime::RunOutput, runtime::HostError> {
        run_durable_with_config(
            config,
            &self.graph(),
            &self.journal(),
            "run",
            RunLimits::default(),
        )
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn granting(url: &str) -> HostConfig {
    HostConfig {
        grants: Capabilities {
            network_origins: [url::Url::parse(url).unwrap().origin().ascii_serialization()].into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn scoped_network_allows_only_the_exact_origin_before_dispatch() {
    let allowed = Witness::new(200, "{}");
    let denied = Witness::new(200, "{}");
    let project = Project::new(&format!("try {{ await fetch('{}'); }} catch (e) {{ console.log(String(e).includes('permission denied')); }} console.log((await fetch('{}')).ok);", denied.url(), allowed.url()));
    let first = project.run(&granting(&allowed.url())).unwrap();
    assert_eq!(first.stdout, "true\ntrue\n");
    assert_eq!(replay_run(&project.journal(), "run", None).unwrap(), first);
    assert_eq!(
        Journal::open(&project.journal())
            .unwrap()
            .attempts("run")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(allowed.finish(), 1);
    assert_eq!(denied.finish(), 0);
}

#[test]
fn missing_scoped_grants_deny_network() {
    let denied = Witness::new(200, "{}");
    let project = Project::new(&format!("await fetch('{}');", denied.url()));
    assert!(project
        .run(&HostConfig::default())
        .unwrap_err()
        .message
        .contains("permission denied"));
    assert_eq!(denied.finish(), 0);
}

#[test]
fn scoped_requests_never_follow_redirects_into_another_origin() {
    let denied = Witness::new(200, "{}");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/redirect", listener.local_addr().unwrap());
    let destination = denied.url();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        write!(stream, "HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let project = Project::new(&format!("await fetch('{url}');"));
    let error = project.run(&granting(&url)).unwrap_err();
    assert!(
        error.message.contains("redirects are disabled"),
        "{}",
        error.message
    );
    server.join().unwrap();
    assert_eq!(denied.finish(), 0);
    assert_eq!(
        replay_run(&project.journal(), "run", None)
            .unwrap_err()
            .message,
        error.message
    );
}

#[test]
fn request_identity_includes_trusted_grants_and_credential_references() {
    let allowed = Witness::new(200, "{}");
    let project = Project::new(&format!(
        "console.log((await fetch('{}')).ok);",
        allowed.url()
    ));
    let mut config = granting(&allowed.url());
    config
        .credentials
        .insert("provider".into(), "ONDURE_TEST_PROVIDER_TOKEN".into());
    let graph = project.graph();
    let first = run_request_with_config(
        &config,
        &graph,
        &project.journal(),
        "request",
        "{}",
        RunLimits::default(),
    )
    .unwrap();
    assert_eq!(
        run_request_with_config(
            &config,
            &graph,
            &project.journal(),
            "request",
            "{}",
            RunLimits::default()
        )
        .unwrap(),
        first
    );
    let mut changed = config.clone();
    changed.grants.tools.insert("payment".into());
    assert!(run_request_with_config(
        &changed,
        &graph,
        &project.journal(),
        "request",
        "{}",
        RunLimits::default()
    )
    .unwrap_err()
    .message
    .contains("conflicts"));
    changed = config.clone();
    changed
        .credentials
        .insert("provider".into(), "OTHER_TOKEN".into());
    assert!(run_request_with_config(
        &changed,
        &graph,
        &project.journal(),
        "request",
        "{}",
        RunLimits::default()
    )
    .unwrap_err()
    .message
    .contains("conflicts"));
    let snapshot: serde_json::Value = serde_json::from_str(
        &Journal::open(&project.journal())
            .unwrap()
            .get_run(&first.id)
            .unwrap()
            .graph,
    )
    .unwrap();
    assert_eq!(
        snapshot["host_config"]["credentials"]["provider"],
        "ONDURE_TEST_PROVIDER_TOKEN"
    );
    assert_eq!(
        snapshot["host_config"]["grants"]["network_origins"][0],
        url::Url::parse(&allowed.url())
            .unwrap()
            .origin()
            .ascii_serialization()
    );
    assert_eq!(allowed.finish(), 1);
}
