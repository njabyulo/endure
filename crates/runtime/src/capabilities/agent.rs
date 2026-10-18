//! Durable waits park a run without retaining an isolate or async task.
use crate::{core::embedding::set_property, host::HostState};

pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>) -> Result<(), String> {
    let input = scope
        .get_slot::<HostState>()
        .expect("host state installed")
        .session
        .borrow()
        .as_ref()
        .ok_or("durable session required")?
        .input
        .to_string();
    let source = v8::String::new(scope, &input).ok_or("input exceeds V8 limit")?;
    let input = v8::json::parse(scope, source).ok_or("invalid durable input")?;
    let namespace = v8::Object::new(scope);
    set_property(scope, namespace, "input", input);
    let approval = v8::Function::new(scope, approve).ok_or("approval callback")?;
    set_property(scope, namespace, "approve", approval.into());
    let timer = v8::Function::new(scope, sleep_until).ok_or("sleep callback")?;
    set_property(scope, namespace, "sleepUntil", timer.into());
    let global = scope.get_current_context().global(scope);
    set_property(scope, global, "ondure", namespace.into());
    Ok(())
}

fn approve(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let Some(text) = v8::json::stringify(scope, args.get(0)) else {
        throw(scope, "approval details must be JSON serializable");
        return;
    };
    let details = match serde_json::from_str(&text.to_rust_string_lossy(scope)) {
        Ok(value) => value,
        Err(error) => {
            throw(scope, &error.to_string());
            return;
        }
    };
    register(scope, "approval", details, None, &mut output);
}

fn sleep_until(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let Some(at) = args.get(0).number_value(scope) else {
        return;
    };
    if !at.is_finite() || at < 0.0 || at >= i64::MAX as f64 || at.fract() != 0.0 {
        throw(
            scope,
            "sleepUntil requires an integer epoch millisecond deadline",
        );
        return;
    }
    register(
        scope,
        "timer",
        serde_json::json!([at as i64]),
        Some(at as i64),
        &mut output,
    );
}

fn register(
    scope: &mut v8::PinScope<'_, '_>,
    kind: &str,
    details: serde_json::Value,
    at: Option<i64>,
    output: &mut v8::ReturnValue,
) {
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    let retained = v8::Global::new(scope, resolver);
    let result = {
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        if let Some(session) = session.as_mut() {
            let result = state
                .operations
                .borrow_mut()
                .wait(retained, kind, &details, at, session);
            result
        } else {
            Err("durable session required".into())
        }
    };
    match result {
        Ok(()) => output.set(resolver.get_promise(scope).into()),
        Err(error) => throw(scope, &error),
    }
}
fn throw(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    let message = v8::String::new(scope, message).expect("wait error");
    let error = v8::Exception::error(scope, message);
    scope.throw_exception(error);
}

#[derive(Default)]
struct Agents(
    Vec<(
        v8::Global<v8::Object>,
        v8::Global<v8::Function>,
        serde_json::Value,
    )>,
);

pub(crate) fn evaluate_builtin<'s>(
    context: v8::Local<'s, v8::Context>,
    module: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Value>> {
    // SAFETY: V8 calls module evaluation with this context active.
    v8::callback_scope!(unsafe scope, context);
    let name = v8::String::new(scope, "agent")?;
    let agent = v8::Function::new(scope, define_agent)?;
    module.set_synthetic_module_export(scope, name, agent.into())?;
    let resolver = v8::PromiseResolver::new(scope)?;
    let value = v8::undefined(scope);
    resolver.resolve(scope, value.into())?;
    Some(resolver.get_promise(scope).into())
}

fn define_agent<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut output: v8::ReturnValue,
) {
    if !args.get(0).is_string() {
        throw(scope, "agent name must be a string");
        return;
    }
    let handler = args.get(if args.length() == 2 { 1 } else { 2 });
    let Ok(handler) = v8::Local::<v8::Function>::try_from(handler) else {
        throw(scope, "agent handler must be a function");
        return;
    };
    let options = if args.length() == 2 {
        serde_json::json!({})
    } else {
        let Some(json) = v8::json::stringify(scope, args.get(1)) else {
            throw(scope, "agent options must be JSON");
            return;
        };
        match serde_json::from_str::<serde_json::Value>(&json.to_rust_string_lossy(scope)) {
            Ok(value) if value.is_object() => value,
            _ => {
                throw(scope, "agent options must be a JSON object");
                return;
            }
        }
    };
    let descriptor = v8::Object::new(scope);
    set_property(scope, descriptor, "name", args.get(0));
    descriptor.set_integrity_level(scope, v8::IntegrityLevel::Frozen);
    let definition = (
        v8::Global::new(scope, descriptor),
        v8::Global::new(scope, handler),
        options,
    );
    if scope.get_slot::<Agents>().is_none() {
        scope.set_slot(Agents::default());
    }
    scope
        .get_slot_mut::<Agents>()
        .expect("agents installed")
        .0
        .push(definition);
    output.set(descriptor.into());
}

