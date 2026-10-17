#[path = "../../../crates/runtime/tests/support/mod.rs"]
mod support;

use std::{fs, path::PathBuf, process::Command};
use support::Witness;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(policy: Option<&str>, source: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "ondure-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("agent.js"), source).unwrap();
        if let Some(policy) = policy {
            fs::write(directory.join("ondure.yaml"), policy).unwrap();
        }
        Self(directory)
    }

    fn run(&self, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_ondure"))
            .current_dir(&self.0)
            .args(arguments)
            .output()
            .unwrap()
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
