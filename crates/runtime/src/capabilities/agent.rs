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
