use runtime::{wake_ready, Journal, ModuleGraph, OpIntent};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

struct Project(PathBuf);

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn crash_after_wait_registration_replays_until_a_confirmed_park_checkpoint() {
    let project = Project(
        std::env::temp_dir().join(format!("ondure-park-checkpoint-{}", uuid::Uuid::new_v4())),
    );
    fs::create_dir(&project.0).unwrap();
    let source = project.0.join("main.ts");
    fs::write(
        &source,
        "const waiting = ondure.approve({action: 'continue'}); console.log('after registration'); await waiting;",
    )
    .unwrap();
    let graph = ModuleGraph::load(&source).unwrap();
    let path = project.0.join("journal.sqlite3");
    let details = serde_json::json!({"action": "continue"}).to_string();

    // Reconstruct exactly the durable boundary before JavaScript continues
    // after approve(): the intent and wait exist, but no park checkpoint does.
    {
        let mut journal = Journal::open(&path).unwrap();
        let snapshot = serde_json::json!({"graph": graph, "net": false, "input": null});
        let identity = serde_json::json!({"net": false}).to_string();
        journal
            .create_run(
                "interrupted",
                None,
                graph.code_hash(),
                &format!("{:x}", Sha256::digest(identity.as_bytes())),
                &snapshot.to_string(),
            )
            .unwrap();
        journal
            .intents(
                "interrupted",
                &[OpIntent {
                    seq: 0,
                    name: "wait.approval".into(),
                    args_digest: format!("{:x}", Sha256::digest(details.as_bytes())),
                    policy: "safe".into(),
                }],
            )
            .unwrap();
        journal
            .park_wait("interrupted", 0, "approval", None, &details)
            .unwrap();
        assert_eq!(journal.get_run("interrupted").unwrap().status, "running");
    }
    fs::remove_file(source).unwrap();

    let mut outcomes = wake_ready(&path).unwrap();
    assert_eq!(outcomes.len(), 1);
    let output = outcomes.remove(0).result.unwrap();
    assert_eq!(output.stdout, "after registration\n");
    assert!(output.stderr.is_empty());
    let journal = Journal::open(&path).unwrap();
    let run = journal.get_run("interrupted").unwrap();
    assert_eq!(run.status, "parked");
    assert_eq!(run.stdout, output.stdout);
    assert_eq!(journal.waits("interrupted").unwrap().len(), 1);
    assert_eq!(journal.waits("interrupted").unwrap()[0].answer, None);
    assert_eq!(journal.operations("interrupted").unwrap().len(), 1);
    assert!(wake_ready(&path).unwrap().is_empty());
    assert_eq!(journal.get_run("interrupted").unwrap(), run);
}
