//! Execution entry point: assemble V8 core, integrations, and capabilities.
//!
//! Policy and buffers stay in a Rust-owned isolate slot, outside JavaScript's
//! globals. Local handles follow the visible scopes below; Global handles retain
//! rejected promises and their reasons until final rejection inspection.

use crate::{
    core::{embedding, execution, microtasks},
    integrations::{io, networking},
    Policy,
};
use std::{cell::RefCell, fmt};

pub(crate) struct HostState {
    pub(crate) policy: Policy,
    pub(crate) stdout: RefCell<String>,
    pub(crate) stderr: RefCell<String>,
    pub(crate) rejections: microtasks::PromiseRejections,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug)]
pub struct HostError {
    pub filename: String,
    pub message: String,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.filename, self.message)
    }
}

impl std::error::Error for HostError {}

pub fn run_source(policy: &Policy, filename: &str, source: &str) -> Result<RunOutput, HostError> {
    let mut isolate = embedding::new_isolate();
    microtasks::install(&mut isolate);
    isolate.set_slot(HostState {
        policy: *policy,
        stdout: RefCell::new(String::new()),
        stderr: RefCell::new(String::new()),
        rejections: microtasks::PromiseRejections::default(),
    });

    {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        io::install(scope);
        networking::install(scope);

        v8::tc_scope!(let scope, scope);
        execution::compile_and_run(scope, filename, source).map_err(|fallback| {
            let message = scope
                .exception()
                .map(|error| error.to_rust_string_lossy(scope))
                .unwrap_or(fallback);
            HostError {
                filename: filename.into(),
                message,
            }
        })?;

        microtasks::drain(scope);
        if let Some(exception) = scope.exception() {
            return Err(HostError {
                filename: filename.into(),
                message: exception.to_rust_string_lossy(scope),
            });
        }
        if let Some(message) = microtasks::take_unhandled_rejection(scope) {
            return Err(HostError {
                filename: filename.into(),
                message,
            });
        }
    }

    let state = isolate
        .remove_slot::<HostState>()
        .expect("host state installed");
    Ok(RunOutput {
        stdout: state.stdout.take(),
        stderr: state.stderr.take(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caught_error_is_success() {
        let output = run_source(
            &Policy::default(),
            "caught.js",
            "try { throw Error('caught'); } catch (e) { console.error(e); }",
        )
        .unwrap();
        assert_eq!(output.stderr, "Error: caught\n");
    }
}
