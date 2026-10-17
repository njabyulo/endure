//! Keep native failures in a worker and stream its output with bounded memory.
//! A private stdin pipe also terminates the worker when its supervisor dies.

use std::{
    error::Error,
    ffi::OsString,
    io::{self, Read, Write},
    process::{Command, ExitStatus, Stdio},
    time::Duration,
};

/// Spawn the worker with a parent-liveness pipe, forwarding both output streams
/// as they arrive. No wall deadline penalizes pending HTTP or parked operations.
pub(crate) fn supervise(arguments: &[OsString]) -> Result<ExitStatus, Box<dyn Error>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("__worker")
        .arg("__parent-watch")
        .args(arguments);
    forward_worker(&mut command, io::stdout(), io::stderr())
}

/// Only supervised workers reserve stdin for parent liveness. A closed pipe
/// means the supervisor is gone, including death before worker initialization.
pub(crate) fn watch_parent() -> io::Result<()> {
    // Handshake before executing user code: EOF during spawn/startup must not
    // let an orphan begin effects before the watcher thread gets scheduled.
    io::stdin().lock().read_exact(&mut [0])?;
    std::thread::Builder::new()
        .name("ondure-parent-watch".into())
        .spawn(|| {
            let mut byte = [0];
            let mut input = io::stdin().lock();
            loop {
                match input.read(&mut byte) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Ok(0) | Err(_) => std::process::exit(1),
                    Ok(_) => continue,
                }
            }
        })?;
    Ok(())
}

fn forward_worker(
    command: &mut Command,
    stdout: impl Write + Send,
    stderr: impl Write + Send,
) -> Result<ExitStatus, Box<dyn Error>> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Keep this writer alive until the child has exited. All other copies of
    // the pipe are closed on exec, so supervisor death is observable as EOF.
    let mut parent_alive = child.stdin.take().expect("worker liveness pipe");
    if let Err(error) = parent_alive.write_all(&[1]) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let out = child.stdout.take().expect("worker stdout pipe");
    let err = child.stderr.take().expect("worker stderr pipe");
    let result = std::thread::scope(|threads| -> Result<ExitStatus, Box<dyn Error>> {
        let (errors, failures) = std::sync::mpsc::channel();
        let out_errors = errors.clone();
        let out_reader = threads.spawn(move || relay(out, stdout, out_errors));
        let err_reader = threads.spawn(move || relay(err, stderr, errors));
        let status = loop {
            if let Ok(error) = failures.try_recv() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            }
        };
        out_reader
            .join()
            .map_err(|_| "stdout forwarding thread panicked")??;
        err_reader
            .join()
            .map_err(|_| "stderr forwarding thread panicked")??;
        Ok(status)
    });
    drop(parent_alive);
    result
}

fn relay(
    mut source: impl Read,
    mut destination: impl Write,
    errors: std::sync::mpsc::Sender<String>,
) -> io::Result<()> {
    // Flush each chunk, including destinations such as stdout that can be line
    // buffered. Memory use is independent of the worker's total output size.
    let result = (|| {
        let mut bytes = [0; 8192];
        loop {
            let count = match source.read(&mut bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                return Ok(());
            }
            destination.write_all(&bytes[..count])?;
            destination.flush()?;
        }
    })();
    if let Err(error) = &result {
        let _ = errors.send(format!("worker output forwarding failed: {error}"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(name: &str, mode: &str) -> (ExitStatus, Vec<u8>, Vec<u8>) {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([name, "--nocapture"])
            .env("ONDURE_SUPERVISOR_TEST_WORKER", mode);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let status = forward_worker(&mut command, &mut out, &mut err).unwrap();
        (status, out, err)
    }

    #[test]
    fn abort_worker() {
        if std::env::var("ONDURE_SUPERVISOR_TEST_WORKER").as_deref() != Ok("abort") {
            return;
        }
        println!("before native abort");
        io::stdout().flush().unwrap();
        eprintln!("native failure diagnostic");
        std::process::abort();
    }

    #[test]
    fn large_output_worker() {
        if std::env::var("ONDURE_SUPERVISOR_TEST_WORKER").as_deref() != Ok("output") {
            return;
        }
        io::stdout()
            .lock()
            .write_all(&vec![b'o'; 1024 * 1024])
            .unwrap();
        io::stderr()
            .lock()
            .write_all(&vec![b'e'; 1024 * 1024])
            .unwrap();
    }

    #[test]
    fn native_abort_is_contained_and_next_worker_runs() {
        let (status, out, err) = worker("abort_worker", "abort");
        assert!(!status.success());
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert!(status.signal().is_some());
        }
        assert!(String::from_utf8_lossy(&out).contains("before native abort"));
        assert!(String::from_utf8_lossy(&err).contains("native failure diagnostic"));
        assert!(worker("large_output_worker", "healthy").0.success());
    }

    #[test]
    fn both_streams_are_forwarded() {
        let (status, out, err) = worker("large_output_worker", "output");
        assert!(status.success());
        assert!(out
            .windows(1024 * 1024)
            .any(|chunk| chunk.iter().all(|&byte| byte == b'o')));
        assert_eq!(err, vec![b'e'; 1024 * 1024]);
    }
}
