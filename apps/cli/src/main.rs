mod supervisor;

use runtime::{
    approve_run_with_limits, journal_log, new_run_id, replay_run_with_limits,
    run_durable_with_limits, run_request_with_limits, wake_ready_with_limits, HostError, Journal,
    ModuleGraph, Policy, RunLimits, RunOutput,
};
use std::{
    error::Error,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};
const USAGE: &str = "usage: ondure run <script> [--run-id <id> | --request <id>] [--input <json>] [--journal <path>] [--cpu-ms <ms>] [--heap-mb <MiB>] | replay <run-id> | log <run-id> | approve <run-id> <wait-seq> yes|no | wake";

fn run(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<(), Box<dyn Error>> {
    let command = arguments
        .next()
        .ok_or(USAGE)?
        .into_string()
        .map_err(|_| USAGE)?;
    let target = if command == "wake" {
        None
    } else {
        Some(arguments.next().ok_or(USAGE)?)
    };
    let answer = if command == "approve" {
        let seq = arguments
            .next()
            .ok_or(USAGE)?
            .to_str()
            .ok_or(USAGE)?
            .parse::<u64>()?;
        let answer = match arguments.next().ok_or(USAGE)?.to_str() {
            Some("yes") => true,
            Some("no") => false,
            _ => return Err(USAGE.into()),
        };
        Some((seq, answer))
    } else {
        None
    };
    let mut journal = PathBuf::from(".ondure/journal.sqlite3");
    let mut limits = RunLimits {
        cpu_ms: 1000,
        heap_mb: 128,
    };
    let mut run_id = None;
    let mut request = None;
    let mut input = "null".to_string();
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or("missing option value")?;
        match flag.to_str() {
            Some("--journal") => journal = value.into(),
            Some("--cpu-ms") => {
                limits.cpu_ms = value.to_str().ok_or("invalid CPU budget")?.parse()?
            }
            Some("--heap-mb") => {
                limits.heap_mb = value.to_str().ok_or("invalid heap limit")?.parse()?
            }
            Some("--run-id") => {
                run_id = Some(value.into_string().map_err(|_| "run ID must be UTF-8")?)
            }
            Some("--request") => {
                request = Some(
                    value
                        .into_string()
                        .map_err(|_| "request ID must be UTF-8")?,
                )
            }
            Some("--input") => {
                input = value
                    .into_string()
                    .map_err(|_| "input must be UTF-8 JSON")?
            }
            _ => return Err("unknown option".into()),
        }
    }
    if request.is_some() && run_id.is_some() {
        return Err("choose --request or --run-id".into());
    }
    let id = target.as_ref().and_then(|s| s.to_str());
    match command.as_str() {
        "log" => println!("{}", journal_log(&journal, id.ok_or(USAGE)?)?),
        "replay" => {
            emit(replay_run_with_limits(
                &journal,
                id.ok_or(USAGE)?,
                None,
                limits,
            ))?;
            report_parked(&journal, id.unwrap())?;
        }
        "approve" => {
            let (seq, answer) = answer.ok_or(USAGE)?;
            emit(approve_run_with_limits(
                &journal,
                id.ok_or(USAGE)?,
                seq,
                answer,
                limits,
            ))?;
            report_parked(&journal, id.unwrap())?;
        }
        "wake" => {
            let runs = match wake_ready_with_limits(&journal, limits) {
                Ok(runs) => runs,
                Err(error) => {
                    write_output(&error.output)?;
                    return Err(error.into());
                }
            };
            let mut failed = false;
            for outcome in runs {
                match outcome.result {
                    Ok(run) => {
                        write_output(&RunOutput {
                            stdout: run.stdout,
                            stderr: run.stderr,
                        })?;
                        eprintln!("run {} {}", run.id, run.status);
                    }
                    Err(error) => {
                        write_output(&error.output)?;
                        eprintln!("{error}");
                        failed = true;
                    }
                }
            }
            if failed {
                return Err("one or more runs failed to wake".into());
            }
        }
        "run" => {
            let policy = Policy::load(Path::new("ondure.yaml"))?;
            let graph = ModuleGraph::load(Path::new(target.as_ref().ok_or(USAGE)?))?;
            if let Some(request) = request {
                let run = match run_request_with_limits(
                    &policy, &graph, &journal, &request, &input, limits,
                ) {
                    Ok(run) => run,
                    Err(error) => {
                        write_output(&error.output)?;
                        return Err(error.into());
                    }
                };
                write_output(&RunOutput {
                    stdout: run.stdout,
                    stderr: run.stderr,
                })?;
                eprintln!("run {} {}", run.id, run.status);
                if run.status == "failed" {
                    return Err("request run previously failed".into());
                }
            } else {
                if input != "null" {
                    return Err("--input requires --request".into());
                }
                let id = match run_id {
                    Some(id) => id,
                    None => {
                        let id = new_run_id();
                        writeln!(std::io::stderr().lock(), "Run ID: {id}")?;
                        id
                    }
                };
                emit(run_durable_with_limits(
                    &policy, &graph, &journal, &id, limits,
                ))?;
                report_parked(&journal, &id)?;
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
fn report_parked(path: &Path, id: &str) -> Result<(), Box<dyn Error>> {
    if Journal::open(path)?.get_run(id)?.status == "parked" {
        eprintln!("run {id} parked");
    }
    Ok(())
}
fn emit(result: Result<RunOutput, HostError>) -> Result<(), Box<dyn Error>> {
    match result {
        Ok(output) => write_output(&output)?,
        Err(error) => {
            write_output(&error.output)?;
            return Err(error.into());
        }
    }
    Ok(())
}
fn write_output(output: &RunOutput) -> std::io::Result<()> {
    std::io::stdout()
        .lock()
        .write_all(output.stdout.as_bytes())?;
    std::io::stderr()
        .lock()
        .write_all(output.stderr.as_bytes())?;
    Ok(())
}
fn main() -> ExitCode {
    let mut arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if arguments
        .first()
        .is_some_and(|argument| argument == "__worker")
    {
        arguments.remove(0);
        run(arguments.into_iter())
    } else {
        supervise(&arguments)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn supervise(arguments: &[std::ffi::OsString]) -> Result<(), Box<dyn Error>> {
    let output = supervisor::supervise(arguments)?;
    std::io::stdout().lock().write_all(&output.stdout)?;
    std::io::stderr().lock().write_all(&output.stderr)?;
    if output.status.success() {
        return Ok(());
    }
    if let Some(code) = output.status.code() {
        std::process::exit(code);
    }
    Err(format!("runtime worker terminated: {}", output.status).into())
}
