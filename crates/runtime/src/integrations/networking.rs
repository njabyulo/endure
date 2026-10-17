//! Network integration: JavaScript fetch, blocking HTTP, and response promises.
//!
//! Permission enforcement stays inside the native callback, before URL
//! conversion or HTTP work. Returning a Promise does not make the HTTP call
//! asynchronous; microtasks resume JavaScript after the callback returns.
//! The network integration tests use real listeners to prove denial precedes
//! sockets and allowed responses complete both await continuations.

use crate::{core::embedding::set_property, host::HostState};
use std::time::Duration;

// Bound blocking HTTP work on the callback thread.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>) {
    let fetch = v8::Function::new(scope, fetch).expect("fetch callback");
    let global = scope.get_current_context().global(scope);
    set_property(scope, global, "fetch", fetch.into());
}

fn fetch(
    scope: &mut v8::PinScope<'_, '_>,
    arguments: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    // Authorize before touching the URL, HTTP client, DNS, or any socket.
    let allowed = scope
        .get_slot::<HostState>()
        .is_some_and(|state| state.policy.allows_net());
    if !allowed {
        throw_error(scope, "permission denied: net");
        return;
    }

    let Some(url) = arguments.get(0).to_string(scope) else {
        return;
    };
    let url = url.to_rust_string_lossy(scope);
    let (status, body) = match http_get(&url) {
        Ok(response) => response,
        Err(error) => {
            throw_error(scope, &format!("fetch failed: {error}"));
            return;
        }
    };
    let Some(body) = v8::String::new(scope, &body) else {
        throw_error(scope, "response exceeds V8 string limit");
        return;
    };
    let response = v8::Object::new(scope);
    let ok = v8::Boolean::new(scope, (200..300).contains(&status));
    let status = v8::Integer::new_from_unsigned(scope, u32::from(status));
    // Callback data retains the original body even if script edits _body.
    let Some(json) = v8::Function::builder(response_json)
        .data(body.into())
        .build(scope)
    else {
        return;
    };
    set_property(scope, response, "ok", ok.into());
    set_property(scope, response, "status", status.into());
    set_property(scope, response, "json", json.into());
    set_property(scope, response, "_body", body.into());
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    if resolver.resolve(scope, response.into()).is_some() {
        return_value.set(resolver.get_promise(scope).into());
    }
}

fn http_get(url: &str) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let response = match ureq::get(url).timeout(HTTP_TIMEOUT).call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(Box::new(error)),
    };
    let status = response.status();
    Ok((status, response.into_string()?))
}

fn response_json(
    scope: &mut v8::PinScope<'_, '_>,
    arguments: v8::FunctionCallbackArguments,
    mut return_value: v8::ReturnValue,
) {
    let Ok(body) = v8::Local::<v8::String>::try_from(arguments.data()) else {
        throw_error(scope, "response body missing");
        return;
    };
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    v8::tc_scope!(let scope, scope);
    if let Some(value) = v8::json::parse(scope, body) {
        resolver.resolve(scope, value);
    } else {
        let Some(exception) = scope.exception() else {
            return;
        };
        scope.reset();
        resolver.reject(scope, exception);
    }
    return_value.set(resolver.get_promise(scope).into());
}

fn throw_error(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    let text = v8::String::new(scope, message).expect("error message");
    let exception = v8::Exception::error(scope, text);
    scope.throw_exception(exception);
}
