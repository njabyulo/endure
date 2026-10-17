//! Console bindings and captured standard output/error.
//!
//! Callbacks read Rust-owned buffers from the host's isolate slot. RefCell
//! permits buffer updates through that shared state; borrowing happens only
//! after conversion because JavaScript toString can re-enter console.
//! Tests below cover captured streams and exceptions during conversion.

use crate::{core::embedding::set_property, host::HostState};

pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>) {
    let console = v8::Object::new(scope);
    let log = v8::Function::new(scope, console_log).expect("console.log");
    let error = v8::Function::new(scope, console_error).expect("console.error");
    set_property(scope, console, "log", log.into());
    set_property(scope, console, "error", error.into());
    let global = scope.get_current_context().global(scope);
    set_property(scope, global, "console", console.into());
}

fn console_log(
    scope: &mut v8::PinScope<'_, '_>,
    arguments: v8::FunctionCallbackArguments,
    _return_value: v8::ReturnValue,
) {
    write_console(scope, arguments, false);
}

fn console_error(
    scope: &mut v8::PinScope<'_, '_>,
    arguments: v8::FunctionCallbackArguments,
    _return_value: v8::ReturnValue,
) {
    write_console(scope, arguments, true);
}

fn write_console(
    scope: &mut v8::PinScope<'_, '_>,
    arguments: v8::FunctionCallbackArguments,
    is_error: bool,
) {
    let mut values = Vec::new();
    for index in 0..arguments.length() {
        let Some(value) = arguments.get(index).to_string(scope) else {
            // Leave the pending JavaScript exception for the caller's catch.
            return;
        };
        values.push(value.to_rust_string_lossy(scope));
    }
    let mut line = values.join(" ");
    line.push('\n');
    // Borrow only after stringification: toString may re-enter console.
    if let Some(state) = scope.get_slot::<HostState>() {
        let buffer = if is_error {
            &state.stderr
        } else {
            &state.stdout
        };
        buffer.borrow_mut().push_str(&line);
    }
}

#[cfg(test)]
mod tests {
    use crate::{run_source, Policy};

    #[test]
    fn console_captures_both_streams() {
        let output = run_source(
            &Policy::default(),
            "console.js",
            "console.log('hello', 2); console.error('err');",
        )
        .unwrap();
        assert_eq!(output.stdout, "hello 2\n");
        assert_eq!(output.stderr, "err\n");
    }

    #[test]
    fn console_conversion_exception_can_be_caught() {
        let output = run_source(
            &Policy::default(),
            "convert.js",
            "try { console.log({ toString() { throw Error('conversion'); } }); } catch(e) { console.error(e); }",
        ).unwrap();
        assert_eq!(output.stdout, "");
        assert_eq!(output.stderr, "Error: conversion\n");
    }
}
