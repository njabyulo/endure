//! Captured local child agents, deterministic request identities and persisted joins.
use crate::{
    core::{
        embedding::set_property,
        event_loop::JsonJob,
        operations::{validate_json, RecoveryPolicy, MAX_OPERATION_ARGUMENT_BYTES},
    },
    host::HostState,
    Capabilities, HostConfig, Journal, ModuleGraph,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildAllocation {
    pub tokens: u64,
    pub dollar_micros: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildConfig {
    pub module: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    pub allocation: ChildAllocation,
}
impl ChildConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.module.is_empty() || Path::new(&self.module).is_absolute() {
            return Err("child module must be a relative local path".into());
        }
        if self.allocation.tokens > i64::MAX as u64
            || self.allocation.dollar_micros > i64::MAX as u64
        {
            return Err("child allocation exceeds SQLite range".into());
        }
        self.capabilities.validate()?;
        Ok(())
    }
}

pub(crate) fn capture(
    parent: &ModuleGraph,
    config: Option<&HostConfig>,
) -> Result<BTreeMap<String, ModuleGraph>, String> {
    let mut children = BTreeMap::new();
    let Some(config) = config else {
        return Ok(children);
    };
    if config.agents.len() > 32 {
        return Err("at most 32 configured child agents are supported".into());
    }
    if config.agents.is_empty() {
        return Ok(children);
    }
    let entry = Path::new(&parent.filename)
        .canonicalize()
        .map_err(|_| "cannot resolve parent module entry")?;
    let root = entry.parent().ok_or("parent module has no directory")?;
    for (name, child) in &config.agents {
        let path = root
            .join(&child.module)
            .canonicalize()
            .map_err(|_| format!("cannot resolve child module {name}"))?;
        if !path.starts_with(root) {
            return Err("child module escapes parent entry directory".into());
        }
        let graph = ModuleGraph::load(&path).map_err(|e| e.to_string())?;
        if !graph.uses_agent_module() {
            return Err("child module must export an ondure agent".into());
        }
        children.insert(name.clone(), graph);
    }
    Ok(children)
}

pub(crate) fn completed(
    journal: &Journal,
    child: &str,
) -> Result<Option<Result<Value, String>>, String> {
    let run = journal.get_run(child).map_err(|e| e.to_string())?;
    if matches!(run.status.as_str(), "running" | "parked") {
        return Ok(None);
    }
    if run.status != "done" {
        return Ok(Some(Err(format!("child agent {child} {}", run.status))));
    }
    let result = journal
        .operations(child)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|op| op.name == "agent.return")
        .and_then(|op| op.result)
        .ok_or("completed child has no recorded return value")?;
    let result: Result<Value, String> = serde_json::from_str(&result).map_err(|e| e.to_string())?;
    Ok(Some(result))
}

