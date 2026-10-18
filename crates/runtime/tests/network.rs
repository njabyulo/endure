mod support;

use runtime::{run_source, Policy};
use support::Witness;

fn denied_source(fetch: &str, url: &str) -> String {
    format!(
        "(async () => {{ try {{ await {fetch}('{url}'); console.log('unexpected-ok'); }} catch (e) {{ console.error(e); }} }})();"
    )
}

#[test]
fn denied_closed_port_reports_permission_not_connection_error() {
    let source = denied_source("fetch", "http://127.0.0.1:1/hit");
    let output = run_source(&Policy { net: false }, "denied.js", &source).unwrap();
    assert!(output.stderr.contains("permission denied: net"));
    assert!(!output.stdout.contains("unexpected-ok"));
}

#[test]
fn denied_fetch_never_opens_live_socket() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = denied_source("fetch", &witness.url());
    let result = run_source(&Policy { net: false }, "denied.js", &source);
    let hits = witness.finish();
    let output = result.unwrap();
    assert!(output.stderr.contains("permission denied: net"));
    assert!(!output.stdout.contains("unexpected-ok"));
    assert_eq!(hits, 0);
}

#[test]
fn computed_fetch_cannot_bypass_policy() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = denied_source("globalThis['fe'+'tch']", &witness.url());
    let result = run_source(&Policy { net: false }, "evade.js", &source);
    let hits = witness.finish();
    let output = result.unwrap();
    assert!(output.stderr.contains("permission denied: net"));
    assert!(!output.stdout.contains("unexpected-ok"));
    assert_eq!(hits, 0);
}

#[test]
fn denial_precedes_url_coercion() {
    let output = run_source(
        &Policy::default(),
        "coerce.js",
        "try { fetch({ toString() { console.log('coerced'); throw Error('wrong'); } }); } catch(e) { console.error(e); }",
    ).unwrap();
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("permission denied: net"));
}

#[test]
fn allowed_fetch_returns_promises_and_parsed_json() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = format!(
        r#"(async () => {{
            const pending = fetch('{}');
            console.log('fetch-promise', pending instanceof Promise);
            const response = await pending;
            console.log('response', response.ok, response.status);
            const body = response.json();
            console.log('json-promise', body instanceof Promise);
            console.log('hit', (await body).hit);
        }})();"#,
        witness.url()
    );
    let result = run_source(&Policy { net: true }, "allow.js", &source);
    let hits = witness.finish();
    let output = result.unwrap();
    assert_eq!(
        output.stdout,
        "fetch-promise true\nresponse true 200\njson-promise true\nhit 1\n"
    );
    assert_eq!(output.stderr, "");
    assert_eq!(hits, 1);
}

#[test]
fn http_error_status_is_a_response() {
    let witness = Witness::new(404, r#"{"error":"missing"}"#);
    let source = format!(
        "(async () => {{ const response = await fetch('{}'); console.log(response.ok, response.status, (await response.json()).error); }})();",
        witness.url()
    );
    let result = run_source(&Policy { net: true }, "status.js", &source);
    let hits = witness.finish();
    assert_eq!(result.unwrap().stdout, "false 404 missing\n");
    assert_eq!(hits, 1);
}

#[test]
fn invalid_json_rejects_and_can_be_caught() {
    let witness = Witness::new(200, "not JSON");
    let source = format!(
        "(async () => {{ const response = await fetch('{}'); const pending = response.json(); console.log(pending instanceof Promise); try {{ await pending; }} catch(e) {{ console.error(e.name); }} }})();",
        witness.url()
    );
    let result = run_source(&Policy { net: true }, "json.js", &source);
    let hits = witness.finish();
    let output = result.unwrap();
    assert_eq!(output.stdout, "true\n");
    assert_eq!(output.stderr, "SyntaxError\n");
    assert_eq!(hits, 1);
}

#[test]
fn allowed_transport_failure_can_be_caught() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hit", listener.local_addr().unwrap());
    drop(listener);
    let output = run_source(
        &Policy { net: true },
        "transport.js",
        &denied_source("fetch", &url),
    )
    .unwrap();
    assert!(output.stderr.contains("fetch failed:"));
    assert!(!output.stdout.contains("unexpected-ok"));
}

#[test]
fn response_creation_ignores_prototype_setters() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = format!(
        "Object.defineProperty(Object.prototype, 'ok', {{ set() {{ throw Error('setter'); }} }}); (async () => {{ const response = await fetch('{}'); console.log(response.ok, (await response.json()).hit); }})();",
        witness.url()
    );
    let result = run_source(&Policy { net: true }, "prototype.js", &source);
    let hits = witness.finish();
    assert_eq!(result.unwrap().stdout, "true 1\n");
    assert_eq!(hits, 1);
}

#[test]
fn transport_failure_returns_promise_and_runs_catch_handler() {
    let output = run_source(
        &Policy { net: true },
        "catch.js",
        "const pending = fetch('http://127.0.0.1:0/hit'); console.log(pending instanceof Promise); pending.catch(e => console.error(e)); console.log('continuing');",
    ).unwrap();
    assert_eq!(output.stdout, "true\ncontinuing\n");
    assert!(output.stderr.contains("fetch failed:"));
}

#[test]
fn unhandled_transport_failure_is_a_promise_rejection() {
    let error = run_source(
        &Policy { net: true },
        "unhandled-fetch.js",
        "fetch('http://127.0.0.1:0/hit'); console.log('continuing');",
    )
    .unwrap_err();
    assert!(error
        .message
        .starts_with("unhandled promise rejection: Error: fetch failed:"));
    assert_eq!(error.output.stdout, "continuing\n");
}

