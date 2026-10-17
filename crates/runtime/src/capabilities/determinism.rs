//! Journal nondeterministic JavaScript globals during durable runs.
//!
//! V8 handles stay on the isolate thread. Each native callback records one
//! synchronous operation, or returns its recorded value during replay. Date
//! formatting/parsing with explicit arguments still follows the host timezone;
//! applications that exchange dates should use UTC or explicit offsets.

use crate::host::HostState;
use serde_json::{json, Value};
use std::{
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// Install before user code, and only when a durable session is present.
pub(crate) fn install(scope: &mut v8::PinScope<'_, '_>) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);
    let date_key = v8::String::new(scope, "Date").ok_or("Date key allocation failed")?;
    let native_date = global
        .get(scope, date_key.into())
        .ok_or("Date unavailable")?;
    let date = v8::Function::new(scope, date_now).ok_or("Date callback allocation failed")?;
    let text = v8::Function::builder(date_string)
        .data(native_date)
        .build(scope)
        .ok_or("Date string callback allocation failed")?;
    let performance = v8::Function::new(scope, performance_now)
        .ok_or("performance callback allocation failed")?;
    let random =
        v8::Function::new(scope, math_random).ok_or("random callback allocation failed")?;
    let uuid = v8::Function::new(scope, random_uuid).ok_or("UUID callback allocation failed")?;
    let source =
        v8::String::new(scope, INSTALL).ok_or("deterministic globals allocation failed")?;
    let script = v8::Script::compile(scope, source, None)
        .ok_or("deterministic globals compilation failed")?;
    let function = script
        .run(scope)
        .ok_or("deterministic globals initialization failed")?;
    let function = v8::Local::<v8::Function>::try_from(function)
        .map_err(|_| "deterministic globals installer is not a function")?;
    let receiver = v8::undefined(scope);
    function
        .call(
            scope,
            receiver.into(),
            &[
                native_date,
                date.into(),
                text.into(),
                performance.into(),
                random.into(),
                uuid.into(),
            ],
        )
        .ok_or("deterministic globals installation failed")?;
    Ok(())
}

// NativeDate remains in this lexical closure; neither Date's prototype chain
// nor Date.prototype.constructor exposes the original no-argument constructor.
const INSTALL: &str = r#"(function(NativeDate, wall, dateText, perf, random, uuid) {
    const construct = Reflect.construct;
    const apply = Reflect.apply;
    const DateRequired = TypeError;
    // Intl's omitted date reads V8's clock directly, bypassing Date.now.
    // Keep explicit-date formatting while closing those implicit clock reads.
    if (typeof Intl !== 'undefined') {
        const prototype = Intl.DateTimeFormat.prototype;
        const formatGetter = Object.getOwnPropertyDescriptor(prototype, 'format').get;
        const formatToParts = prototype.formatToParts;
        const formats = new WeakMap();
        const cacheGet = WeakMap.prototype.get;
        const cacheSet = WeakMap.prototype.set;
        Object.defineProperty(prototype, 'format', {
            configurable: true,
            get: function() {
                let format = apply(cacheGet, formats, [this]);
                if (!format) {
                    const original = apply(formatGetter, this, []);
                    format = function(date) {
                        if (date === undefined) throw new DateRequired('durable Intl formatting requires an explicit date');
                        return original(date);
                    };
                    apply(cacheSet, formats, [this, format]);
                }
                return format;
            }
        });
        Object.defineProperty(prototype, 'formatToParts', {
            configurable: true, writable: true,
            value: function(date) {
                if (date === undefined) throw new DateRequired('durable Intl formatting requires an explicit date');
                return apply(formatToParts, this, [date]);
            }
        });
    }
    if (typeof Temporal !== 'undefined') delete Temporal.Now;
    function Date(...args) {
        if (!new.target) return dateText();
        return construct(NativeDate, args.length ? args : [wall()], new.target);
    }
    Object.defineProperty(Date, 'length', { value: 7 });
    Date.prototype = NativeDate.prototype;
    Object.defineProperty(Date.prototype, 'constructor', {
        value: Date, writable: true, configurable: true
    });
    Object.defineProperties(Date, {
        now: { value: wall, writable: true, configurable: true },
        parse: { value: NativeDate.parse, writable: true, configurable: true },
        UTC: { value: NativeDate.UTC, writable: true, configurable: true }
    });
    globalThis.Date = Date;
    globalThis.performance = { now: perf };
    Math.random = random;
    globalThis.crypto = { randomUUID: uuid };
    delete globalThis.WeakRef;
    delete globalThis.FinalizationRegistry;
})"#;

