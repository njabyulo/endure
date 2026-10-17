use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Project(PathBuf);
impl Project {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "ondure-supervisor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("main.js"), source).unwrap();
        fs::write(path.join("ondure.yaml"), "permissions: [net]").unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ondure"));
        command
            .current_dir(&self.0)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
    fn lease(&self) -> File {
        let directory = self.0.join(".ondure/journal.sqlite3.leases");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(mut entries) = fs::read_dir(&directory) {
                if let Some(Ok(entry)) = entries.next() {
                    let file = File::open(entry.path()).unwrap();
                    match file.try_lock() {
                        Err(std::fs::TryLockError::WouldBlock) => return file,
                        Ok(()) => file.unlock().unwrap(),
                        Err(error) => panic!("cannot probe lease: {error}"),
                    }
                }
            }
            assert!(
                Instant::now() < deadline,
                "worker did not acquire run lease"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn generated_run_id_is_forwarded_before_worker_exits() {
    let project = Project::new("await new Promise(resolve => setTimeout(resolve, 60000));");
    let mut parent = Process(project.command().args(["run", "main.js"]).spawn().unwrap());
    let stderr = parent.0.stderr.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let line = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(line.starts_with("Run ID: "), "{line}");
    assert!(
        parent.0.try_wait().unwrap().is_none(),
        "ID arrived only after exit"
    );
    let lease = project.lease();
    assert!(lease.try_lock().is_err(), "worker should hold its lease");
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while lease.try_lock().is_err() {
        assert!(
            Instant::now() < deadline,
            "orphan worker retained run lease"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().unwrap();
}

#[test]
fn killed_supervisor_stops_worker_before_later_effects() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/hit", listener.local_addr().unwrap());
    let project = Project::new(&format!("await fetch('{url}'); await fetch('{url}');"));
    let mut parent = Process(
        project
            .command()
            .args(["run", "main.js", "--run-id", "orphan"])
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "worker never made first request");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error}"),
        }
    };
    let lease = project.lease();
    assert!(lease.try_lock().is_err());
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while lease.try_lock().is_err() {
        assert!(
            Instant::now() < deadline,
            "orphan retained lease while HTTP was pending"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ =
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    drop(stream);
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "orphan performed another network effect"
    );
}

#[test]
fn parent_eof_before_worker_start_prevents_user_code() {
    let project = Project::new("console.log('must not run');");
    let mut child = Process(
        project
            .command()
            .args(["__worker", "__parent-watch", "run", "main.js"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "worker with no parent did not exit"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut output = Vec::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert!(!status.success());
    assert!(output.is_empty());
    assert!(!project.0.join(".ondure/journal.sqlite3").exists());
}
