#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEADLINE: Duration = Duration::from_secs(15);

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ondure-crash-replay-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Drain both pipes immediately, and kill/reap on every early exit or assertion.
struct Process {
    child: Child,
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl Process {
    fn spawn(directory: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ondure"))
            .arg("__worker")
            .current_dir(directory)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        fn drain(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).unwrap();
                bytes
            })
        }
        let stdout = Some(drain(child.stdout.take().unwrap()));
        let stderr = Some(drain(child.stderr.take().unwrap()));
        Self {
            child,
            stdout,
            stderr,
        }
    }

    fn finish(&mut self) -> Output {
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "ondure subprocess timed out");
            thread::sleep(Duration::from_millis(5));
        };
        Output {
            status,
            stdout: self.stdout.take().unwrap().join().unwrap(),
            stderr: self.stderr.take().unwrap().join().unwrap(),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr.take() {
            let _ = reader.join();
        }
    }
}

struct Server {
    address: String,
    second_started: Receiver<()>,
    retry_started: Receiver<()>,
    release: Option<Sender<()>>,
    release_retry: Option<Sender<()>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<(usize, usize)>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (started_tx, second_started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (retry_tx, retry_started) = mpsc::channel();
        let (release_retry, retry_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + DEADLINE;
            let (mut first, mut second) = (0, 0);
            while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept failed: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let path = request_path(&mut stream);
                match path.as_str() {
                    "/first" => {
                        first += 1;
                        respond(&mut stream, r#"{"first":1}"#);
                    }
                    "/second" => {
                        second += 1;
                        if second == 1 {
                            // This request can happen only after the first
                            // response has been journaled and its await resumed.
                            let _ = started_tx.send(());
                            let _ = release_rx.recv_timeout(DEADLINE);
                            // Deliberately lose the response at the crash point.
                        } else {
                            let _ = retry_tx.send(());
                            let _ = retry_rx.recv_timeout(DEADLINE);
                            respond(&mut stream, r#"{"second":2}"#);
                        }
                    }
                    _ => panic!("unexpected request path: {path}"),
                }
            }
            (first, second)
        });
        Self {
            address,
            second_started,
            retry_started,
            release: Some(release),
            release_retry: Some(release_retry),
            stop,
            worker: Some(worker),
        }
    }

    fn release_crashed_request(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }

    fn finish(&mut self) -> (usize, usize) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        if let Some(release) = self.release_retry.take() {
            let _ = release.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn request_path(stream: &mut TcpStream) -> String {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        headers.push(byte[0]);
        assert!(headers.len() < 16 * 1024, "request headers too large");
    }
    String::from_utf8(headers)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned()
}

fn respond(stream: &mut TcpStream, body: &str) {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

#[test]
fn sigkill_then_replay_reuses_completed_get_and_retries_unfinished_get() {
    let directory = Directory::new();
    let mut server = Server::new();
    fs::write(directory.0.join("ondure.yaml"), "permissions: [net]\n").unwrap();
    fs::write(
        directory.0.join("agent.js"),
        format!(
            r#"
        const first = await (await fetch('http://{0}/first')).json();
        const second = await (await fetch('http://{0}/second')).json();
        console.log('finished', first.first, second.second);
    "#,
            server.address
        ),
    )
    .unwrap();
    let mut original = Process::spawn(
        &directory.0,
        &[
            "run",
            "agent.js",
            "--run-id",
            "crash-run",
            "--journal",
            "journal.sqlite3",
        ],
    );
    server
        .second_started
        .recv_timeout(DEADLINE)
        .expect("second request did not begin before deadline");
    for args in [
        vec!["replay", "crash-run", "--journal", "journal.sqlite3"],
        vec![
            "run",
            "agent.js",
            "--run-id",
            "crash-run",
            "--journal",
            "journal.sqlite3",
        ],
    ] {
        let blocked = Process::spawn(&directory.0, &args).finish();
        assert!(!blocked.status.success());
        assert!(String::from_utf8_lossy(&blocked.stderr).contains("already running"));
    }
    original.child.kill().unwrap(); // Child::kill sends SIGKILL on Unix.
    let killed = original.finish();
    assert_eq!(killed.status.signal(), Some(9));
    server.release_crashed_request();

    // Replay must use the stored graph and policy, not mutable disk inputs.
    fs::remove_file(directory.0.join("agent.js")).unwrap();
    fs::remove_file(directory.0.join("ondure.yaml")).unwrap();
    let mut replay = Process::spawn(
        &directory.0,
        &["replay", "crash-run", "--journal", "journal.sqlite3"],
    );
    server
        .retry_started
        .recv_timeout(DEADLINE)
        .expect("replay did not retry unfinished request");
    let blocked = Process::spawn(
        &directory.0,
        &["replay", "crash-run", "--journal", "journal.sqlite3"],
    )
    .finish();
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("already running"));
    server.release_retry.take().unwrap().send(()).unwrap();
    let replay = replay.finish();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(replay.stdout, b"finished 1 2\n");
    assert!(replay.stderr.is_empty());
    assert_eq!(server.finish(), (1, 2));
}