fn journal(
    scope: &mut v8::PinScope<'_, '_>,
    name: &str,
    produce: impl FnOnce() -> Result<Value, String>,
) -> Result<Value, String> {
    let state = scope
        .get_slot::<HostState>()
        .ok_or("host state unavailable")?;
    let mut session = state.session.borrow_mut();
    let session = session.as_mut().ok_or("durable session unavailable")?;
    session.sync_op(name, &json!([]), produce)
}

fn date_now(
    scope: &mut v8::PinScope<'_, '_>,
    _: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = journal(scope, "clock.date", || {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        Ok(json!(elapsed.as_millis() as f64))
    });
    return_number(scope, result, &mut output);
}

fn date_string(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    // Convert the native result before borrowing session state. Replaying records
    // the displayed string too, so Date() does not change with timezone settings.
    let Ok(original) = v8::Local::<v8::Function>::try_from(args.data()) else {
        throw(scope, "original Date unavailable");
        return;
    };
    let receiver = v8::undefined(scope);
    let Some(value) = original.call(scope, receiver.into(), &[]) else {
        return;
    };
    let text = value.to_rust_string_lossy(scope);
    let result = journal(scope, "clock.date_string", || Ok(json!(text)));
    return_string(scope, result, &mut output);
}

fn performance_now(
    scope: &mut v8::PinScope<'_, '_>,
    _: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = journal(scope, "clock.performance", || {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        Ok(json!(
            ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
        ))
    });
    return_number(scope, result, &mut output);
}

fn math_random(
    scope: &mut v8::PinScope<'_, '_>,
    _: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = journal(scope, "random.math", || {
        // The first 48 UUID bits are random; later bytes contain version bits.
        // Combining those bits with the final byte gives 53 unbiased bits.
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let mut bits = 0u64;
        for byte in &bytes[..6] {
            bits = (bits << 8) | u64::from(*byte);
        }
        bits = (bits << 5) | u64::from(bytes[15] & 31);
        Ok(json!(bits as f64 / ((1u64 << 53) as f64)))
    });
    return_number(scope, result, &mut output);
}

fn random_uuid(
    scope: &mut v8::PinScope<'_, '_>,
    _: v8::FunctionCallbackArguments,
    mut output: v8::ReturnValue,
) {
    let result = journal(scope, "random.uuid", || {
        Ok(json!(uuid::Uuid::new_v4().to_string()))
    });
    return_string(scope, result, &mut output);
}

fn return_number(
    scope: &mut v8::PinScope<'_, '_>,
    result: Result<Value, String>,
    output: &mut v8::ReturnValue,
) {
    match result {
        Ok(value) => match value.as_f64().filter(|number| number.is_finite()) {
            Some(number) => output.set(v8::Number::new(scope, number).into()),
            None => throw(scope, "invalid numeric result in durable journal"),
        },
        Err(error) => throw(scope, &error),
    }
}

fn return_string(
    scope: &mut v8::PinScope<'_, '_>,
    result: Result<Value, String>,
    output: &mut v8::ReturnValue,
) {
    match result {
        Ok(value) => match value.as_str().and_then(|text| v8::String::new(scope, text)) {
            Some(text) => output.set(text.into()),
            None => throw(scope, "invalid string result in durable journal"),
        },
        Err(error) => throw(scope, &error),
    }
}

