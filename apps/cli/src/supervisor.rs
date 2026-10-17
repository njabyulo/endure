//! Execute the CLI workload in a disposable child process.
//!
//! In-process `RunLimits` bounds active execution and V8's managed heap; it does
//! not contain external allocations or fatal native V8 out-of-memory failures.
//! The CLI parent survives a worker abort and can report its exit status. This
//! is process containment, not a guarantee against a system-wide kernel OOM.

use std::{
    error::Error,
    ffi::OsString,
    process::{Command, Output, Stdio},
};

/// Run an internal worker with the original arguments and working directory.
///
/// `wait_with_output` drains stdout and stderr concurrently, preventing full
/// pipes from deadlocking the worker. The caller forwards both captured streams
/// and handles unsuccessful statuses, including signal termination. There is no
/// wall-clock deadline: parked runs and pending HTTP operations may wait without
/// consuming their active-execution budget.
pub(crate) fn supervise(arguments: &[OsString]) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("__worker").args(arguments);
    collect_worker(&mut command)
}

fn collect_worker(command: &mut Command) -> Result<Output, Box<dyn Error>> {
    let child = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    Ok(child.wait_with_output()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn worker(name: &str, mode: &str) -> Output {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([name, "--nocapture"])
            .env("ONDURE_SUPERVISOR_TEST_WORKER", mode);
        collect_worker(&mut command).unwrap()
    }

    #[test]
    fn abort_worker() {
        if std::env::var("ONDURE_SUPERVISOR_TEST_WORKER").as_deref() != Ok("abort") {
            return;
        }
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "before native abort").unwrap();
        stdout.flush().unwrap();
        let mut stderr = std::io::stderr().lock();
        writeln!(stderr, "native failure diagnostic").unwrap();
        stderr.flush().unwrap();
        std::process::abort();
    }

    #[test]
    fn large_output_worker() {
        if std::env::var("ONDURE_SUPERVISOR_TEST_WORKER").as_deref() != Ok("output") {
            return;
        }
        std::io::stdout()
            .lock()
            .write_all(&vec![b'o'; 1024 * 1024])
            .unwrap();
        std::io::stderr()
            .lock()
            .write_all(&vec![b'e'; 1024 * 1024])
            .unwrap();
    }

    #[test]
    fn native_abort_is_contained_and_next_worker_runs() {
        let output = worker("abort_worker", "abort");
        assert!(!output.status.success());
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert!(
                output.status.signal().is_some(),
                "expected native signal termination"
            );
        }
        assert!(String::from_utf8_lossy(&output.stdout).contains("before native abort"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("native failure diagnostic"));
        assert!(worker("large_output_worker", "healthy").status.success());
    }

    #[test]
    fn both_streams_are_drained_while_worker_runs() {
        let output = worker("large_output_worker", "output");
        assert!(output.status.success());
        assert!(output
            .stdout
            .windows(1024 * 1024)
            .any(|chunk| chunk.iter().all(|&byte| byte == b'o')));
        assert_eq!(output.stderr, vec![b'e'; 1024 * 1024]);
    }
}
