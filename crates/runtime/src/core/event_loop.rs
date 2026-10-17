//! Host operations bridge owned Rust results to V8 promises and callbacks.
//!
//! Only HTTP work crosses threads. The registry and all persistent JS handles
//! stay with the isolate. Every delivered completion gets its own handle scope
//! and microtask checkpoint before another completion can enter JavaScript.

use crate::{host::HostState, integrations::networking};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, task::JoinSet};

pub(crate) type OperationId = u32;

#[derive(Default)]
pub(crate) struct Operations {
    next_id: OperationId,
    queued: Vec<(OperationId, String)>,
    fetches: BTreeMap<OperationId, v8::Global<v8::PromiseResolver>>,
    timers: BTreeMap<OperationId, Timer>,
}

struct Timer {
    deadline: Instant,
    callback: v8::Global<v8::Function>,
    arguments: Vec<v8::Global<v8::Value>>,
}

impl Operations {
    fn id(&mut self) -> OperationId {
        // Never reuse a live ID, even after the counter wraps.
        loop {
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.fetches.contains_key(&self.next_id) && !self.timers.contains_key(&self.next_id)
            {
                return self.next_id;
            }
        }
    }

    pub(crate) fn fetch(&mut self, url: String, resolver: v8::Global<v8::PromiseResolver>) {
        let id = self.id();
        self.fetches.insert(id, resolver);
        self.queued.push((id, url));
    }

    pub(crate) fn timer(
        &mut self,
        delay: Duration,
        callback: v8::Global<v8::Function>,
        arguments: Vec<v8::Global<v8::Value>>,
    ) -> OperationId {
        let id = self.id();
        self.timers.insert(
            id,
            Timer {
                deadline: Instant::now() + delay,
                callback,
                arguments,
            },
        );
        id
    }

    pub(crate) fn cancel_timer(&mut self, id: OperationId) {
        self.timers.remove(&id);
    }

    fn next_timer(&self) -> Option<(OperationId, Instant)> {
        self.timers
            .iter()
            .map(|(&id, timer)| (id, timer.deadline))
            .min_by_key(|&(id, deadline)| (deadline, id))
    }
}

type HttpResult = (OperationId, Result<(u16, String), String>);
enum Completion {
    Http(HttpResult),
    Timer(OperationId),
}

async fn next_completion(
    tasks: &mut JoinSet<HttpResult>,
    timer: Option<(OperationId, Instant)>,
) -> Completion {
    let timer_ready = async {
        match timer {
            Some((id, deadline)) => {
                tokio::time::sleep_until(deadline.into()).await;
                id
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = tasks.join_next(), if !tasks.is_empty() => {
            // The wrapper task only awaits a worker and constructs owned data.
            Completion::Http(result.expect("active HTTP task").expect("HTTP wrapper task"))
        }
        id = timer_ready => Completion::Timer(id),
    }
}

pub(crate) fn run(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
    mut failure: Option<String>,
) -> Result<(), String> {
    let mut tasks = JoinSet::new();
    loop {
        {
            v8::scope!(let scope, scope);
            v8::tc_scope!(let scope, scope);
            super::microtasks::drain(scope);
            if let Some(exception) = scope.exception() {
                let message = exception.to_rust_string_lossy(scope);
                failure.get_or_insert(message);
                scope.reset();
            }
        }
        // Release the isolate-slot borrow before any JavaScript can run.
        let (queued, timer) = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let mut ops = state.operations.borrow_mut();
            (std::mem::take(&mut ops.queued), ops.next_timer())
        };
        for (id, url) in queued {
            let _entered = runtime.enter();
            tasks.spawn(async move {
                let result = tokio::task::spawn_blocking(move || networking::http_get(&url))
                    .await
                    .unwrap_or_else(|error| Err(format!("HTTP worker failed: {error}")));
                (id, result)
            });
        }
        if tasks.is_empty() && timer.is_none() {
            // Rejection stringification can itself run JS and enqueue work.
            let rejection = super::microtasks::take_unhandled_rejection(scope);
            if let Some(message) = rejection {
                failure.get_or_insert(message);
                // A user-defined rejection toString may schedule microtasks.
                // Checkpoint again before deciding that the runtime is idle.
                continue;
            }
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let ops = state.operations.borrow();
            if ops.queued.is_empty() && ops.timers.is_empty() {
                break;
            }
            continue;
        }
        let completion = runtime.block_on(next_completion(&mut tasks, timer));
        v8::scope!(let scope, scope);
        v8::tc_scope!(let scope, scope);
        match completion {
            Completion::Http((id, result)) => {
                let retained = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .fetches
                    .remove(&id)
                    .expect("pending fetch");
                let resolver = v8::Local::new(scope, &retained);
                networking::complete(scope, resolver, result);
            }
            Completion::Timer(id) => {
                let timer = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .timers
                    .remove(&id);
                if let Some(timer) = timer {
                    let callback = v8::Local::new(scope, &timer.callback);
                    let args: Vec<_> = timer
                        .arguments
                        .iter()
                        .map(|arg| v8::Local::new(scope, arg))
                        .collect();
                    let receiver = scope.get_current_context().global(scope).into();
                    callback.call(scope, receiver, &args);
                }
            }
        }
        if let Some(exception) = scope.exception() {
            let message = exception.to_rust_string_lossy(scope);
            failure.get_or_insert(message);
            scope.reset();
        }
    }
    failure.map_or(Ok(()), Err)
}
