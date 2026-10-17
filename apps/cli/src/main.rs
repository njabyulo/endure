use runtime::{journal_log, new_run_id, replay_run, run_durable, ModuleGraph, Policy, RunOutput};
use std::{error::Error, io::Write, path::Path, process::ExitCode};

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments.next();
    let target = arguments
        .next()
        .ok_or("usage: ondure run <script> [--run-id <id>] [--journal <path>] | replay <run-id> | log <run-id>")?;
    let mut journal = std::path::PathBuf::from(".ondure/journal.sqlite3");
    let mut run_id = None;
    while let Some(flag) = arguments.next() {
        let value = arguments.next().ok_or("missing option value")?;
        match flag.to_str() {
            Some("--journal") => journal = value.into(),
            Some("--run-id") => {
                run_id = Some(value.into_string().map_err(|_| "run ID must be UTF-8")?)
            }
            _ => return Err("unknown option".into()),
        }
    }
    match command.as_deref().and_then(|s| s.to_str()) {
        Some("log") => {
            println!(
                "{}",
                journal_log(&journal, target.to_str().ok_or("run ID must be UTF-8")?)?
            );
            return Ok(());
        }
        Some("replay") => {
            let output = match replay_run(
                &journal,
                target.to_str().ok_or("run ID must be UTF-8")?,
                None,
            ) {
                Ok(output) => output,
                Err(error) => {
                    write_output(&error.output)?;
                    return Err(error.into());
                }
            };
            write_output(&output)?;
            return Ok(());
        }
        Some("run") => {}
        _ => {
            return Err(
                "usage: ondure run <script> [--run-id <id>] [--journal <path>] | replay <run-id> | log <run-id>"
                    .into(),
            )
        }
    }
    let policy = Policy::load(Path::new("ondure.yaml"))?;
    let script = target;
    let graph = ModuleGraph::load(Path::new(&script))?;
    let run_id = match run_id {
        Some(run_id) => run_id,
        None => {
            let run_id = new_run_id();
            writeln!(std::io::stderr().lock(), "Run ID: {run_id}")?;
            run_id
        }
    };
    let output = match run_durable(&policy, &graph, &journal, &run_id) {
        Ok(output) => output,
        Err(error) => {
            write_output(&error.output)?;
            return Err(error.into());
        }
    };
    write_output(&output)?;
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
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