#[test]
fn permission_denial_stays_synchronous() {
    let output = run_source(
        &Policy::default(),
        "sync-denial.js",
        "try { fetch('http://127.0.0.1:0/hit').catch(() => console.log('wrong-handler')); } catch(e) { console.error(e); } console.log('continuing');",
    ).unwrap();
    assert_eq!(output.stdout, "continuing\n");
    assert_eq!(output.stderr, "Error: permission denied: net\n");
}

#[test]
fn witness_waits_for_complete_request_headers() {
    use std::io::{Read, Write};
    use std::time::Duration;

    let witness = Witness::new(200, r#"{"hit":1}"#);
    let address = witness
        .url()
        .trim_start_matches("http://")
        .trim_end_matches("/hit")
        .to_owned();
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    stream
        .write_all(b"GET /hit HTTP/1.1\r\nHost: localhost\r\n")
        .unwrap();
    let mut byte = [0];
    let error = stream
        .read(&mut byte)
        .expect_err("responded before headers were complete");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(b"\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.ends_with(r#"{"hit":1}"#));
    assert_eq!(witness.finish(), 1);
}

#[test]
fn response_body_read_failure_reaches_catch_handler() {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/hit", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "client never connected");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let size = stream.read(&mut buffer).unwrap();
            assert_ne!(size, 0, "client closed before sending headers");
            request.extend_from_slice(&buffer[..size]);
        }
        // Advertise more bytes than supplied, then close the connection.
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\nshort")
            .unwrap();
    });
    let source = format!(
        "const pending = fetch('{url}'); console.log(pending instanceof Promise); pending.catch(e => console.error(e)); console.log('continuing');"
    );
    let result = run_source(&Policy { net: true }, "truncated.js", &source);
    worker.join().unwrap();
    let output = result.unwrap();
    assert_eq!(output.stdout, "true\ncontinuing\n");
    assert!(output.stderr.contains("fetch failed:"));
}

#[test]
fn requests_overlap_with_timers_and_continuations_can_submit_more() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/hit", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut hits = 0;
        // First three must arrive before any response is sent. The fourth is
        // submitted by an await continuation after the first batch completes.
        for count in [3, 1] {
            let mut streams = Vec::new();
            while streams.len() < count {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = Vec::new();
                        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                            let mut buffer = [0; 1024];
                            let n = stream.read(&mut buffer).unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buffer[..n]);
                        }
                        streams.push(stream);
                        hits += 1;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "requests did not overlap");
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
            for mut stream in streams {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n{\"hit\":1}").unwrap();
            }
        }
        hits
    });
    let source = format!(
        r#"
        (async () => {{
            const requests = [fetch('{url}'), fetch('{url}')];
            requests.push(new Promise(resolve => setTimeout(() => {{
                console.log('timer');
                resolve(fetch('{url}'));
            }}, 0)));
            console.log('continuing');
            const responses = await Promise.all(requests);
            console.log('responses', responses.length);
            console.log('hit', (await (await fetch('{url}')).json()).hit);
        }})();
    "#
    );
    let result = run_source(&Policy { net: true }, "overlap.js", &source);
    assert_eq!(worker.join().unwrap(), 4);
    assert_eq!(
        result.unwrap().stdout,
        "continuing\ntimer\nresponses 3\nhit 1\n"
    );
}

#[test]
fn unawaited_fetch_finishes_even_after_script_failure() {
    for failure in ["", "throw Error('boom');"] {
        let witness = Witness::new(200, r#"{"hit":1}"#);
        let source = format!(
            "fetch('{}').then(r => r.json()).then(body => console.log(body.hit)); {failure}",
            witness.url()
        );
        let result = run_source(&Policy { net: true }, "unawaited.js", &source);
        assert_eq!(witness.finish(), 1);
        let output = match result {
            Ok(output) => {
                assert!(failure.is_empty());
                output
            }
            Err(error) => {
                assert_eq!(error.message, "Error: boom");
                error.output
            }
        };
        assert_eq!(output.stdout, "1\n");
    }
}

#[test]
fn tokio_caller_can_complete_fetch_and_await_json() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = format!("(async () => {{ const response = await fetch('{}'); console.log((await response.json()).hit); }})();", witness.url());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let output = runtime
        .block_on(async { run_source(&Policy { net: true }, "tokio-fetch.js", &source) })
        .unwrap();
    assert_eq!(output.stdout, "1\n");
    assert_eq!(witness.finish(), 1);
}

#[test]
fn witness_waits_for_declared_post_body_split_from_headers() {
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };
    let witness = Witness::new(200, r#"{"received":true}"#);
    let url = url::Url::parse(&witness.url()).unwrap();
    let mut stream = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let body = vec![b'x'; 32 * 1024];
    write!(
        stream,
        "POST /hit HTTP/1.1\r\nHost: fixture\r\ncontent-length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    let mut probe = [0];
    let early = stream.read(&mut probe);
    assert!(
        matches!(early,Err(ref error) if matches!(error.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut)),
        "fixture responded or closed before receiving the declared POST body: {early:?}"
    );
    stream.write_all(&body[..4096]).unwrap();
    stream.write_all(&body[4096..]).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.ends_with(r#"{"received":true}"#));
    assert_eq!(witness.finish(), 1);
}
