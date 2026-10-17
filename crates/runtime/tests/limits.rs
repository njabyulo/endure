use runtime::{run_source, run_source_with_limits, Policy, RunLimits};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn budget() -> RunLimits {
    RunLimits {
        cpu_ms: 50,
        heap_mb: 0,
    }
}

#[test]
fn tight_loop_terminates_and_next_run_succeeds() {
    let started = Instant::now();
    let error = run_source_with_limits(
        &Policy::default(),
        "loop.js",
        "console.log('starting'); while (true) {}",
        budget(),
    )
    .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(error.message, "execution time limit exceeded");
    assert_eq!(error.output.stdout, "starting\n");
    assert_eq!(
        run_source(&Policy::default(), "ok.js", "console.log('healthy')")
            .unwrap()
            .stdout,
        "healthy\n"
    );
}

#[test]
fn hostile_microtasks_terminate() {
    let error = run_source_with_limits(
        &Policy::default(),
        "microtasks.js",
        "function spin() { Promise.resolve().then(spin); } spin();",
        budget(),
    )
    .unwrap_err();
    assert_eq!(error.message, "execution time limit exceeded");
}

#[test]
fn waiting_does_not_spend_execution_budget() {
    let output = run_source_with_limits(
        &Policy::default(),
        "wait.js",
        "setTimeout(() => console.log('awake'), 150);",
        budget(),
    )
    .unwrap();
    assert_eq!(output.stdout, "awake\n");
}

#[test]
fn separate_callbacks_share_one_budget() {
    let error = run_source_with_limits(&Policy::default(), "callbacks.js", "function work() { const end = Date.now() + 20; while (Date.now() < end) {} setTimeout(work, 1); } work();", budget()).unwrap_err();
    assert_eq!(error.message, "execution time limit exceeded");
}

#[test]
fn heap_bomb_child() {
    if std::env::var_os("ONDURE_HEAP_CHILD").is_none() {
        return;
    }
    let error = run_source_with_limits(&Policy::default(), "heap.js", "console.log('starting'); const arrays = []; while (true) arrays.push(new Array(10000).fill('allocation'));", RunLimits { cpu_ms: 2000, heap_mb: 16 }).unwrap_err();
    assert_eq!(error.message, "V8 heap limit exceeded");
    assert_eq!(error.output.stdout, "starting\n");
    assert_eq!(
        run_source(&Policy::default(), "ok.js", "console.log('healthy')")
            .unwrap()
            .stdout,
        "healthy\n"
    );
}

#[test]
fn heap_bomb_does_not_kill_host() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "heap_bomb_child", "--nocapture"])
        .env("ONDURE_HEAP_CHILD", "1")
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "heap worker: {status}");
            break;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("heap worker timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn termination_prevents_queued_network_dispatch() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let source = format!(
        "fetch('http://{}/'); while(true) {{}}",
        listener.local_addr().unwrap()
    );
    let error =
        run_source_with_limits(&Policy { net: true }, "queued.js", &source, budget()).unwrap_err();
    assert!(error.message.contains("time limit"));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
