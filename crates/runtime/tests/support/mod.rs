use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Witness {
    url: String,
    stop: Arc<AtomicBool>,
    hits: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

impl Witness {
    pub fn new(status: u16, body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/hit", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));
        let worker_stop = Arc::clone(&stop);
        let worker_hits = Arc::clone(&hits);
        let body = body.to_owned();
        let worker = thread::spawn(move || loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    worker_hits.fetch_add(1, Ordering::SeqCst);
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => break,
                            Ok(size) => request.extend_from_slice(&buffer[..size]),
                        }
                    }
                    let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        continue;
                    };
                    let content_length = String::from_utf8_lossy(&request[..header_end])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let received = request.len().saturating_sub(header_end + 4);
                    let mut remaining = content_length.saturating_sub(received);
                    // POST headers and body need not arrive in one read. Closing
                    // with unread request bytes can reset the client connection.
                    while remaining > 0 {
                        let wanted = remaining.min(buffer.len());
                        match stream.read(&mut buffer[..wanted]) {
                            Ok(0) | Err(_) => break,
                            Ok(size) => remaining -= size,
                        }
                    }
                    if remaining != 0 {
                        continue;
                    }
                    let response = format!(
                        "HTTP/1.1 {status} Witness\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    // A connecting client may close early; the socket still counts.
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    // Check stop only after accepting every queued connection.
                    if worker_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("witness accept: {error}"),
            }
        });
        Self {
            url,
            stop,
            hits,
            worker: Some(worker),
        }
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn finish(mut self) -> usize {
        self.join();
        self.hits.load(Ordering::SeqCst)
    }

    fn join(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("witness worker");
        }
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        self.join();
    }
}