pub(crate) fn install_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    ctx: v8::Local<'s, v8::Object>,
) -> Result<(), String> {
    let agents = v8::Object::new(scope);
    let run = v8::Function::new(scope, run).ok_or("child agent callback")?;
    set_property(scope, agents, "run", run.into());
    set_property(scope, ctx, "agents", agents.into());
    Ok(())
}
fn run(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    let result = (|| -> Result<(), String> {
        if !args.get(0).is_string() {
            return Err("child agent name must be a string".into());
        }
        let name = args.get(0).to_rust_string_lossy(scope);
        {
            scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .require_child_agent(&name)?;
        }
        let input = if args.get(1).is_undefined() {
            Value::Null
        } else {
            let json = v8::json::stringify(scope, args.get(1)).ok_or("child input must be JSON")?;
            if json.length() > MAX_OPERATION_ARGUMENT_BYTES {
                return Err("child input exceeds limit".into());
            }
            serde_json::from_str(&json.to_rust_string_lossy(scope))
                .map_err(|_| "invalid child input")?
        };
        validate_json(&input, MAX_OPERATION_ARGUMENT_BYTES, "arguments")?;
        let (mut config, definition, graph, children, path, parent, sequence, depth) = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let config = state
                .host_config
                .clone()
                .ok_or("child agents are not configured")?;
            let definition = config
                .agents
                .get(&name)
                .cloned()
                .ok_or("unknown configured child agent")?;
            let mut config = config;
            config.grants = definition.capabilities.intersection(
                state
                    .effective_capabilities
                    .borrow()
                    .as_ref()
                    .ok_or("permission denied: child agent")?,
            );
            let session = state.session.borrow();
            let session = session
                .as_ref()
                .ok_or("child agents require durable execution")?;
            if session.child_depth >= 8 {
                return Err("child agent nesting exceeds 8 levels".into());
            }
            let graph = session
                .children
                .get(&name)
                .cloned()
                .ok_or("child module was not captured")?;
            (
                config,
                definition,
                graph,
                session.children.clone(),
                session.journal.path().map_err(|e| e.to_string())?,
                session.run_id.clone(),
                session.next_operation_sequence(),
                session.child_depth + 1,
            )
        };
        config.resources.token_limit = Some(definition.allocation.tokens);
        config.resources.dollar_limit_micros = Some(definition.allocation.dollar_micros);
        let request_id = format!("child:{parent}:{sequence}");
        let arguments = json!({"name":name,"input":input,"parent_run_id":parent,"parent_sequence":sequence,"child_request_id":request_id,"allocation":definition.allocation});
        let job = JsonJob {
            name: "agent.run".into(),
            args: arguments,
            network_url: None,
            required_tool: None,
            required_child: Some(name.clone()),
            task: Box::new(move |cancel| {
                cancel.check()?;
                let run = crate::host::run_child_request(
                    &config,
                    &graph,
                    &path,
                    &request_id,
                    &input.to_string(),
                    children,
                    depth,
                )
                .map_err(|e| format!("child execution failed: {}", e.message))?;
                let journal = Journal::open(&path).map_err(|e| e.to_string())?;
                let outcome = completed(&journal, &run.id)?;
                let result = match outcome {
                    Some(Ok(output)) => {
                        let usage = journal.resource_report(&run.id)?;
                        json!({"child_run_id":run.id,"status":"done","output":output,"usage":{"total_tokens":usage["reserved_tokens"]},"spent_dollar_micros":usage["reserved_dollar_micros"]})
                    }
                    Some(Err(error)) => return Err(error),
                    None => json!({"child_run_id":run.id,"status":"parked"}),
                };
                cancel.check()?;
                Ok(result)
            }),
        };
        let retained = v8::Global::new(scope, resolver);
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        let result = state.operations.borrow_mut().json(
            job,
            RecoveryPolicy::RetrySafe,
            retained,
            session.as_mut(),
        );
        result
    })();
    if let Err(error) = result {
        reject(scope, resolver, &error);
    }
    let promise = resolver.get_promise(scope);
    let Some(join) = v8::Function::new(scope, join) else {
        return;
    };
    if let Some(promise) = promise.then(scope, join) {
        output.set(promise.into());
    }
}
fn join(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = (|| -> Result<Option<Value>, String> {
        let text = v8::json::stringify(scope, args.get(0)).ok_or("invalid child launch result")?;
        let result: Value = serde_json::from_str(&text.to_rust_string_lossy(scope))
            .map_err(|_| "invalid child launch result")?;
        if result["status"] == "done" {
            return Ok(Some(result["output"].clone()));
        }
        let child = result["child_run_id"]
            .as_str()
            .ok_or("child launch identity missing")?;
        let resolver = v8::PromiseResolver::new(scope).ok_or("child wait resolver")?;
        let retained = v8::Global::new(scope, resolver);
        {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let mut session = state.session.borrow_mut();
            state.operations.borrow_mut().wait(
                retained,
                "child",
                &json!({"child_run_id":child}),
                None,
                session.as_mut().ok_or("durable child wait required")?,
            )?;
        }
        output.set(resolver.get_promise(scope).into());
        Ok(None)
    })();
    match result {
        Ok(Some(value)) => {
            if let Some(text) = v8::String::new(scope, &value.to_string()) {
                if let Some(value) = v8::json::parse(scope, text) {
                    output.set(value);
                }
            }
        }
        Ok(None) => {}
        Err(error) => {
            let text = v8::String::new(scope, &error).expect("child wait error");
            let error = v8::Exception::error(scope, text);
            scope.throw_exception(error);
        }
    }
}
fn reject(scope: &mut v8::PinScope<'_, '_>, resolver: v8::Local<v8::PromiseResolver>, error: &str) {
    if let Some(text) = v8::String::new(scope, error) {
        let error = v8::Exception::error(scope, text);
        resolver.reject(scope, error);
    }
}
