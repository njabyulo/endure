use std::{path::Path, process::Command};

#[test]
fn local_workflows_prove_standard_interfaces_and_approved_crash_recovery() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    for example in ["standard-interfaces", "onboarding"] {
        let output = Command::new("python3")
            .arg(root.join("examples").join(example).join("run.py"))
            .env("ONDURE_BIN", env!("CARGO_BIN_EXE_ondure"))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{example}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
