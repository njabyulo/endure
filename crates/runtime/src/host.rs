//! Execution entry point: assemble V8 core, integrations, and capabilities.
//!
//! Policy and buffers stay in a Rust-owned isolate slot, outside JavaScript's
//! globals. Local handles follow the visible scopes below; Global handles retain
//! compiled modules, pending resolvers, timer callbacks, and rejection reasons.

use crate::{
    core::{durability::Session, embedding, event_loop, execution, limits, microtasks, modules},
    integrations::{io, networking, timers},
    Policy,
};
use std::{cell::RefCell, fmt};

pub(crate) struct HostState {
    pub(crate) policy: Policy,
    pub(crate) stdout: RefCell<String>,
    pub(crate) stderr: RefCell<String>,
    pub(crate) operations: RefCell<event_loop::Operations>,
    pub(crate) session: RefCell<Option<Session>>,
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

/// Execute JavaScript and wait for all submitted host operations.
///
/// This function blocks its caller. Inside Tokio it runs the entire isolate on
/// a separate thread to avoid nesting executors. Async callers should use
/// `spawn_blocking` when they need their own executor to keep making progress.
pub fn run_source(policy: &Policy, filename: &str, source: &str) -> Result<RunOutput, HostError> {
    run_source_with_limits(policy, filename, source, limits::RunLimits::default())
}

/// Execute with explicit active-execution and V8 managed-heap limits.
pub fn run_source_with_limits(
    policy: &Policy,
    filename: &str,
    source: &str,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    execute(policy, filename, Program::Script(source), None, limits)
}

/// Execute a prepared graph, optionally requiring the code hash of a prior run.
/// Hash mismatch fails before isolate creation or JavaScript side effects.
/// Like `run_source`, this blocks its caller and supports entered Tokio contexts.
pub fn run_module(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    expected_hash: Option<&str>,
) -> Result<RunOutput, HostError> {
    run_module_with_limits(policy, graph, expected_hash, limits::RunLimits::default())
}

/// Execute with explicit active-execution and V8 managed-heap limits.
pub fn run_module_with_limits(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    expected_hash: Option<&str>,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    graph.verify_integrity().map_err(|error| HostError {
        filename: error.filename,
        message: error.message,
        output: RunOutput::default(),
    })?;
    if let Some(expected) = expected_hash {
        if expected != graph.code_hash() {
            return Err(HostError {
                filename: graph.filename.clone(),
                message: format!(
                    "code hash mismatch: expected {expected}, actual {}",
                    graph.code_hash()
                ),
                output: RunOutput::default(),
            });
        }
    }
    execute(
        policy,
        &graph.filename,
        Program::Module(graph),
        None,
        limits,
    )
}

/// Execute a fresh journaled run. Operation results commit before JS observes them.
pub fn run_durable(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    journal: &std::path::Path,
    run_id: &str,
) -> Result<RunOutput, HostError> {
    run_durable_with_limits(policy, graph, journal, run_id, limits::RunLimits::default())
}

/// Execute with explicit active-execution and V8 managed-heap limits.
pub fn run_durable_with_limits(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    journal: &std::path::Path,
    run_id: &str,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    graph.verify_integrity().map_err(|error| HostError {
        filename: error.filename,
        message: error.message,
        output: RunOutput::default(),
    })?;
    let session = Session::create(journal, run_id, policy, graph).map_err(|message| HostError {
        filename: graph.filename.clone(),
        message,
        output: RunOutput::default(),
    })?;
    execute(
        policy,
        &graph.filename,
        Program::Module(graph),
        Some(session),
        limits,
    )
}

/// Re-execute captured code, supplying recorded results in their original order.
/// Unfinished safe operations may run live; completed effects are never reissued.
pub fn replay_run(
    journal: &std::path::Path,
    run_id: &str,
    replacement: Option<&modules::ModuleGraph>,
) -> Result<RunOutput, HostError> {
    replay_run_with_limits(journal, run_id, replacement, limits::RunLimits::default())
}

/// Execute with explicit active-execution and V8 managed-heap limits.
pub fn replay_run_with_limits(
    journal: &std::path::Path,
    run_id: &str,
    replacement: Option<&modules::ModuleGraph>,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    let (recorded, policy, session) =
        Session::resume(journal, run_id).map_err(|message| HostError {
            filename: run_id.into(),
            message,
            output: RunOutput::default(),
        })?;
    replay_session(recorded, policy, session, replacement, limits)
}

pub(crate) fn replay_session(
    recorded: modules::ModuleGraph,
    policy: Policy,
    mut session: Session,
    replacement: Option<&modules::ModuleGraph>,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    let run_id = session.run_id.clone();
    let parked = session.journal.get_run(&run_id).map_err(|e| HostError {
        filename: run_id.clone(),
        message: e.to_string(),
        output: RunOutput::default(),
    })?;
    let graph = replacement.unwrap_or(&recorded);
    graph.verify_integrity().map_err(|error| HostError {
        filename: error.filename,
        message: error.message,
        output: RunOutput::default(),
    })?;
    if graph.code_hash() != recorded.code_hash() {
        return Err(HostError {
            filename: graph.filename.clone(),
            message: "code hash mismatch on replay".into(),
            output: RunOutput::default(),
        });
    }
    if parked.status == "parked"
        && crate::core::durability::can_remain_parked(&session.journal, &run_id).map_err(
            |message| HostError {
                filename: run_id.clone(),
                message,
                output: RunOutput::default(),
            },
        )?
    {
        return Ok(RunOutput {
            stdout: parked.stdout,
            stderr: parked.stderr,
        });
    }
    if !session.is_sealed() {
        session
            .journal
            .set_status(&run_id, "running")
            .map_err(|error| HostError {
                filename: run_id.clone(),
                message: error.to_string(),
                output: RunOutput::default(),
            })?;
    }
    execute(
        &policy,
        &graph.filename,
        Program::Module(graph),
        Some(session),
        limits,
    )
}

/// Start or rejoin a request. The same request ID cannot change code, policy,
/// or JSON input, and concurrent executors cannot own the same request/run.
pub fn run_request(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    path: &std::path::Path,
    request_id: &str,
    input: &str,
) -> Result<crate::RunRecord, HostError> {
    run_request_with_limits(
        policy,
        graph,
        path,
        request_id,
        input,
        limits::RunLimits::default(),
    )
}

/// Execute with explicit active-execution and V8 managed-heap limits.
pub fn run_request_with_limits(
    policy: &Policy,
    graph: &modules::ModuleGraph,
    path: &std::path::Path,
    request_id: &str,
    input: &str,
    limits: limits::RunLimits,
) -> Result<crate::RunRecord, HostError> {
    let fail = |message: String| HostError {
        filename: graph.filename.clone(),
        message,
        output: RunOutput::default(),
    };
    graph.verify_integrity().map_err(|e| fail(e.to_string()))?;
    let input: serde_json::Value = serde_json::from_str(input).map_err(|e| fail(e.to_string()))?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    }
    let journal = crate::Journal::open(path).map_err(|e| fail(e.to_string()))?;
    let _request_lease =
        crate::core::parking::RunLease::acquire(path, &format!("request:{request_id}"))
            .map_err(fail)?;
    let identity =
        crate::core::durability::digest(&serde_json::json!({"net": policy.net, "input": input}));
    if let Some(run) = journal
        .find_request(request_id)
        .map_err(|e| fail(e.to_string()))?
    {
        if run.code_hash != graph.code_hash() || run.input_digest != identity {
            return Err(fail(
                "request ID conflicts with different code, policy, or input".into(),
            ));
        }
        if run.status == "parked"
            && crate::core::durability::can_remain_parked(&journal, &run.id).map_err(fail)?
        {
            return Ok(run);
        }
        if matches!(run.status.as_str(), "done" | "failed") {
            return Ok(run);
        }
        if run.status == "diverged" {
            return Err(fail("request run has diverged".into()));
        }
        replay_run_with_limits(path, &run.id, Some(graph), limits)?;
        return journal.get_run(&run.id).map_err(|e| fail(e.to_string()));
    }
    let id = crate::new_run_id();
    let session =
        Session::create_request(path, &id, policy, graph, Some(request_id), input).map_err(fail)?;
    execute(
        policy,
        &graph.filename,
        Program::Module(graph),
        Some(session),
        limits,
    )?;
    journal.get_run(&id).map_err(|e| fail(e.to_string()))
}

