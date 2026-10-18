use std::{fs, process::Command};

#[test]
fn inspection_commands_accept_two_run_ids_and_never_execute_code() {
    let directory =
        std::env::temp_dir().join(format!("ondure-inspection-cli-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let script = directory.join("main.js");
    fs::write(&script, "console.log(Math.random());").unwrap();
    let journal = directory.join("journal.sqlite3");
    let command = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ondure"))
            .args(args)
            .arg("--journal")
            .arg(&journal)
            .current_dir(&directory)
            .output()
            .unwrap()
    };
    for id in ["left", "right"] {
        assert!(command(&["run", script.to_str().unwrap(), "--run-id", id])
            .status
            .success());
    }
    fs::remove_file(script).unwrap();
    let diff = command(&["diff", "left", "right"]);
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );
    assert!(String::from_utf8(diff.stdout)
        .unwrap()
        .contains("\"identical\":false"));
    let explain = command(&["explain", "left"]);
    assert!(explain.status.success());
    assert!(String::from_utf8(explain.stdout)
        .unwrap()
        .contains("permissions"));
    assert!(!command(&["diff", "left", "unknown"]).status.success());
    fs::remove_dir_all(directory).unwrap();
}
