use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ondure-parking-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("agent.ts"), source).unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Output {
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
                panic!("parked process did not exit");
            }
            thread::sleep(Duration::from_millis(2));
        }
        Output {
            status: child.wait().unwrap(),
            stdout: fs::read(self.0.join("stdout")).unwrap(),
            stderr: fs::read(self.0.join("stderr")).unwrap(),
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parked_process_exits_and_another_process_answers_from_stored_code() {
    let project = Project::new("const answer = await ondure.approve({action:'continue'}); console.log(answer ? 'accepted' : 'denied');");
    let output = project.run(&["run", "agent.ts", "--run-id", "approval"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("parked"));
    let journal_path = project.0.join(".ondure/journal.sqlite3");
    let journal = runtime::Journal::open(&journal_path).unwrap();
    assert_eq!(journal.get_run("approval").unwrap().status, "parked");
    let seq = journal.waits("approval").unwrap()[0].seq.to_string();
    fs::remove_file(project.0.join("agent.ts")).unwrap();
    let output = project.run(&["approve", "approval", &seq, "yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"accepted\n");
    assert_eq!(journal.get_run("approval").unwrap().status, "done");
    let output = project.run(&["approve", "approval", &seq, "no"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicting answer"));
}

#[test]
fn request_retries_return_same_run_and_changed_content_conflicts() {
    let project = Project::new("console.log(ondure.input.value);");
    let args = [
        "run",
        "agent.ts",
        "--request",
        "ticket",
        "--input",
        r#"{"value":42}"#,
    ];
    let first = project.run(&args);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = project.run(&args);
    assert_eq!(first.stdout, b"42\n");
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stderr, second.stderr);
    let output = project.run(&[
        "run",
        "agent.ts",
        "--request",
        "ticket",
        "--input",
        r#"{"value":99}"#,
    ]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicts"));
}

#[test]
fn wake_recovers_a_killed_resumption_without_repeating_completed_http() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
    };

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stalled_tx, stalled_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut paths = Vec::new();
        for request in 0..3 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return paths;
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("witness accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut headers = Vec::new();
            while !headers.windows(4).any(|part| part == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return paths,
                    Ok(size) => headers.extend_from_slice(&buffer[..size]),
                }
            }
            paths.push(
                String::from_utf8_lossy(&headers)
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned(),
            );
            if request == 1 {
                let _ = stalled_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            } else {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .unwrap();
            }
        }
        paths
    });
    let project = Project::new(&format!("await fetch('{url}/before'); await ondure.approve({{action:'continue'}}); await fetch('{url}/after'); console.log('finished');"));
    fs::write(project.0.join("ondure.yaml"), "permissions: [net]").unwrap();
    assert!(project
        .run(&["run", "agent.ts", "--run-id", "recover"])
        .status
        .success());
    let path = project.0.join(".ondure/journal.sqlite3");
    let mut journal = runtime::Journal::open(&path).unwrap();
    let seq = journal.waits("recover").unwrap()[0].seq;
    journal.answer_wait("recover", seq, "true").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ondure"))
        .current_dir(&project.0)
        .args(["replay", "recover"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stalled = stalled_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let _ = child.kill();
    child.wait().unwrap();
    let _ = release_tx.send(());
    assert!(
        stalled,
        "resumption did not submit its unfinished HTTP operation"
    );
    assert_eq!(journal.get_run("recover").unwrap().status, "running");
    fs::remove_file(project.0.join("agent.ts")).unwrap();
    let output = project.run(&["wake"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"finished\n");
    assert_eq!(journal.get_run("recover").unwrap().status, "done");
    assert!(project.run(&["wake"]).stdout.is_empty());
    let paths = server.join().unwrap();
    assert_eq!(
        paths,
        [
            "GET /before HTTP/1.1",
            "GET /after HTTP/1.1",
            "GET /after HTTP/1.1"
        ]
    );
}