fn throw(scope: &mut v8::PinScope<'_, '_>, text: &str) {
    if let Some(message) = v8::String::new(scope, text) {
        let error = v8::Exception::error(scope, message);
        scope.throw_exception(error);
    }
}

#[cfg(test)]
mod tests {
    use super::INSTALL;
    use crate::{run_source, Policy};

    // Exercise Date compatibility separately from journal storage/replay tests.
    // Constant providers model the recorded values returned by native callbacks.
    fn run_wrapped(source: &str) -> String {
        let source = format!(
            "({INSTALL})(Date, () => 1700000000000, () => 'recorded date', () => 12.5, () => 0.25, () => 'recorded-uuid');\n{source}"
        );
        run_source(&Policy::default(), "deterministic-globals.js", &source)
            .unwrap()
            .stdout
    }

    #[test]
    fn wrapper_preserves_explicit_dates_and_constructor_identity() {
        assert_eq!(
            run_wrapped(
                r#"
                class ChildDate extends Date {}
                console.log(new Date().getTime() === 1700000000000);
                console.log(new Date(0).toISOString() === '1970-01-01T00:00:00.000Z');
                console.log(new Date('2000-01-01T00:00:00Z').getTime() === 946684800000);
                console.log(new ChildDate() instanceof ChildDate, new ChildDate() instanceof Date);
                console.log(Date.prototype.constructor === Date);
                console.log(Object.getPrototypeOf(Date) === Function.prototype);
                console.log(Date.length === 7);
                console.log(Date.parse('2000-01-01T00:00:00Z') === Date.UTC(2000, 0, 1));
                "#
            ),
            "true\ntrue\ntrue\ntrue true\ntrue\ntrue\ntrue\ntrue\n"
        );
    }

    #[test]
    fn wrapper_exposes_recorded_values_and_removes_weak_references() {
        assert_eq!(
            run_wrapped(
                r#"
                console.log(Date.now(), Date(), performance.now(), Math.random(), crypto.randomUUID());
                console.log(typeof WeakRef, typeof FinalizationRegistry);
                "#
            ),
            "1700000000000 recorded date 12.5 0.25 recorded-uuid\nundefined undefined\n"
        );
    }

    #[test]
    fn implicit_intl_clocks_and_temporal_now_are_unavailable() {
        assert_eq!(
            run_wrapped(
                r#"
                const formatter = new Intl.DateTimeFormat('en-GB', {year: 'numeric', timeZone: 'UTC'});
                for (const call of [() => formatter.format(), () => formatter.format(undefined),
                    () => formatter.formatToParts(), () => formatter.formatToParts(undefined)]) {
                    try { call(); console.log('unexpected'); }
                    catch (error) { console.log(error instanceof TypeError); }
                }
                console.log(typeof Temporal.Now);
                console.log(formatter.format(0), formatter.format(new Date(0)));
                console.log(formatter.formatToParts(0).find(part => part.type === 'year').value);
                console.log(formatter.format === formatter.format);
                "#
            ),
            "true\ntrue\ntrue\ntrue\nundefined\n1970 1970\n1970\ntrue\n"
        );
    }

    #[test]
    fn patched_reflect_apply_cannot_capture_original_intl_formatter() {
        assert_eq!(
            run_wrapped(
                r#"
                let captured = false;
                const originalApply = Reflect.apply;
                Reflect.apply = function(...args) { captured = true; return originalApply(...args); };
                const formatter = new Intl.DateTimeFormat('en-GB', {year: 'numeric', timeZone: 'UTC'});
                console.log(formatter.format(0));
                console.log(formatter.formatToParts(0).find(part => part.type === 'year').value);
                console.log(captured);
                "#
            ),
            "1970\n1970\nfalse\n"
        );
    }
}