pub(crate) fn invoke_default(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = (|| -> Result<Option<v8::Local<'_, v8::Value>>, String> {
        let namespace =
            v8::Local::<v8::Object>::try_from(args.data()).map_err(|_| "agent namespace")?;
        let key = v8::String::new(scope, "default").ok_or("default export name")?;
        let default = namespace
            .get(scope, key.into())
            .ok_or("agent default export missing")?;
        let definitions = scope
            .get_slot::<Agents>()
            .ok_or("default export must be an agent")?;
        let definition = definitions
            .0
            .iter()
            .find(|(descriptor, _, _)| {
                // Global equality compares V8 handle identity without entering JS.
                let local = v8::Local::new(scope, descriptor);
                default.strict_equals(local.into())
            })
            .map(|(_, handler, options)| (handler.clone(), options.clone()))
            .ok_or("default export must be an agent")?;
        let requested: crate::Capabilities = serde_json::from_value(
            definition
                .1
                .get("capabilities")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )
        .map_err(|error| error.to_string())?;
        scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .request_capabilities(&requested)?;
        let context = create_context(scope)?;
        let input_json = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow()
            .as_ref()
            .ok_or("agent entrypoints require a durable run")?
            .input
            .to_string();
        let input = v8::String::new(scope, &input_json).ok_or("agent input")?;
        let input = v8::json::parse(scope, input).ok_or("agent input JSON")?;
        let handler = v8::Local::new(scope, &definition.0);
        let receiver = v8::undefined(scope);
        // None leaves V8's original exception pending. Do not replace a
        // handler's synchronous throw with an unrelated host error.
        Ok(handler.call(scope, receiver.into(), &[context.into(), input]))
    })();
    match result {
        Ok(Some(value)) => output.set(value),
        Ok(None) => {}
        Err(error) => throw(scope, &error),
    }
}

pub(crate) fn create_context<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let context = v8::Object::new(scope);
    let sleep = v8::Function::new(scope, sleep_duration).ok_or("sleep callback")?;
    set_property(scope, context, "sleep", sleep.into());
    let human = v8::Object::new(scope);
    let approve = v8::Function::new(scope, approve_agent).ok_or("approval callback")?;
    set_property(scope, human, "approve", approve.into());
    set_property(scope, context, "human", human.into());
    super::memory::install_context(scope, context)?;
    Ok(context)
}

fn duration(
    value: v8::Local<'_, v8::Value>,
    scope: &mut v8::PinScope<'_, '_>,
) -> Result<i64, String> {
    let milliseconds = if value.is_string() {
        let text = value.to_rust_string_lossy(scope);
        let (number, multiplier) = if let Some(n) = text.strip_suffix("ms") {
            (n, 1.0)
        } else if let Some(n) = text.strip_suffix('s') {
            (n, 1000.0)
        } else if let Some(n) = text.strip_suffix('m') {
            (n, 60000.0)
        } else if let Some(n) = text.strip_suffix('h') {
            (n, 3600000.0)
        } else if let Some(n) = text.strip_suffix('d') {
            (n, 86400000.0)
        } else {
            return Err("duration requires ms, s, m, h or d suffix".into());
        };
        number.parse::<f64>().map_err(|_| "invalid duration")? * multiplier
    } else if value.is_number() {
        value.number_value(scope).ok_or("invalid duration")?
    } else {
        return Err("duration must be milliseconds or a duration string".into());
    };
    if !milliseconds.is_finite()
        || milliseconds < 0.0
        || milliseconds.fract() != 0.0
        || milliseconds >= i64::MAX as f64
    {
        return Err("duration must be nonnegative integer milliseconds".into());
    }
    Ok(milliseconds as i64)
}

fn sleep_duration(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = duration(args.get(0), scope).and_then(|duration| {
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        let session = session.as_mut().ok_or("durable session required")?;
        session.sync_op("wait.deadline", &serde_json::json!([duration]), || {
            let now = crate::core::durability::epoch_millis()?;
            Ok(serde_json::json!(now
                .checked_add(duration)
                .ok_or("duration overflow")?))
        })
    });
    match result {
        Ok(at) => register(
            scope,
            "timer",
            serde_json::json!([at]),
            at.as_i64(),
            &mut output,
        ),
        Err(error) => throw(scope, &error),
    }
}

fn approve_agent(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = (|| -> Result<(serde_json::Value, Option<i64>), String> {
        let json =
            v8::json::stringify(scope, args.get(0)).ok_or("approval details must be JSON")?;
        let mut details: serde_json::Value =
            serde_json::from_str(&json.to_rust_string_lossy(scope)).map_err(|e| e.to_string())?;
        if !details.is_object() {
            return Err("approval details must be an object".into());
        }
        details["resultFormat"] = serde_json::json!("agent-v1");
        let at = if let Some(timeout) = details.get("timeout") {
            if details
                .get("onTimeout")
                .and_then(|v| v.as_str())
                .unwrap_or("reject")
                != "reject"
            {
                return Err("only onTimeout: reject is supported".into());
            }
            let timeout_json = v8::String::new(scope, &timeout.to_string()).ok_or("timeout")?;
            let timeout = v8::json::parse(scope, timeout_json).ok_or("timeout")?;
            let duration = duration(timeout, scope)?;
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let mut session = state.session.borrow_mut();
            let at = session
                .as_mut()
                .ok_or("durable session required")?
                .sync_op("wait.deadline", &serde_json::json!([duration]), || {
                    Ok(serde_json::json!(crate::core::durability::epoch_millis()?
                        .checked_add(duration)
                        .ok_or("duration overflow")?))
                })?;
            Some(at.as_i64().ok_or("invalid deadline")?)
        } else {
            None
        };
        Ok((details, at))
    })();
    match result {
        Ok((details, at)) => register(scope, "approval", details, at, &mut output),
        Err(error) => throw(scope, &error),
    }
}

pub(crate) fn dispose(isolate: &mut v8::Isolate) {
    isolate.remove_slot::<Agents>();
}
