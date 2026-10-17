//! Execution entry point: assemble V8 core, integrations, and capabilities.
//!
//! Policy and buffers stay in a Rust-owned isolate slot, outside JavaScript's
//! globals. Local handles follow the visible scopes below; Global handles retain
//! pending resolvers, timer callbacks, and unhandled rejection reasons.

use crate::{
    core::{embedding, event_loop, execution, microtasks},
    integrations::{io, networking, timers},
    Policy,
};
use std::{cell::RefCell, fmt};

pub(crate) struct HostState {
    pub(crate) policy: Policy,
    pub(crate) stdout: RefCell<String>,
    pub(crate) stderr: RefCell<String>,
    pub(crate) operations: RefCell<event_loop::Operations>,
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
    /// Console output captured before the failure.
    pub output: RunOutput,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.filename, self.message)
    }
}

impl std::error::Error for HostError {}

pub fn run_source(policy: &Policy, filename: &str, source: &str) -> Result<RunOutput, HostError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| HostError {
            filename: filename.into(),
            message: format!("event loop initialization failed: {error}"),
            output: RunOutput::default(),
        })?;
    let mut isolate = embedding::new_isolate();
    microtasks::install(&mut isolate);
    isolate.set_slot(HostState {
        policy: *policy,
        stdout: RefCell::new(String::new()),
        stderr: RefCell::new(String::new()),
        operations: RefCell::new(event_loop::Operations::default()),
        rejections: microtasks::PromiseRejections::default(),
    });

    let result = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        io::install(scope);
        networking::install(scope);
        timers::install(scope);

        let failure = {
            v8::tc_scope!(let scope, scope);
            let failure = execution::compile_and_run(scope, filename, source)
                .err()
                .map(|fallback| {
                    scope
                        .exception()
                        .map(|error| error.to_rust_string_lossy(scope))
                        .unwrap_or(fallback)
                });
            scope.reset();
            failure
        };
        event_loop::run(scope, &runtime, failure)
    };

    let state = isolate
        .remove_slot::<HostState>()
        .expect("host state installed");
    let output = RunOutput {
        stdout: state.stdout.take(),
        stderr: state.stderr.take(),
    };
    match result {
        Ok(()) => Ok(output),
        Err(message) => Err(HostError {
            filename: filename.into(),
            message,
            output,
        }),
    }
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

    #[test]
    fn errors_retain_both_output_streams() {
        for failure in [
            "throw Error('boom');",
            "(async () => { await Promise.resolve(); throw Error('boom'); })();",
            "Promise.reject(Error('boom')); 42;",
        ] {
            let source = format!("console.log('starting'); console.error('warning'); {failure}");
            let error = run_source(&Policy::default(), "failure.js", &source).unwrap_err();
            assert_eq!(error.output.stdout, "starting\n");
            assert_eq!(error.output.stderr, "warning\n");
            assert!(error.message.contains("boom"));
        }
    }

    #[test]
    fn syntax_errors_have_no_output() {
        let error = run_source(
            &Policy::default(),
            "syntax.js",
            "console.log('not executed'); const = ;",
        )
        .unwrap_err();
        assert_eq!(error.output, RunOutput::default());
        assert!(error.message.contains("SyntaxError"));
    }
}
