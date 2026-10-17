//! One-shot timers retain callbacks in the host until delivery or cancellation.

use crate::{core::embedding::set_property, host::HostState};
use std::time::Duration;

pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>) {
    let global = scope.get_current_context().global(scope);
    let timeout = v8::Function::new(scope, set_timeout).expect("setTimeout callback");
    set_property(scope, global, "setTimeout", timeout.into());
    let clear = v8::Function::new(scope, clear_timeout).expect("clearTimeout callback");
    set_property(scope, global, "clearTimeout", clear.into());
}

fn set_timeout(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let Ok(callback) = v8::Local::<v8::Function>::try_from(args.get(0)) else {
        let message = v8::String::new(scope, "setTimeout callback must be a function").unwrap();
        let error = v8::Exception::type_error(scope, message);
        scope.throw_exception(error);
        return;
    };
    let Some(delay) = args.get(1).number_value(scope) else {
        return;
    };
    let delay = if delay.is_nan() || delay < 0.0 {
        0.0
    } else {
        delay.min(i32::MAX as f64)
    };
    let callback = v8::Global::new(scope, callback);
    let arguments = (2..args.length())
        .map(|index| v8::Global::new(scope, args.get(index)))
        .collect();
    let id = scope
        .get_slot::<HostState>()
        .expect("host state installed")
        .operations
        .borrow_mut()
        .timer(Duration::from_millis(delay as u64), callback, arguments);
    rv.set(v8::Integer::new_from_unsigned(scope, id).into());
}

fn clear_timeout(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let Some(id) = args.get(0).number_value(scope) else {
        return;
    };
    if id.is_finite() && id >= 1.0 && id <= u32::MAX as f64 && id.fract() == 0.0 {
        scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .operations
            .borrow_mut()
            .cancel_timer(id as u32);
    }
}
