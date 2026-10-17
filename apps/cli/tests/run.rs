#[path = "../../../crates/runtime/tests/support/mod.rs"]
mod support;

use std::{
    fs,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
use support::Witness;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(policy: Option<&str>, source: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "ondure-cli-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("agent.js"), source).unwrap();
        if let Some(policy) = policy {
            fs::write(directory.join("ondure.yaml"), policy).unwrap();
        }
        Self(directory)
    }

    fn run(&self, arguments: &[&str]) -> std::process::Output {
        self.run_with_timeout(arguments, std::time::Duration::from_secs(15))
    }

    fn run_with_timeout(
        &self,
        arguments: &[&str],
        timeout: std::time::Duration,
    ) -> std::process::Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ondure"))
            .current_dir(&self.0)
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let stdout_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = std::time::Instant::now() + timeout;
        let timed_out = loop {
            if child.try_wait().unwrap().is_some() {
                break false;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                break true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let status = child.wait().unwrap();
        let stdout = stdout_reader.join().unwrap();
        let stderr = stderr_reader.join().unwrap();
        assert!(
            !timed_out,
            "ondure did not exit: {}",
            String::from_utf8_lossy(&stderr)
        );
        std::process::Output {
            status,
            stdout,
            stderr,
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Keep test input independent of the example script used for experiments.
fn fetch_source(url: &str) -> String {
    format!(
        r#"(async () => {{
            try {{
                const response = await fetch('{url}');
                if (!response.ok) throw new Error(`HTTP ${{response.status}}`);
                console.log(JSON.stringify(await response.json()));
            }} catch (error) {{
                console.error(error);
            }}
        }})();"#
    )
}

#[test]
fn missing_and_empty_policy_deny_without_failing_caught_script() {
    for policy in [None, Some("permissions: []\n")] {
        let witness = Witness::new(200, r#"{"hit":1}"#);
        let source = fetch_source(&witness.url());
        let directory = TestDirectory::new(policy, &source);
        let output = directory.run(&["run", "agent.js"]);
        let hits = witness.finish();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("permission denied: net"));
        assert!(output.stdout.is_empty());
        assert_eq!(hits, 0);
    }
}

#[test]
fn allowed_policy_prints_witness_json_once() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = fetch_source(&witness.url());
    let directory = TestDirectory::new(Some("permissions: [net]\n"), &source);
    let output = directory.run(&["run", "agent.js"]);
    let hits = witness.finish();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "{\"hit\":1}\n");
    assert!(output.stderr.is_empty());
    assert_eq!(hits, 1);
}

#[test]
fn computed_fetch_cannot_open_socket() {
    let witness = Witness::new(200, r#"{"hit":1}"#);
    let source = format!(
        "(async () => {{ try {{ await globalThis['fe' + 'tch']('{}'); console.log('unexpected-ok'); }} catch (error) {{ console.error(error); }} }})();",
        witness.url()
    );
    let directory = TestDirectory::new(Some("permissions: []\n"), &source);
    let output = directory.run(&["run", "agent.js"]);
    let hits = witness.finish();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("permission denied: net"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("unexpected-ok"));
    assert_eq!(hits, 0);
}

#[test]
fn invalid_policy_exits_one_before_executing_script() {
    for policy in ["permissions: [fs]", "permissions: ["] {
        let directory = TestDirectory::new(Some(policy), "console.log('executed');");
        let output = directory.run(&["run", "agent.js"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(!output.stderr.is_empty());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn usage_missing_script_and_host_errors_exit_one() {
    let directory = TestDirectory::new(None, "throw Error('boom');");
    for arguments in [
        vec![],
        vec!["run"],
        vec!["other", "agent.js"],
        vec!["run", "agent.js", "extra"],
        vec!["run", "missing.js"],
        vec!["run", "agent.js"],
    ] {
        let output = directory.run(&arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn failed_scripts_print_prior_output_before_the_error() {
    for (failure, diagnostic) in [
        ("throw Error('boom');", "agent.js: Error: boom\n"),
        (
            "(async () => { await Promise.resolve(); throw Error('boom'); })();",
            "agent.js: unhandled promise rejection: Error: boom\n",
        ),
        (
            "Promise.reject(Error('boom')); 42;",
            "agent.js: unhandled promise rejection: Error: boom\n",
        ),
    ] {
        let source = format!("console.log('starting'); console.error('warning'); {failure}");
        let directory = TestDirectory::new(None, &source);
        let output = directory.run(&["run", "agent.js"]);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "starting\n");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            format!("warning\n{diagnostic}")
        );
    }
}

#[test]
fn event_loop_finishes_timers_and_exits_when_idle() {
    let directory = TestDirectory::new(
        None,
        r#"
        console.log('starting');
        const canceled = setTimeout(() => console.log('wrong'), 2147483647);
        clearTimeout(canceled);
        new Promise(() => {});
        Promise.all(Array.from({length: 1000}, () => new Promise(resolve => setTimeout(resolve, 5))))
            .then(() => console.log('finished'));
    "#,
    );
    let output = directory.run(&["run", "agent.js"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"starting\nfinished\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn failed_script_finishes_pending_timers_before_printing_error() {
    let directory = TestDirectory::new(
        None,
        "setTimeout(() => console.log('finished'), 5); throw Error('boom');",
    );
    let output = directory.run(&["run", "agent.js"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"finished\n");
    assert_eq!(output.stderr, b"agent.js: Error: boom\n");
}

#[test]
fn large_stdout_and_stderr_do_not_block_child_exit() {
    let directory = TestDirectory::new(
        None,
        "console.log('x'.repeat(1024 * 1024)); console.error('y'.repeat(1024 * 1024));",
    );
    let output = directory.run(&["run", "agent.js"]);
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("{}\n", "x".repeat(1024 * 1024)).as_bytes()
    );
    assert_eq!(
        output.stderr,
        format!("{}\n", "y".repeat(1024 * 1024)).as_bytes()
    );
}

#[test]
fn timeout_still_kills_and_reaps_a_stuck_child() {
    let directory = TestDirectory::new(None, "while (true) {}");
    let failure = std::panic::catch_unwind(|| {
        directory.run_with_timeout(&["run", "agent.js"], std::time::Duration::from_millis(100))
    })
    .unwrap_err();
    let message = failure.downcast_ref::<String>().unwrap();
    assert!(message.contains("ondure did not exit:"));
}
