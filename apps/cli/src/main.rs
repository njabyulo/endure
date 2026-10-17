use runtime::{run_source, Policy, RunOutput};
use std::{error::Error, fs, io::Write, path::Path, process::ExitCode};

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments.next();
    let script = arguments.next();
    if command.as_deref() != Some(std::ffi::OsStr::new("run"))
        || script.is_none()
        || arguments.next().is_some()
    {
        return Err("usage: ondure run <script>".into());
    }
    let policy = Policy::load(Path::new("ondure.yaml"))?;
    let script = script.expect("script argument checked");
    let source = fs::read_to_string(&script)?;
    let output = match run_source(&policy, &script.to_string_lossy(), &source) {
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
