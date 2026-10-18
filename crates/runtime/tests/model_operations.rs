use runtime::{
    replay_run, run_request_with_config, HostConfig, Journal, ModelConfig, ModuleGraph, RunLimits,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
struct Fixture {
    endpoint: String,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(status: u16, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (h, r, s) = (hits.clone(), requests.clone(), stop.clone());
        let worker = thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 4096];
                    loop {
                        let n = stream.read(&mut buffer).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..n]);
                        if let Some(end) = request.windows(4).position(|b| b == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&request[..end]);
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length: ")
                                        .and_then(|n| n.parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if request.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    r.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&request).to_string());
                    h.fetch_add(1, Ordering::SeqCst);
                    if status == 0 {
                        while !s.load(Ordering::SeqCst) {
                            thread::sleep(Duration::from_millis(2));
                        }
                        continue;
                    }
                    let response=format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nLocation: http://127.0.0.1:9/forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if s.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(e) => panic!("{e}"),
            }
        });
        Self {
            endpoint,
            hits,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn success() -> Self {
        Self::new(200,json!({"choices":[{"message":{"content":"Hello Ada"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}).to_string())
    }
    fn origin(&self) -> String {
        url::Url::parse(&self.endpoint)
            .unwrap()
            .origin()
            .ascii_serialization()
    }
    fn config(&self) -> HostConfig {
        HostConfig {
            grants: runtime::Capabilities {
                network_origins: [self.origin()].into(),
                ..Default::default()
            },
            model: Some(ModelConfig {
                endpoint: self.endpoint.clone(),
                model: "fixture-model".into(),
                credential: None,
                max_output_tokens: 64,
                retry_interrupted: false,
            }),
            ..Default::default()
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
    }
}
struct Project(PathBuf);
impl Project {
    fn new(origin: &str, call: &str) -> Self {
        let p = std::env::temp_dir().join(format!("ondure-model-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        fs::write(p.join("main.js"),format!("import {{agent}} from 'ondure'; export default agent('model',{{capabilities:{{network_origins:[{origin:?}]}}}},async ctx=>{{const result=await {call};console.log(JSON.stringify(result));}});")).unwrap();
        Self(p)
    }
    fn graph(&self) -> ModuleGraph {
        ModuleGraph::load(&self.0.join("main.js")).unwrap()
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
    fn run(&self, config: &HostConfig) -> runtime::RunRecord {
        run_request_with_config(
            config,
            &self.graph(),
            &self.journal(),
            "request",
            "null",
            RunLimits::default(),
        )
        .unwrap()
    }
    fn simulate_uncommitted(&self, id: &str, attempt: bool) {
        let db = rusqlite::Connection::open(self.journal()).unwrap();
        db.execute(
            "UPDATE runs SET status='running',stdout='' WHERE id=?1",
            [id],
        )
        .unwrap();
        db.execute("UPDATE operations SET state='pending',result=NULL,completion_order=NULL WHERE run_id=?1 AND name='model.call'",[id]).unwrap();
        if attempt {
            db.execute(
                "UPDATE operation_attempts SET state='dispatched',outcome=NULL WHERE run_id=?1",
                [id],
            )
            .unwrap();
        } else {
            db.execute("DELETE FROM operation_attempts WHERE run_id=?1", [id])
                .unwrap();
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn model_normalizes_usage_commits_before_delivery_and_replays_without_provider() {
    let fixture = Fixture::success();
    let p = Project::new(
        &fixture.origin(),
        "ctx.model.call({prompt:'Hello',max_output_tokens:8})",
    );
    let config = fixture.config();
    let run = p.run(&config);
    let result: Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(result["text"], "Hello Ada");
    assert_eq!(result["usage"]["total_tokens"], 5);
    let requests = fixture.requests.lock().unwrap();
    let body = requests[0].split("\r\n\r\n").nth(1).unwrap();
    let request: Value = serde_json::from_str(body).unwrap();
    assert_eq!(request["max_tokens"], 8);
    assert_eq!(request["stream"], false);
    assert_eq!(request["model"], "fixture-model");
    drop(requests);
    let journal = Journal::open(&p.journal()).unwrap();
    assert!(journal
        .operations(&run.id)
        .unwrap()
        .iter()
        .any(|o| o.name == "model.call" && o.state == "completed"));
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
}
#[test]
fn messages_and_prompt_overloads_and_unknown_usage() {
    for call in [
        "ctx.model.call('Hi',{max_output_tokens:8})",
        "ctx.model.call([{role:'user',content:'Hi'}])",
    ] {
        let fixture = Fixture::new(
            200,
            json!({"choices":[{"message":{"content":"ok"}}]}).to_string(),
        );
        let p = Project::new(&fixture.origin(), call);
        let run = p.run(&fixture.config());
        let result: Value = serde_json::from_str(run.stdout.trim()).unwrap();
        assert!(result["usage"].is_null());
    }
}
#[test]
fn missing_grants_and_bad_arguments_never_dispatch() {
    let fixture = Fixture::success();
    let p = Project::new(&fixture.origin(), "ctx.model.call('hello')");
    let mut config = fixture.config();
    config.grants.network_origins.clear();
    assert!(run_request_with_config(
        &config,
        &p.graph(),
        &p.journal(),
        "denied",
        "null",
        RunLimits::default()
    )
    .is_err());
    for call in [
        "ctx.model.call({prompt:'x',max_output_tokens:65})",
        "ctx.model.call({prompt:'x',messages:[]})",
        "ctx.model.call([{role:'tool',content:'x'}])",
        "ctx.model.call({prompt:'x',stream:true})",
    ] {
        let p = Project::new(&fixture.origin(), call);
        assert!(run_request_with_config(
            &fixture.config(),
            &p.graph(),
            &p.journal(),
            "invalid",
            "null",
            RunLimits::default()
        )
        .is_err());
    }
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 0);
}
#[test]
fn malformed_provider_results_and_redirects_are_recorded_errors() {
    for (status, body) in [
        (200, "{}".into()),
        (
            200,
            json!({"choices":[{"message":{"content":null}}]}).to_string(),
        ),
        (
            200,
            json!({"choices":[{"message":{"content":"x"}}],"usage":{"total_tokens":-1}})
                .to_string(),
        ),
        (302, "".into()),
        (500, "secret provider diagnostic".into()),
    ] {
        let f = Fixture::new(status, body);
        let p = Project::new(&f.origin(), "ctx.model.call('Hello')");
        let e = run_request_with_config(
            &f.config(),
            &p.graph(),
            &p.journal(),
            "invalid",
            "null",
            RunLimits::default(),
        )
        .unwrap_err();
        assert!(!e.to_string().contains("secret provider diagnostic"));
        let journal = Journal::open(&p.journal()).unwrap();
        let run = journal.find_request("invalid").unwrap().unwrap();
        let op = journal
            .operations(&run.id)
            .unwrap()
            .into_iter()
            .find(|o| o.name == "model.call")
            .unwrap();
        assert!(op.result.unwrap().contains("Err"));
        assert!(replay_run(&p.journal(), &run.id, None).is_err());
        assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn unfinished_model_dispatch_requires_explicit_retry_but_unissued_intent_resumes() {
    let f = Fixture::success();
    let p = Project::new(&f.origin(), "ctx.model.call('Hello')");
    let run = p.run(&f.config());
    p.simulate_uncommitted(&run.id, true);
    let e = replay_run(&p.journal(), &run.id, None).unwrap_err();
    assert!(e.message.contains("InterruptedError"));
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    let p = Project::new(&f.origin(), "ctx.model.call('Hello')");
    let run = p.run(&f.config());
    p.simulate_uncommitted(&run.id, false);
    replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(f.hits.load(Ordering::SeqCst), 3);
    let p = Project::new(&f.origin(), "ctx.model.call('Hello')");
    let mut config = f.config();
    config.model.as_mut().unwrap().retry_interrupted = true;
    let run = p.run(&config);
    p.simulate_uncommitted(&run.id, true);
    replay_run(&p.journal(), &run.id, None).unwrap();
    assert_eq!(f.hits.load(Ordering::SeqCst), 5);
}
#[test]
fn operator_model_config_rejects_unsafe_endpoints_and_invalid_limits() {
    for endpoint in [
        "file:///etc/passwd",
        "https://secret@example.com/v1/chat/completions",
        "https://example.com/v1/chat/completions?api_key=x",
    ] {
        let config = ModelConfig {
            endpoint: endpoint.into(),
            model: "x".into(),
            credential: None,
            max_output_tokens: 1,
            retry_interrupted: false,
        };
        assert!(config.validate().is_err());
    }
    assert!(HostConfig::from_yaml("model: {endpoint: 'https://example.com/v1/chat/completions', model: fixture, credential: missing, max_output_tokens: 8}").is_err());
}

#[test]
fn model_process_worker() {
    let Ok(root) = std::env::var("ONDURE_MODEL_TEST_PROJECT") else {
        return;
    };
    let p = Project(PathBuf::from(root));
    let config: HostConfig =
        serde_json::from_slice(&fs::read(p.0.join("config.json")).unwrap()).unwrap();
    p.run(&config);
    std::mem::forget(p);
}

#[test]
fn credentials_resolve_only_in_host_and_are_absent_from_recorded_operations() {
    let fixture = Fixture::success();
    let p = Project::new(&fixture.origin(), "ctx.model.call('Hello')");
    let mut config = fixture.config();
    config
        .credentials
        .insert("provider".into(), "ONDURE_TEST_MODEL_SECRET".into());
    config.model.as_mut().unwrap().credential = Some("provider".into());
    fs::write(
        p.0.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let secret = "fixture-private-token-do-not-record";
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "model_process_worker", "--nocapture"])
        .env("ONDURE_MODEL_TEST_PROJECT", &p.0)
        .env("ONDURE_TEST_MODEL_SECRET", secret)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture.requests.lock().unwrap()[0].contains(&format!("Authorization: Bearer {secret}"))
    );
    let journal = Journal::open(&p.journal()).unwrap();
    let run = journal.find_request("request").unwrap().unwrap();
    assert!(!run.graph.contains(secret));
    assert!(!run.stdout.contains(secret));
    assert!(
        !serde_json::to_string(&journal.operations(&run.id).unwrap())
            .unwrap()
            .contains(secret)
    );
}

#[test]
fn killed_model_request_does_not_automatically_dispatch_again() {
    let fixture = Fixture::new(0, String::new());
    let p = Project::new(&fixture.origin(), "ctx.model.call('Hello')");
    fs::write(
        p.0.join("config.json"),
        serde_json::to_vec(&fixture.config()).unwrap(),
    )
    .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "model_process_worker", "--nocapture"])
        .env("ONDURE_MODEL_TEST_PROJECT", &p.0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..200 {
        if fixture.hits.load(Ordering::SeqCst) == 1 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
    child.kill().unwrap();
    child.wait().unwrap();
    let journal = Journal::open(&p.journal()).unwrap();
    let run = journal.find_request("request").unwrap().unwrap();
    assert_eq!(journal.attempts(&run.id).unwrap()[0].state, "dispatched");
    let error = replay_run(&p.journal(), &run.id, None).unwrap_err();
    assert!(error.message.contains("InterruptedError"));
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
}

fn assert_oversized_model_response_is_committed(body: String, expected_error: &str) {
    let fixture = Fixture::new(200, body);
    let project = Project::new(&fixture.origin(), "ctx.model.call('Hello')");
    let error = run_request_with_config(
        &fixture.config(),
        &project.graph(),
        &project.journal(),
        "oversized-response",
        "null",
        RunLimits::default(),
    )
    .unwrap_err();
    assert!(error.message.contains(expected_error), "{error}");
    assert!(error.output.stdout.is_empty());
    let journal = Journal::open(&project.journal()).unwrap();
    let run = journal.find_request("oversized-response").unwrap().unwrap();
    assert_eq!(run.status, "failed");
    let operation = journal
        .operations(&run.id)
        .unwrap()
        .into_iter()
        .find(|operation| operation.name == "model.call")
        .unwrap();
    assert_eq!(operation.state, "completed");
    let encoded = operation.result.unwrap();
    assert!(
        encoded.len() < 1024,
        "oversized payload entered the journal"
    );
    let recorded: Result<Value, String> = serde_json::from_str(&encoded).unwrap();
    assert!(recorded.unwrap_err().contains(expected_error));
    let attempts = journal.attempts(&run.id).unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].state, "completed");
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
    let replayed = replay_run(&project.journal(), &run.id, None).unwrap_err();
    assert_eq!(replayed.message, error.message);
    assert!(replayed.output.stdout.is_empty());
    assert_eq!(fixture.hits.load(Ordering::SeqCst), 1);
}

#[test]
fn oversized_raw_provider_response_commits_bounded_error_and_replays_without_dispatch() {
    let text = "x".repeat(runtime::MAX_OPERATION_RESULT_BYTES + 1);
    let body = json!({"choices":[{"message":{"content":text}}]}).to_string();
    assert!(body.len() > runtime::MAX_OPERATION_RESULT_BYTES);
    assert_oversized_model_response_is_committed(body, "model response exceeds result limit");
}

#[test]
fn normalized_result_envelope_limit_applies_when_provider_body_fits() {
    let empty = json!({"choices":[{"message":{"content":""}}]}).to_string();
    let text = "x".repeat(runtime::MAX_OPERATION_RESULT_BYTES - empty.len());
    let body = json!({"choices":[{"message":{"content":text}}]}).to_string();
    assert_eq!(body.len(), runtime::MAX_OPERATION_RESULT_BYTES);
    let normalized: Result<Value, String> = Ok(json!({
        "text": text,
        "usage": null,
        "finish_reason": null,
    }));
    assert!(serde_json::to_vec(&normalized).unwrap().len() > runtime::MAX_OPERATION_RESULT_BYTES);
    assert_oversized_model_response_is_committed(body, "host operation result exceed");
}
