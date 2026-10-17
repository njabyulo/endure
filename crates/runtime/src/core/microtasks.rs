//! Promise continuations and tracking of unhandled rejections.
//!
//! A resolved Promise resumes await through microtasks, which this host drains
//! explicitly. Global handles retain promise identity and rejection reasons
//! across callback scopes; a later handler removes only its matching promise.
//! Tests below demonstrate nested awaits, late handlers, and discarded errors.

use crate::host::HostState;
use std::cell::RefCell;

#[derive(Default)]
pub(crate) struct PromiseRejections {
    pending: RefCell<Vec<PendingRejection>>,
}

struct PendingRejection {
    promise: v8::Global<v8::Promise>,
    reason: v8::Global<v8::Value>,
}

pub(crate) fn install(isolate: &mut v8::Isolate) {
    isolate.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
    isolate.set_promise_reject_callback(promise_rejected);
}

pub(crate) fn drain(scope: &mut v8::PinScope<'_, '_>) {
    scope.perform_microtask_checkpoint();
}

pub(crate) fn take_unhandled_rejection(scope: &mut v8::PinScope<'_, '_>) -> Option<String> {
    // Release the state borrow before converting a reason: toString may run JS.
    let pending = scope
        .get_slot::<HostState>()
        .expect("host state installed")
        .rejections
        .pending
        .take();
    pending.first().map(|rejection| {
        let reason = v8::Local::new(scope, &rejection.reason);
        format!(
            "unhandled promise rejection: {}",
            reason.to_rust_string_lossy(scope)
        )
    })
}

extern "C" fn promise_rejected(message: v8::PromiseRejectMessage) {
    // SAFETY: V8 invokes this callback with the message's isolate entered.
    v8::callback_scope!(unsafe scope, &message);
    let promise = v8::Global::new(scope, message.get_promise());
    match message.get_event() {
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler => {
            if let Some(reason) = message.get_value() {
                let reason = v8::Global::new(scope, reason);
                if let Some(state) = scope.get_slot::<HostState>() {
                    state
                        .rejections
                        .pending
                        .borrow_mut()
                        .push(PendingRejection { promise, reason });
                }
            }
        }
        v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject => {
            if let Some(state) = scope.get_slot::<HostState>() {
                state
                    .rejections
                    .pending
                    .borrow_mut()
                    .retain(|pending| pending.promise != promise);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::{run_source, Policy};

    #[test]
    fn microtasks_complete_nested_awaits() {
        let output = run_source(
            &Policy::default(),
            "await.js",
            "(async () => { await Promise.resolve(); await Promise.resolve(); console.log('drained'); })();",
        ).unwrap();
        assert_eq!(output.stdout, "drained\n");
    }

    #[test]
    fn unhandled_async_error_fails() {
        let error = run_source(
            &Policy::default(),
            "async-error.js",
            "(async () => { await Promise.resolve(); throw Error('async boom'); })();",
        )
        .unwrap_err();
        assert!(error.message.contains("async boom"));
    }

    #[test]
    fn discarded_rejection_fails_even_when_script_returns_number() {
        let error = run_source(
            &Policy::default(),
            "discard.js",
            "Promise.reject(Error('discarded')); 42;",
        )
        .unwrap_err();
        assert!(error.message.contains("discarded"));
    }

    #[test]
    fn handler_attached_in_microtask_clears_rejection() {
        let output = run_source(
            &Policy::default(),
            "handled.js",
            "const pending = Promise.reject(Error('handled')); Promise.resolve().then(() => pending.catch(e => console.error(e)));",
        ).unwrap();
        assert_eq!(output.stderr, "Error: handled\n");
    }

    #[test]
    fn handling_one_rejection_does_not_clear_another() {
        let error = run_source(
            &Policy::default(),
            "multiple.js",
            "Promise.reject(Error('remaining')); Promise.reject(Error('handled')).catch(() => {});",
        )
        .unwrap_err();
        assert!(error.message.contains("remaining"));
    }
}