#[derive(Clone, Copy)]
enum Program<'a> {
    Script(&'a str),
    Module(&'a modules::ModuleGraph),
}

fn execute(
    policy: &Policy,
    filename: &str,
    program: Program<'_>,
    session: Option<Session>,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::scope(|threads| {
            let worker = std::thread::Builder::new()
                .name("ondure-isolate".into())
                .spawn_scoped(threads, || {
                    run_on_thread(policy, filename, program, session, limits)
                })
                .map_err(|error| HostError {
                    filename: filename.into(),
                    message: format!("isolate thread initialization failed: {error}"),
                    output: RunOutput::default(),
                })?;
            worker.join().unwrap_or_else(|_| {
                Err(HostError {
                    filename: filename.into(),
                    message: "isolate thread panicked".into(),
                    output: RunOutput::default(),
                })
            })
        });
    }
    run_on_thread(policy, filename, program, session, limits)
}

// Create, execute, and dispose both V8 and Tokio on the same host thread.
fn run_on_thread(
    policy: &Policy,
    filename: &str,
    program: Program<'_>,
    session: Option<Session>,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| HostError {
            filename: filename.into(),
            message: format!("event loop initialization failed: {error}"),
            output: RunOutput::default(),
        })?;
    let mut isolate = embedding::new_isolate_with_heap_limit(limits.heap_mb);
    microtasks::install(&mut isolate);
    isolate.set_host_import_module_dynamically_callback(modules::reject_dynamic_import);
    isolate.set_slot(HostState {
        policy: *policy,
        stdout: RefCell::new(String::new()),
        stderr: RefCell::new(String::new()),
        operations: RefCell::new(event_loop::Operations::default()),
        session: RefCell::new(session),
        rejections: microtasks::PromiseRejections::default(),
    });

    let guard = limits::Guard::install(&mut isolate, limits);
    let result = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        io::install(scope);
        networking::install(scope);
        timers::install(scope);

        let mut evaluation = None;
        let failure = {
            v8::tc_scope!(let scope, scope);
            let installed = if scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .session
                .borrow()
                .is_some()
            {
                crate::capabilities::determinism::install(scope)
                    .and_then(|()| crate::capabilities::agent::install(scope))
            } else {
                Ok(())
            };
            let result = installed.and_then(|()| match program {
                Program::Script(source) => execution::compile_and_run(scope, filename, source),
                Program::Module(graph) => {
                    modules::evaluate(scope, graph).map(|promise| evaluation = Some(promise))
                }
            });
            let failure = result.err().map(|fallback| {
                scope
                    .exception()
                    .map(|error| error.to_rust_string_lossy(scope))
                    .unwrap_or(fallback)
            });
            scope.reset();
            failure
        };
        event_loop::run(scope, &runtime, failure, evaluation.as_ref())
    };

    let limit_failure = guard.finish(&mut isolate);
    if limit_failure.is_some() {
        // Blocking HTTP cannot be canceled. Let its bounded worker finish in
        // the background; it owns no V8 handles and cannot deliver a result.
        runtime.shutdown_background();
    }
    let result = limit_failure.map_or(result, Err);

    // Drop all module handles while their isolate is still alive.
    isolate.remove_slot::<modules::ModuleStore>();
    let state = isolate
        .remove_slot::<HostState>()
        .expect("host state installed");
    let output = RunOutput {
        stdout: state.stdout.take(),
        stderr: state.stderr.take(),
    };
    let mut result = result;
    if let Some(mut session) = state.session.into_inner() {
        let status = if result.is_ok() { "done" } else { "failed" };
        if let Err(error) = session.finish(status, &output) {
            result = Err(error);
        }
    }
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

    #[test]
    fn tokio_callers_receive_output_and_errors_without_nested_runtime_panics() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let output = run_source(&Policy::default(), "sync.js", "console.log('sync');").unwrap();
            assert_eq!(output.stdout, "sync\n");
            let output = run_source(
                &Policy::default(),
                "timer.js",
                "setTimeout(() => console.log('done'), 1);",
            )
            .unwrap();
            assert_eq!(output.stdout, "done\n");
            let error = run_source(
                &Policy::default(),
                "failure.js",
                "setTimeout(() => console.log('done'), 1); throw Error('boom');",
            )
            .unwrap_err();
            assert_eq!(error.message, "Error: boom");
            assert_eq!(error.output.stdout, "done\n");
        });
    }
}
