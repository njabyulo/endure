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
