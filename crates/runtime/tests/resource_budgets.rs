use runtime::{
    replay_run, run_request_with_config, wake_ready, Capabilities, HostConfig, Journal,
    ModelConfig, ModuleGraph, RateLimit, ResourceLimits, RunLimits,
};
use serde_json::json;
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
    time::{Duration, Instant},
};
struct Fixture {
    origin: String,
    endpoint: String,
    hits: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    times: Arc<Mutex<Vec<Instant>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(delay: u64, known: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let endpoint = format!("{origin}/v1/chat/completions");
        let hits = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let times = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (h, a, m, t, s) = (
            hits.clone(),
            active,
            max_active.clone(),
            times.clone(),
            stop.clone(),
        );
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (h, a, m, t) = (h.clone(), a.clone(), m.clone(), t.clone());
                        workers.push(thread::spawn(move||{stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();let mut request=Vec::new();let mut bytes=[0;4096];loop{let n=stream.read(&mut bytes).unwrap_or(0);if n==0{break}request.extend_from_slice(&bytes[..n]);if let Some(end)=request.windows(4).position(|p|p==b"\r\n\r\n"){let len=String::from_utf8_lossy(&request[..end]).lines().find_map(|l|l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|n|n.parse::<usize>().ok())).unwrap_or(0);if request.len()>=end+4+len{break}}}
  h.fetch_add(1,Ordering::SeqCst);t.lock().unwrap().push(Instant::now());let current=a.fetch_add(1,Ordering::SeqCst)+1;m.fetch_max(current,Ordering::SeqCst);thread::sleep(Duration::from_millis(delay));
  let mut body=json!({"choices":[{"message":{"content":"fixture"},"finish_reason":"stop"}]});if known{body["usage"]=json!({"prompt_tokens":2,"completion_tokens":3,"total_tokens":5})}let body=body.to_string();let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());a.fetch_sub(1,Ordering::SeqCst);
  }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            origin,
            endpoint,
            hits,
            max_active,
            times,
            stop,
            worker: Some(worker),
        }
    }
    fn config(&self, resources: ResourceLimits) -> HostConfig {
        HostConfig {
            grants: Capabilities {
                network_origins: [self.origin.clone()].into(),
                ..Default::default()
            },
            model: Some(ModelConfig {
                endpoint: self.endpoint.clone(),
                model: "fixture".into(),
                credential: None,
                max_output_tokens: 10,
                retry_interrupted: false,
            }),
            resources,
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
    fn new(origin: &str, body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ondure-budget-api-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("main.js"),format!("import {{agent}} from 'ondure';export default agent('budget',{{capabilities:{{network_origins:[{origin:?}]}}}},async ctx=>{{{body}}});")).unwrap();
        Self(dir)
    }
    fn run(&self, config: &HostConfig) -> runtime::RunRecord {
        run_request_with_config(
            config,
            &ModuleGraph::load(&self.0.join("main.js")).unwrap(),
            &self.journal(),
            "request",
            "null",
            RunLimits::default(),
        )
        .unwrap()
    }
    fn journal(&self) -> PathBuf {
        self.0.join("journal.sqlite")
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn concurrent_reservation_exhaustion_is_recorded_before_dispatch_and_replays() {
    let f = Fixture::new(25, false);
    let p=Project::new(&f.origin,"console.log(JSON.stringify(await Promise.allSettled([ctx.model.call('a'),ctx.model.call('b')])));");
    let config = f.config(ResourceLimits {
        token_limit: Some(20),
        model_input_token_estimate: 10,
        ..Default::default()
    });
    let run = p.run(&config);
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    assert!(run.stdout.contains("rejected"));
    let ops = Journal::open(&p.journal())
        .unwrap()
        .operations(&run.id)
        .unwrap();
    assert!(ops.iter().any(|op| op
        .result
        .as_ref()
        .is_some_and(|r| r.contains("reservation exhausted"))));
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
}
#[test]
fn concurrency_rate_and_known_usage_reconcile_without_spinning() {
    let f = Fixture::new(30, true);
    let p=Project::new(&f.origin,"console.log((await Promise.all([ctx.model.call('a'),ctx.model.call('b'),ctx.model.call('c')])).length);");
    let run = p.run(&f.config(ResourceLimits {
        model_concurrency: 1,
        model_rate: Some(RateLimit {
            max_calls: 1,
            window_ms: 80,
        }),
        model_input_token_estimate: 10,
        input_price_micros_per_million_tokens: 1_000_000,
        output_price_micros_per_million_tokens: 2_000_000,
        ..Default::default()
    }));
    assert_eq!(run.stdout, "3\n");
    assert_eq!(f.max_active.load(Ordering::SeqCst), 1);
    let times = f.times.lock().unwrap();
    assert_eq!(times.len(), 3);
    for pair in times.windows(2) {
        assert!(pair[1].duration_since(pair[0]) >= Duration::from_millis(65));
    }
    let conn = rusqlite::Connection::open(p.journal()).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT SUM(tokens),SUM(micros) FROM resource_reservations",
            [],
            |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
        )
        .unwrap(),
        (15, 24)
    );
}
#[test]
fn parked_time_consumes_elapsed_deadline_without_dispatch_after_wake() {
    let f = Fixture::new(0, true);
    let p=Project::new(&f.origin,"try{await ctx.sleep('1h')}catch(e){console.log(e.message)}try{await ctx.model.call('late')}catch(e){console.log(e.message)}");
    let run = p.run(&f.config(ResourceLimits {
        elapsed_deadline_ms: Some(20),
        ..Default::default()
    }));
    assert_eq!(run.status, "parked");
    thread::sleep(Duration::from_millis(30));
    let outcomes = wake_ready(&p.journal()).unwrap();
    assert_eq!(outcomes.len(), 1);
    let done = outcomes.into_iter().next().unwrap().result.unwrap();
    assert_eq!(done.status, "done");
    assert_eq!(
        done.stdout,
        "BudgetError: elapsed deadline exceeded\nBudgetError: elapsed deadline exceeded\n"
    );
    assert_eq!(f.hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        done.stdout
    );
}
#[test]
fn deadline_cancels_delivery_of_inflight_provider_result_but_keeps_reservation() {
    let f = Fixture::new(250, true);
    let p = Project::new(
        &f.origin,
        "try{await ctx.model.call('slow')}catch(e){console.log(e.message)}",
    );
    let started = Instant::now();
    let run = p.run(&f.config(ResourceLimits {
        elapsed_deadline_ms: Some(40),
        model_input_token_estimate: 10,
        ..Default::default()
    }));
    assert!(started.elapsed() < Duration::from_millis(220));
    assert_eq!(run.stdout, "BudgetError: elapsed deadline exceeded\n");
    assert_eq!(f.hits.load(Ordering::SeqCst), 1);
    thread::sleep(Duration::from_millis(270));
    let conn = rusqlite::Connection::open(p.journal()).unwrap();
    assert_eq!(
        conn.query_row("SELECT tokens FROM resource_reservations", [], |r| r
            .get::<_, u64>(0))
            .unwrap(),
        20
    );
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
}

