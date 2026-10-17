//! The CLI supervisor must survive limits and leave usable durable records.
use std::{
    fs,
    io::Read,
    net::TcpListener,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Project(PathBuf);

impl Project {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ondure-limits-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("agent.ts"), source).unwrap();
        Self(root)
    }

    fn run(&self, args: &[&str]) -> Output {
        // Files cannot fill a pipe and block a child before its deadline.
        let stdout = fs::File::create(self.0.join("stdout")).unwrap();
        let stderr = fs::File::create(self.0.join("stderr")).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_ondure"))
            .args(args)
            .current_dir(&self.0)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "limited CLI did not exit: {}",
                    String::from_utf8_lossy(&fs::read(self.0.join("stderr")).unwrap())
                );
            }
            thread::sleep(Duration::from_millis(2));
        }
        Output {
            status: child.wait().unwrap(),
            stdout: fs::read(self.0.join("stdout")).unwrap(),
            stderr: fs::read(self.0.join("stderr")).unwrap(),
        }
    }

    fn journal(&self) -> runtime::Journal {
        runtime::Journal::open(&self.0.join(".ondure/journal.sqlite3")).unwrap()
    }

    fn assert_healthy_followup(&self) {
        fs::write(self.0.join("agent.ts"), "console.log('healthy');").unwrap();
        let output = self.run(&["run", "agent.ts", "--run-id", "healthy"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"healthy\n");
        assert_eq!(self.journal().get_run("healthy").unwrap().status, "done");
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cpu_limit_preserves_logs_and_journal_for_a_healthy_followup() {
    let project =
        Project::new("console.log('starting'); console.error('before loop'); while (true) {}");
    let started = Instant::now();
    let output = project.run(&["run", "agent.ts", "--run-id", "cpu", "--cpu-ms", "50"]);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"starting\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("before loop"));
    let failed = project.journal().get_run("cpu").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.stdout, "starting\n");
    assert!(failed.stderr.contains("before loop"));
    project.assert_healthy_followup();
    assert_eq!(project.journal().get_run("cpu").unwrap(), failed);
}

#[test]
fn heap_limit_leaves_the_journal_usable_by_the_next_run() {
    let project = Project::new("console.log('allocating'); const retained = []; while (true) retained.push(new Array(100000).fill(42));");
    let output = project.run(&["run", "agent.ts", "--run-id", "heap", "--heap-mb", "16"]);
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"allocating\n");
    let failed = project.journal().get_run("heap").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.stdout, "allocating\n");
    project.assert_healthy_followup();
    assert_eq!(project.journal().get_run("heap").unwrap(), failed);
}

#[test]
fn cpu_limit_exits_promptly_while_an_allowed_http_worker_is_stalled() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/stalled", listener.local_addr().unwrap());
    let project = Project::new(&format!(
        "fetch('{url}'); setTimeout(() => {{ console.log('loop'); while (true) {{}} }}, 100);"
    ));
    fs::write(project.0.join("ondure.yaml"), "permissions: [net]").unwrap();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let (stop_tx, stop_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut headers = Vec::new();
                    while !headers.windows(4).any(|part| part == b"\r\n\r\n") {
                        let mut buffer = [0; 1024];
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => return 0,
                            Ok(size) => headers.extend_from_slice(&buffer[..size]),
                        }
                    }
                    let _ = accepted_tx.send(());
                    // Keep the HTTP response outstanding until the supervisor has exited.
                    let _ = stop_rx.recv_timeout(Duration::from_secs(5));
                    return 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline || stop_rx.try_recv().is_ok() {
                        return 0;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("stalled witness accept: {error}"),
            }
        }
    });
    let started = Instant::now();
    let output = project.run(&["run", "agent.ts", "--run-id", "stalled", "--cpu-ms", "50"]);
    let elapsed = started.elapsed();
    let accepted = accepted_rx.recv_timeout(Duration::from_secs(1)).is_ok();
    let _ = stop_tx.send(());
    let hits = server.join().unwrap();
    assert!(accepted);
    assert_eq!(hits, 1);
    assert!(
        elapsed < Duration::from_secs(1),
        "CLI waited for stalled HTTP: {elapsed:?}"
    );
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"loop\n");
    assert_eq!(
        project.journal().get_run("stalled").unwrap().status,
        "failed"
    );
}

#[test]
fn console_output_limit_bounds_retained_logs_and_preserves_the_journal() {
    let project =
        Project::new("const chunk='x'.repeat(1023); for(let i=0;i<10000;i++) console.log(chunk);");
    let output = project.run(&["run", "agent.ts", "--run-id", "output"]);
    assert!(!output.status.success());
    assert_eq!(output.stdout.len(), runtime::MAX_OUTPUT_BYTES);
    assert!(String::from_utf8_lossy(&output.stderr).contains("console output limit exceeded"));
    let failed = project.journal().get_run("output").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.stdout.len(), runtime::MAX_OUTPUT_BYTES);
    project.assert_healthy_followup();
}
