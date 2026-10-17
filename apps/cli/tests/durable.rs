//! Real SIGKILL occurs after an external witness sees an await continuation.
#[cfg(unix)]
#[test]
fn hundred_killed_runs_keep_every_acknowledged_result() {
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        process::{Command, Stdio},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    let root = std::env::temp_dir().join(format!(
        "ondure-crash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("ondure.yaml"), "permissions: [net]").unwrap();
    let journal_path = root.join("journal.sqlite3");
    for iteration in 0..100 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        fs::write(root.join("agent.js"), format!("const result = await (await fetch('{url}/value')).json(); if (result.hit !== 1) throw Error('bad'); await fetch('{url}/ack');")).unwrap();
        let (seen_tx, seen_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            for request in 0..2 {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return;
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => return,
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut headers = Vec::new();
                while !headers.windows(4).any(|p| p == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => headers.extend_from_slice(&buffer[..n]),
                    }
                }
                if request == 0 {
                    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n{\"hit\":1}").unwrap();
                } else {
                    let _ = seen_tx.send(());
                    let _ = stop_rx.recv_timeout(Duration::from_secs(5));
                }
            }
        });
        let run_id = format!("crash-{iteration}");
        let mut child = Command::new(env!("CARGO_BIN_EXE_ondure"))
            .arg("__worker")
            .current_dir(&root)
            .args(["run", "agent.js", "--run-id", &run_id, "--journal"])
            .arg(&journal_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let acknowledged = seen_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        let _ = child.kill();
        child.wait().unwrap();
        let _ = stop_tx.send(());
        server.join().unwrap();
        assert!(
            acknowledged,
            "run {iteration} never reached its await continuation"
        );
        let journal = runtime::Journal::open(&journal_path).unwrap();
        let ops = journal.operations(&run_id).unwrap();
        assert_eq!(
            ops[0].state, "completed",
            "lost acknowledged result in run {iteration}"
        );
        assert!(ops[0].result.as_ref().unwrap().contains("hit"));
        assert_eq!(ops[1].state, "pending");
    }
    fs::remove_dir_all(root).unwrap();
}