#[test]
fn expired_ordinary_timer_records_failure_and_replays_without_running_callback() {
    let p = Project::new(
        "http://127.0.0.1:9",
        "await new Promise(resolve=>setTimeout(()=>{console.log('late');resolve()},1000));",
    );
    let config = HostConfig {
        resources: ResourceLimits {
            elapsed_deadline_ms: Some(20),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = run_request_with_config(
        &config,
        &ModuleGraph::load(&p.0.join("main.js")).unwrap(),
        &p.journal(),
        "timer",
        "null",
        RunLimits::default(),
    )
    .unwrap_err();
    assert!(error.message.contains("elapsed deadline"));
    assert!(!error.output.stdout.contains("late"));
    let journal = Journal::open(&p.journal()).unwrap();
    let run = journal.find_request("timer").unwrap().unwrap();
    assert_eq!(journal.operations(&run.id).unwrap()[0].state, "completed");
    let replay = replay_run(&p.journal(), &run.id, None).unwrap_err();
    assert!(replay.message.contains("elapsed deadline"));
    assert!(!replay.output.stdout.contains("late"));
}

#[test]
fn deadline_rejection_finally_cancels_visible_timer_and_replays_consistently() {
    let f = Fixture::new(150, true);
    let p=Project::new(&f.origin,"const timer=setTimeout(()=>console.log('unexpected timer'),10000);try{await ctx.model.call('slow')}catch(e){console.log(e.message)}finally{clearTimeout(timer)}");
    let run = p.run(&f.config(ResourceLimits {
        elapsed_deadline_ms: Some(30),
        model_input_token_estimate: 10,
        ..Default::default()
    }));
    assert_eq!(run.status, "done");
    assert_eq!(run.stdout, "BudgetError: elapsed deadline exceeded\n");
    let journal = Journal::open(&p.journal()).unwrap();
    let ops = journal.operations(&run.id).unwrap();
    assert_eq!(ops.iter().filter(|op| op.name == "timer.cancel").count(), 1);
    assert_eq!(
        ops.iter()
            .find(|op| op.name == "timer.sleep")
            .unwrap()
            .result
            .as_deref(),
        Some("{\"canceled\":true}")
    );
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        run.stdout
    );
}

#[test]
fn late_scheduler_wake_rejects_uncommitted_wait_even_if_it_was_due_before_deadline() {
    let p = Project::new(
        "http://127.0.0.1:9",
        "try{await ctx.sleep('10ms');console.log('late success')}catch(e){console.log(e.message)}",
    );
    let config = HostConfig {
        resources: ResourceLimits {
            elapsed_deadline_ms: Some(40),
            ..Default::default()
        },
        ..Default::default()
    };
    let run = p.run(&config);
    assert_eq!(run.status, "parked");
    thread::sleep(Duration::from_millis(60));
    let outcomes = wake_ready(&p.journal()).unwrap();
    let done = outcomes
        .into_iter()
        .find(|outcome| outcome.run_id == run.id)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(done.stdout, "BudgetError: elapsed deadline exceeded\n");
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        done.stdout
    );
}

#[test]
fn previously_committed_wait_answer_replays_after_elapsed_deadline() {
    let p = Project::new(
        "http://127.0.0.1:9",
        "await ctx.sleep('1h');console.log('saved answer')",
    );
    let config = HostConfig {
        resources: ResourceLimits {
            elapsed_deadline_ms: Some(40),
            ..Default::default()
        },
        ..Default::default()
    };
    let run = p.run(&config);
    let mut journal = Journal::open(&p.journal()).unwrap();
    let wait = journal.waits(&run.id).unwrap()[0].seq;
    journal.answer_wait(&run.id, wait, "null").unwrap();
    thread::sleep(Duration::from_millis(60));
    assert_eq!(
        replay_run(&p.journal(), &run.id, None).unwrap().stdout,
        "saved answer\n"
    );
}
