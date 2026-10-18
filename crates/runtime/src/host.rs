//! Execution entry point: assemble V8 core, integrations, and capabilities.
//!
//! Policy and buffers stay in a Rust-owned isolate slot, outside JavaScript's
//! globals. Local handles follow the visible scopes below; Global handles retain
//! compiled modules, pending resolvers, timer callbacks, and rejection reasons.

use crate::{
    core::{durability::Session, embedding, event_loop, execution, limits, microtasks, modules},
    integrations::{io, networking, timers},
    Capabilities, HostConfig, Policy,
};
use std::{cell::RefCell, fmt};

pub(crate) struct HostState {
    pub(crate) policy: Policy,
    pub(crate) host_config: Option<HostConfig>,
    pub(crate) effective_capabilities: RefCell<Option<Capabilities>>,
    pub(crate) stdout: RefCell<String>,
    pub(crate) stderr: RefCell<String>,
    pub(crate) operations: RefCell<event_loop::Operations>,
    pub(crate) session: RefCell<Option<Session>>,
    pub(crate) rejections: microtasks::PromiseRejections,
}

impl HostState {
    /// Called once by the agent entrypoint before invoking its handler.
    pub(crate) fn request_capabilities(&self, requested: &Capabilities) -> Result<(), String> {
        requested.validate()?;
        let grants = self
            .host_config
            .as_ref()
            .map(|c| c.grants.clone())
            .unwrap_or_default();
        *self.effective_capabilities.borrow_mut() = Some(requested.intersection(&grants));
        Ok(())
    }
    pub(crate) fn require_network(&self, url: &str) -> Result<(), String> {
        match self.effective_capabilities.borrow().as_ref() {
            Some(effective) => effective.require_network(url),
            None if self.policy.allows_net() => Ok(()),
            None => Err("permission denied: net".into()),
        }
    }
    pub(crate) fn require_child_agent(&self, name: &str) -> Result<(), String> {
        self.effective_capabilities
            .borrow()
            .as_ref()
            .ok_or("permission denied: child agent")?
            .require_child_agent(name)
    }
    pub(crate) fn require_tool(&self, tool: &str) -> Result<(), String> {
        self.effective_capabilities
            .borrow()
            .as_ref()
            .ok_or("permission denied: tool")?
            .require_tool(tool)
    }
    #[allow(dead_code)] // Consumed by the memory layer after stacking.
    pub(crate) fn require_memory_scope(&self, scope: &str) -> Result<(), String> {
        self.effective_capabilities
            .borrow()
            .as_ref()
            .ok_or_else(|| "permission denied: memory scope".to_string())?
            .require_memory_scope(scope)
    }
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

/// Execute with trusted, scoped operator configuration captured in the journal.
pub fn run_durable_with_config(
    config: &HostConfig,
    graph: &modules::ModuleGraph,
    journal: &std::path::Path,
    run_id: &str,
    limits: limits::RunLimits,
) -> Result<RunOutput, HostError> {
    let fail = |message| HostError {
        filename: graph.filename.clone(),
        message,
        output: RunOutput::default(),
    };
    graph.verify_integrity().map_err(|e| fail(e.to_string()))?;
    let policy = Policy::default();
    let session = Session::create_request_config(
        journal,
        run_id,
        &policy,
        graph,
        None,
        serde_json::Value::Null,
        Some(config),
    )
    .map_err(fail)?;
    execute(
        &policy,
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
    run_request_configuration(
        policy, None, graph, path, request_id, input, limits, None, 0,
    )
}

/// Start or rejoin a request under persisted scoped operator grants.
pub fn run_request_with_config(
    config: &HostConfig,
    graph: &modules::ModuleGraph,
    path: &std::path::Path,
    request_id: &str,
    input: &str,
    limits: limits::RunLimits,
) -> Result<crate::RunRecord, HostError> {
    run_request_configuration(
        &Policy::default(),
        Some(config),
        graph,
        path,
        request_id,
        input,
        limits,
        None,
        0,
    )
}

pub(crate) fn run_child_request(
    config: &HostConfig,
    graph: &modules::ModuleGraph,
    path: &std::path::Path,
    request_id: &str,
    input: &str,
    children: std::collections::BTreeMap<String, modules::ModuleGraph>,
    depth: usize,
) -> Result<crate::RunRecord, HostError> {
    run_request_configuration(
        &Policy::default(),
        Some(config),
        graph,
        path,
        request_id,
        input,
        limits::RunLimits {
            cpu_ms: 1000,
            heap_mb: 128,
        },
        Some(children),
        depth,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_request_configuration(
    policy: &Policy,
    config: Option<&HostConfig>,
    graph: &modules::ModuleGraph,
    path: &std::path::Path,
    request_id: &str,
    input: &str,
    limits: limits::RunLimits,
    captured_children: Option<std::collections::BTreeMap<String, modules::ModuleGraph>>,
    child_depth: usize,
) -> Result<crate::RunRecord, HostError> {
    let fail = |message: String| HostError {
        filename: graph.filename.clone(),
        message,
        output: RunOutput::default(),
    };
    graph.verify_integrity().map_err(|e| fail(e.to_string()))?;
    if let Some(config) = config {
        config.validate().map_err(fail)?;
    }
    let input: serde_json::Value = serde_json::from_str(input).map_err(|e| fail(e.to_string()))?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    }
    let journal = crate::Journal::open(path).map_err(|e| fail(e.to_string()))?;
    let _request_lease =
        crate::core::parking::RunLease::acquire(path, &format!("request:{request_id}"))
            .map_err(fail)?;
    let mut identity = serde_json::json!({"net": policy.net, "input": input});
    if let Some(config) = config {
        identity["host_config"] = serde_json::to_value(config).map_err(|e| fail(e.to_string()))?;
    }
    let identity = crate::core::durability::digest(&identity);
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
        if child_depth > 0 {
            // A scheduler may already own this child's run lease without its
            // request lease. Rejoin that live identity through the parent's
            // durable child wait rather than committing a transient lease error.
            let Some(lease) =
                crate::core::parking::RunLease::try_acquire(path, &format!("run:{}", run.id))
                    .map_err(fail)?
            else {
                return Ok(run);
            };
            let (recorded, policy, session) =
                Session::resume_with_lease(path, &run.id, lease).map_err(fail)?;
            replay_session(recorded, policy, session, Some(graph), limits)?;
        } else {
            replay_run_with_limits(path, &run.id, Some(graph), limits)?;
        }
        return journal.get_run(&run.id).map_err(|e| fail(e.to_string()));
    }
    let id = crate::new_run_id();
    let session = Session::create_captured_request(
        path,
        &id,
        policy,
        graph,
        Some(request_id),
        input,
        config,
        captured_children,
        child_depth,
    )
    .map_err(fail)?;
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
    let config = session
        .as_ref()
        .and_then(|session| session.host_config.clone());
    execute_config(policy, filename, program, session, limits, config)
}

fn execute_config(
    policy: &Policy,
    filename: &str,
    program: Program<'_>,
    session: Option<Session>,
    limits: limits::RunLimits,
    config: Option<HostConfig>,
) -> Result<RunOutput, HostError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::scope(|threads| {
            let worker = std::thread::Builder::new()
                .name("ondure-isolate".into())
                .spawn_scoped(threads, || {
                    run_on_thread(policy, filename, program, session, limits, config)
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
    run_on_thread(policy, filename, program, session, limits, config)
}

// Create, execute, and dispose both V8 and Tokio on the same host thread.
fn run_on_thread(
    policy: &Policy,
    filename: &str,
    program: Program<'_>,
    session: Option<Session>,
    limits: limits::RunLimits,
    config: Option<HostConfig>,
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
        effective_capabilities: RefCell::new(
            if matches!(&program, Program::Module(graph) if graph.uses_agent_module()) {
                Some(Capabilities::default())
            } else {
                config.as_ref().map(|c| c.grants.clone())
            },
        ),
        host_config: config,
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
        let failure =
            {
                v8::tc_scope!(let scope, scope);
                let installed =
                    if scope
                        .get_slot::<HostState>()
                        .expect("host state installed")
                        .session
                        .borrow()
                        .is_some()
                    {
                        crate::capabilities::determinism::install(scope).and_then(|()| {
                    let legacy_agent_api = scope
                        .get_slot::<HostState>()
                        .expect("host state installed")
                        .session.borrow().as_ref()
                        .is_some_and(|session| session.legacy_agent_api);
                    if matches!(&program, Program::Module(graph) if graph.uses_agent_module())
                        && !legacy_agent_api {
                        Ok(())
                    } else {
                        // Ordinary scripts and old captured agents retain their API.
                        crate::capabilities::agent::install(scope)
                    }
                })
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
    if limit_failure.is_some() || isolate.get_slot::<event_loop::DetachedHostWork>().is_some() {
        // Blocking HTTP cannot be canceled. Let its bounded worker finish in
        // the background; it owns no V8 handles and cannot deliver a result.
        runtime.shutdown_background();
    }
    let result = limit_failure.map_or(result, Err);

    // Drop all module handles while their isolate is still alive.
    isolate.remove_slot::<modules::ModuleStore>();
    crate::capabilities::agent::dispose(&mut isolate);
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
    fn agent_requests_cannot_elevate_grants_or_legacy_net() {
        for config in [
            None,
            Some(HostConfig {
                grants: Capabilities {
                    tools: ["read".into()].into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        ] {
            let state = HostState {
                policy: Policy { net: true },
                host_config: config,
                effective_capabilities: RefCell::new(None),
                stdout: RefCell::new(String::new()),
                stderr: RefCell::new(String::new()),
                operations: RefCell::new(event_loop::Operations::default()),
                session: RefCell::new(None),
                rejections: microtasks::PromiseRejections::default(),
            };
            state
                .request_capabilities(&Capabilities {
                    network_origins: ["https://example.com".into()].into(),
                    tools: ["pay".into()].into(),
                    memory_scopes: ["admin".into()].into(),
                    child_agents: ["root".into()].into(),
                })
                .unwrap();
            assert!(state.require_network("https://example.com").is_err());
            assert!(state.require_memory_scope("admin").is_err());
            let effective = state.effective_capabilities.borrow();
            let effective = effective.as_ref().unwrap();
            assert!(effective.require_tool("pay").is_err());
            assert!(effective.require_child_agent("root").is_err());
        }
    }

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
