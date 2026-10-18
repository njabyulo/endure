//! GET-only fetch: authorize, register work, and build responses on completion.
//! HTTP runs on Tokio's blocking pool; V8 handles stay on the host thread.

use crate::MAX_OPERATION_RESULT_BYTES;
use crate::{core::embedding::set_property, host::HostState};
use std::{io::Read, time::Duration};

// Bound each HTTP operation, including its response body read.
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
    let allowed = scope.get_slot::<HostState>().is_some_and(|state| {
        state
            .effective_capabilities
            .borrow()
            .as_ref()
            .map_or(state.policy.allows_net(), |caps| {
                !caps.network_origins.is_empty()
            })
    });
    if !allowed {
        throw_error(scope, "permission denied: net");
        return;
    }
    let Some(url) = arguments.get(0).to_string(scope) else {
        return;
    };
    let url = url.to_rust_string_lossy(scope);
    let authorization = scope
        .get_slot::<HostState>()
        .expect("host state installed")
        .require_network(&url);
    if let Err(error) = authorization {
        throw_error(scope, &error);
        return;
    }
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        return;
    };
    let retained = v8::Global::new(scope, resolver);
    let result = {
        let state = scope.get_slot::<HostState>().expect("host state installed");
        let mut session = state.session.borrow_mut();
        let result = state
            .operations
            .borrow_mut()
            .fetch(url, retained, session.as_mut());
        result
    };
    if let Err(error) = result {
        throw_error(scope, &error);
        return;
    }
    return_value.set(resolver.get_promise(scope).into());
}

pub(crate) fn complete(
    scope: &mut v8::PinScope<'_, '_>,
    resolver: v8::Local<v8::PromiseResolver>,
    result: Result<(u16, String), String>,
) {
    let (status, body) = match result {
        Ok(response) => response,
        Err(error) => {
            reject(scope, resolver, &format!("fetch failed: {error}"));
            return;
        }
    };
    let Some(body) = v8::String::new(scope, &body) else {
        reject(scope, resolver, "response exceeds V8 string limit");
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
    resolver.resolve(scope, response.into());
}

pub(crate) fn http_get(url: &str, allow_redirects: bool) -> Result<(u16, String), String> {
    let client = ureq::AgentBuilder::new()
        .redirects(if allow_redirects { 5 } else { 0 })
        .build();
    let response = match client.get(url).timeout(HTTP_TIMEOUT).call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(error.to_string()),
    };
    let status = response.status();
    if !allow_redirects && (300..400).contains(&status) && response.header("Location").is_some() {
        return Err(
            "permission denied: automatic redirects are disabled for scoped network access".into(),
        );
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_OPERATION_RESULT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_OPERATION_RESULT_BYTES {
        return Err(format!(
            "host operation response exceeds {} bytes",
            MAX_OPERATION_RESULT_BYTES
        ));
    }
    let body = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    Ok((status, body))
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

fn reject(
    scope: &mut v8::PinScope<'_, '_>,
    resolver: v8::Local<v8::PromiseResolver>,
    message: &str,
) {
    let message = v8::String::new(scope, message).expect("fetch error message");
    let error = v8::Exception::error(scope, message);
    resolver.reject(scope, error);
}
