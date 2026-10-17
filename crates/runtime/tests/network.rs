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
