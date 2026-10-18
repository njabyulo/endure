//! Host operations bridge owned Rust results to V8 promises and callbacks.
//!
//! Only HTTP work crosses threads. The registry and all persistent JS handles
//! stay with the isolate. Every delivered completion gets its own handle scope
//! and microtask checkpoint before another completion can enter JavaScript.

use super::operations::{
    validate_json, CancellationGuard, OperationCancellation, OperationSpec, RecoveryPolicy,
    MAX_OPERATION_RESULT_BYTES,
};
use crate::{host::HostState, integrations::networking};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, task::JoinSet};

pub(crate) type OperationId = u32;
/// Deadline cancellation detached owned workers; runtime drop must not wait.
pub(crate) struct DetachedHostWork;
pub(crate) type JsonTask =
    Box<dyn FnOnce(OperationCancellation) -> Result<serde_json::Value, String> + Send>;
/// Owned adapter work: no V8 handles or resolved credentials cross this boundary.
pub(crate) struct JsonJob {
    pub name: String,
    pub args: serde_json::Value,
    pub network_url: Option<String>,
    pub required_tool: Option<String>,
    pub task: JsonTask,
}

#[derive(Default)]
pub(crate) struct Operations {
    next_id: OperationId,
    pub(crate) json_queued: Vec<(OperationId, JsonJob)>,
    json_resolvers: BTreeMap<OperationId, v8::Global<v8::PromiseResolver>>,
    queued: Vec<(OperationId, String)>,
    fetches: BTreeMap<OperationId, v8::Global<v8::PromiseResolver>>,
    waits: BTreeMap<OperationId, v8::Global<v8::PromiseResolver>>,
    timers: BTreeMap<OperationId, Timer>,
    deadlines: BTreeSet<(Instant, OperationId)>,
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
            if !self.json_resolvers.contains_key(&self.next_id)
                && !self.fetches.contains_key(&self.next_id)
                && !self.timers.contains_key(&self.next_id)
                && !self.waits.contains_key(&self.next_id)
            {
                return self.next_id;
            }
        }
    }

    pub(crate) fn json(
        &mut self,
        job: JsonJob,
        recovery: RecoveryPolicy,
        resolver: v8::Global<v8::PromiseResolver>,
        session: Option<&mut super::durability::Session>,
    ) -> Result<(), String> {
        let id = self.id();
        if let Some(session) = session {
            session.register(id, &job.name, &job.args, recovery)?;
        } else {
            OperationSpec::new(&job.name, recovery).validate(&job.args)?;
        }
        self.json_resolvers.insert(id, resolver);
        self.json_queued.push((id, job));
        Ok(())
    }

    pub(crate) fn fetch(
        &mut self,
        url: String,
        resolver: v8::Global<v8::PromiseResolver>,
        session: Option<&mut super::durability::Session>,
    ) -> Result<(), String> {
        let id = self.id();
        let args = serde_json::json!([url]);
        if let Some(session) = session {
            session.register(id, "fetch.get", &args, RecoveryPolicy::RetrySafe)?;
        } else {
            OperationSpec::new("fetch.get", RecoveryPolicy::RetrySafe).validate(&args)?;
        }
        self.fetches.insert(id, resolver);
        self.queued.push((id, url));
        Ok(())
    }

    pub(crate) fn timer(
        &mut self,
        delay: Duration,
        callback: v8::Global<v8::Function>,
        arguments: Vec<v8::Global<v8::Value>>,
        session: Option<&mut super::durability::Session>,
    ) -> Result<OperationId, String> {
        let id = self.id();
        if let Some(session) = session {
            session.register(
                id,
                "timer.sleep",
                &serde_json::json!([delay.as_millis() as u64]),
                RecoveryPolicy::RetrySafe,
            )?;
        }
        let deadline = Instant::now() + delay;
        self.deadlines.insert((deadline, id));
        self.timers.insert(
            id,
            Timer {
                deadline,
                callback,
                arguments,
            },
        );
        Ok(id)
    }

    pub(crate) fn wait(
        &mut self,
        resolver: v8::Global<v8::PromiseResolver>,
        kind: &str,
        args: &serde_json::Value,
        wake_at: Option<i64>,
        session: &mut super::durability::Session,
    ) -> Result<(), String> {
        let id = self.id();
        session.register_wait(id, kind, args, wake_at)?;
        self.waits.insert(id, resolver);
        Ok(())
    }

    pub(crate) fn cancel_timer(
        &mut self,
        id: OperationId,
        session: Option<&mut super::durability::Session>,
    ) -> Result<(), String> {
        if !self.timers.contains_key(&id) {
            return Ok(());
        }
        if let Some(session) = session {
            session.cancel(id)?;
        }
        self.remove_timer(id);
        Ok(())
    }

    fn remove_timer(&mut self, id: OperationId) -> Option<Timer> {
        let timer = self.timers.remove(&id)?;
        self.deadlines.remove(&(timer.deadline, id));
        Some(timer)
    }

    fn next_timer(&self) -> Option<(OperationId, Instant)> {
        self.deadlines.first().map(|&(deadline, id)| (id, deadline))
    }
}

type HttpResult = (OperationId, Result<(u16, String), String>);
enum Completion {
    Http(HttpResult),
    Json((OperationId, Result<serde_json::Value, String>)),
    Timer(OperationId),
    TimerDenied(OperationId, String),
    Wait((OperationId, serde_json::Value)),
    PollWaits,
}

/// A ready worker/timer may win select after the elapsed deadline is also
/// ready. The clock is checked again at delivery, independently of select order.
/// Historical committed outcomes bypass this live admission check.
fn enforce_delivery_deadline(completion: Completion, expired: bool, recorded: bool) -> Completion {
    if !expired || recorded {
        return completion;
    }
    let error = "BudgetError: elapsed deadline exceeded".to_string();
    match completion {
        Completion::Http((id, _)) => Completion::Http((id, Err(error))),
        Completion::Json((id, _)) => Completion::Json((id, Err(error))),
        Completion::Timer(id) => Completion::TimerDenied(id, error),
        other => other,
    }
}

async fn next_completion(
    tasks: &mut JoinSet<Completion>,
    timer: Option<(OperationId, Instant)>,
    wake: Option<Duration>,
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
    let wait_ready = async {
        match wake {
            Some(delay) => tokio::time::sleep(delay).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        result = tasks.join_next(), if !tasks.is_empty() => {
            // The wrapper task only awaits a worker and constructs owned data.
            result.expect("active adapter task").expect("adapter wrapper task")
        }
        id = timer_ready => Completion::Timer(id),
        () = wait_ready => Completion::PollWaits,
    }
}

pub(crate) fn run(
    scope: &mut v8::PinScope<'_, '_>,
    runtime: &Runtime,
    mut failure: Option<String>,
    evaluation: Option<&v8::Global<v8::Promise>>,
) -> Result<(), String> {
    let mut tasks = JoinSet::new();
    let mut immediate = VecDeque::new();
    let mut active = BTreeMap::<OperationId, bool>::new();
    let mut deadline_timers = BTreeSet::new();
    let cancellation = OperationCancellation::default();
    let _cancellation_guard = CancellationGuard(cancellation.clone());
    loop {
        let mut module_failed = false;
        {
            v8::scope!(let scope, scope);
            v8::tc_scope!(let scope, scope);
            if let Some(error) = super::limits::failure(scope) {
                return Err(error);
            }
            super::microtasks::drain(scope);
            if let Some(error) = super::limits::failure(scope) {
                return Err(error);
            }
            if failure.is_none() {
                if let Some(evaluation) = evaluation {
                    let promise = v8::Local::new(scope, evaluation);
                    if promise.state() == v8::PromiseState::Rejected {
                        failure = Some(promise.result(scope).to_rust_string_lossy(scope));
                        module_failed = true;
                    }
                }
            }
            if let Some(error) = super::limits::failure(scope) {
                return Err(error);
            }
            if let Some(exception) = scope.exception() {
                let message = exception.to_rust_string_lossy(scope);
                failure.get_or_insert(message);
                scope.reset();
            }
        }
        if module_failed {
            // Rendering a user-thrown value can enqueue more microtasks.
            continue;
        }
        if let Some(session) = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow_mut()
            .as_mut()
        {
            session.flush()?;
        }
        // Release the isolate-slot borrow before any JavaScript can run.
        let (queued, timer) = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let mut ops = state.operations.borrow_mut();
            (std::mem::take(&mut ops.queued), ops.next_timer())
        };
        let historical = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow()
            .as_ref()
            .is_some_and(|session| session.has_history());
        let mut schedule_at: Option<i64> = None;
        // Once one category is full, requeue the rest without more SQLite
        // admissions in this pass. Owned task completion will wake the loop.
        let mut tool_blocked = false;
        let mut model_blocked = false;
        let expired = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.deadline_expired());
        if expired && !historical {
            cancellation.cancel();
            if !active.is_empty() {
                scope.set_slot(DetachedHostWork);
            }
            tasks.abort_all();
            tasks.detach_all();
            for (id, is_json) in std::mem::take(&mut active) {
                let error = "BudgetError: elapsed deadline exceeded".to_string();
                immediate.push_back(if is_json {
                    Completion::Json((id, Err(error)))
                } else {
                    Completion::Http((id, Err(error)))
                });
            }
            let ids: Vec<_> = scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .operations
                .borrow()
                .timers
                .keys()
                .copied()
                .collect();
            for id in ids {
                // Keep the timer visible until its own completion is delivered.
                // A preceding rejection's finally block may still cancel it.
                if deadline_timers.insert(id) {
                    immediate.push_back(Completion::TimerDenied(
                        id,
                        "BudgetError: elapsed deadline exceeded".into(),
                    ));
                }
            }
        }
        for (id, url) in queued {
            let cached = scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .session
                .borrow()
                .as_ref()
                .is_some_and(|session| session.is_cached(id));
            if cached {
                continue;
            }
            if historical {
                scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .queued
                    .push((id, url));
                continue;
            }
            let allow_redirects = {
                let state = scope.get_slot::<HostState>().expect("host state installed");
                state.require_network(&url)?;
                state.effective_capabilities.borrow().is_none()
            };
            let admission = if tool_blocked {
                super::resources::Admission::Blocked
            } else if let Some(session) = scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .session
                .borrow_mut()
                .as_mut()
            {
                session.admit_resource(id, "fetch.get", &serde_json::json!([url]))?
            } else {
                super::resources::Admission::Ready
            };
            match admission {
                super::resources::Admission::Ready => {}
                super::resources::Admission::Blocked => {
                    tool_blocked = true;
                    scope
                        .get_slot::<HostState>()
                        .expect("host state installed")
                        .operations
                        .borrow_mut()
                        .queued
                        .push((id, url));
                    continue;
                }
                super::resources::Admission::Deferred(at) => {
                    schedule_at = Some(schedule_at.map_or(at, |old| old.min(at)));
                    scope
                        .get_slot::<HostState>()
                        .expect("host state installed")
                        .operations
                        .borrow_mut()
                        .queued
                        .push((id, url));
                    continue;
                }
                super::resources::Admission::Denied(error) => {
                    immediate.push_back(Completion::Http((id, Err(error))));
                    continue;
                }
            }
            if let Some(session) = scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .session
                .borrow_mut()
                .as_mut()
            {
                session.begin_dispatch(id)?;
            }
            active.insert(id, false);
            let cancellation = cancellation.clone();
            let _entered = runtime.enter();
            tasks.spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    cancellation.check()?;
                    let result = networking::http_get(&url, allow_redirects);
                    cancellation.check()?;
                    result
                })
                .await
                .unwrap_or_else(|error| Err(format!("HTTP worker failed: {error}")));
                Completion::Http((id, result))
            });
        }
        let json_queued = {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            std::mem::take(&mut state.operations.borrow_mut().json_queued)
        };
        for (id, job) in json_queued {
            let state = scope.get_slot::<HostState>().expect("host state installed");
            let cached = state
                .session
                .borrow()
                .as_ref()
                .is_some_and(|s| s.is_cached(id));
            if cached {
                continue;
            }
            if historical {
                state.operations.borrow_mut().json_queued.push((id, job));
                continue;
            }
            if let Some(url) = &job.network_url {
                state.require_network(url)?;
            }
            if let Some(tool) = &job.required_tool {
                state.require_tool(tool)?;
            }
            let model_job = job.name == "model.call";
            let blocked = if model_job {
                model_blocked
            } else {
                tool_blocked
            };
            let admission = if blocked {
                super::resources::Admission::Blocked
            } else if let Some(session) = state.session.borrow_mut().as_mut() {
                session.admit_resource(id, &job.name, &job.args)?
            } else {
                super::resources::Admission::Ready
            };
            match admission {
                super::resources::Admission::Ready => {}
                super::resources::Admission::Blocked => {
                    if model_job {
                        model_blocked = true;
                    } else {
                        tool_blocked = true;
                    }
                    state.operations.borrow_mut().json_queued.push((id, job));
                    continue;
                }
                super::resources::Admission::Deferred(at) => {
                    schedule_at = Some(schedule_at.map_or(at, |old| old.min(at)));
                    state.operations.borrow_mut().json_queued.push((id, job));
                    continue;
                }
                super::resources::Admission::Denied(error) => {
                    immediate.push_back(Completion::Json((id, Err(error))));
                    continue;
                }
            }
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.begin_dispatch(id)?;
            }
            active.insert(id, true);
            let cancellation = cancellation.clone();
            let _entered = runtime.enter();
            tasks.spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    cancellation.check()?;
                    let result = (job.task)(cancellation.clone());
                    cancellation.check()?;
                    result
                })
                .await
                .unwrap_or_else(|_| Err("adapter worker failed".into()));
                Completion::Json((id, result))
            });
        }
        let cached = if let Some(session) = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow_mut()
            .as_mut()
        {
            session.next_cached()?
        } else {
            None
        };
        let cached = if let Some((id, result)) = cached {
            if scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .operations
                .borrow()
                .fetches
                .contains_key(&id)
            {
                Some(Completion::Http((
                    id,
                    serde_json::from_value(result).map_err(|e| e.to_string())?,
                )))
            } else if scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .operations
                .borrow()
                .json_resolvers
                .contains_key(&id)
            {
                Some(Completion::Json((
                    id,
                    serde_json::from_value(result).map_err(|e| e.to_string())?,
                )))
            } else {
                let is_wait = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow()
                    .waits
                    .contains_key(&id);
                if is_wait {
                    Some(Completion::Wait((id, result)))
                } else {
                    Some(
                        if let Some(error) = result.get("error").and_then(serde_json::Value::as_str)
                        {
                            Completion::TimerDenied(id, error.to_string())
                        } else {
                            Completion::Timer(id)
                        },
                    )
                }
            }
        } else {
            None
        };
        let cached = if cached.is_some() {
            cached
        } else {
            let ready = if let Some(session) = scope
                .get_slot::<HostState>()
                .expect("host state installed")
                .session
                .borrow_mut()
                .as_mut()
            {
                session.ready_wait()?
            } else {
                None
            };
            ready.map(Completion::Wait)
        };
        let cached = cached.or_else(|| immediate.pop_front());
        if tasks.is_empty() && timer.is_none() && cached.is_none() && schedule_at.is_none() {
            // Rejection stringification can itself run JS and enqueue work.
            let rejection = super::microtasks::take_unhandled_rejection(scope);
            if let Some(message) = rejection {
                failure.get_or_insert(message);
                // A user-defined rejection toString may schedule microtasks.
                // Checkpoint again before deciding that the runtime is idle.
                continue;
            }
            let state = scope.get_slot::<HostState>().expect("host state installed");
            if let Some(session) = state.session.borrow_mut().as_mut() {
                session.check_idle()?;
            }
            let ops = state.operations.borrow();
            if ops.queued.is_empty() && ops.json_queued.is_empty() && ops.timers.is_empty() {
                drop(ops);
                let parked = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow()
                    .as_ref()
                    .is_some_and(|session| session.should_park());
                if failure.is_none() && parked {
                    break;
                }
                if let Some(evaluation) = evaluation {
                    let promise = v8::Local::new(scope, evaluation);
                    if promise.state() == v8::PromiseState::Pending {
                        failure.get_or_insert_with(|| {
                            "module evaluation is pending but no host operations remain".into()
                        });
                    }
                }
                break;
            }
            continue;
        }
        let wake = scope
            .get_slot::<HostState>()
            .expect("host state installed")
            .session
            .borrow()
            .as_ref()
            .and_then(|session| session.next_wake())
            .into_iter()
            .chain(schedule_at)
            .min()
            .map(|at| {
                super::durability::epoch_millis()
                    .map(|now| Duration::from_millis(at.saturating_sub(now).max(0) as u64))
            })
            .transpose()?;
        let completion = match cached {
            Some(completion) => completion,
            None => {
                super::limits::pause(scope);
                if let Some(error) = super::limits::failure(scope) {
                    return Err(error);
                }
                let completion = runtime.block_on(next_completion(&mut tasks, timer, wake));
                super::limits::resume(scope);
                let expired = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow()
                    .as_ref()
                    .is_some_and(|session| session.deadline_expired());
                enforce_delivery_deadline(completion, expired, false)
            }
        };
        if matches!(completion, Completion::PollWaits) {
            continue;
        }
        v8::scope!(let scope, scope);
        v8::tc_scope!(let scope, scope);
        match completion {
            Completion::PollWaits => unreachable!(),
            Completion::Http((id, result)) => {
                active.remove(&id);
                // Commit a bounded error too: oversized responses must not leave
                // an ambiguous pending operation or reach JavaScript.
                let encoded = serde_json::to_value(&result).map_err(|e| e.to_string())?;
                let cached = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow()
                    .as_ref()
                    .is_some_and(|session| session.is_cached(id));
                let result = if cached {
                    result
                } else {
                    match validate_json(&encoded, MAX_OPERATION_RESULT_BYTES, "result") {
                        Ok(()) => result,
                        Err(error) => Err(error),
                    }
                };
                if let Some(session) = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow_mut()
                    .as_mut()
                {
                    session.complete(
                        id,
                        &serde_json::to_value(&result).map_err(|e| e.to_string())?,
                    )?;
                }
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
            Completion::Json((id, result)) => {
                active.remove(&id);
                let cached = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow()
                    .as_ref()
                    .is_some_and(|s| s.is_cached(id));
                let encoded = serde_json::to_value(&result).map_err(|e| e.to_string())?;
                let result = if cached {
                    result
                } else {
                    match validate_json(&encoded, MAX_OPERATION_RESULT_BYTES, "result") {
                        Ok(()) => result,
                        Err(error) => Err(error),
                    }
                };
                if let Some(session) = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow_mut()
                    .as_mut()
                {
                    session.complete(
                        id,
                        &serde_json::to_value(&result).map_err(|e| e.to_string())?,
                    )?;
                }
                let retained = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .json_resolvers
                    .remove(&id)
                    .ok_or("pending JSON operation missing")?;
                let resolver = v8::Local::new(scope, &retained);
                complete_json(scope, resolver, result)?;
            }
            Completion::Wait((id, result)) => {
                if let Some(session) = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow_mut()
                    .as_mut()
                {
                    session.complete(id, &result)?;
                }
                let retained = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .waits
                    .remove(&id)
                    .ok_or("pending wait missing")?;
                let resolver = v8::Local::new(scope, &retained);
                let decoded: Result<serde_json::Value, String> =
                    serde_json::from_value(result).map_err(|e| e.to_string())?;
                match decoded {
                    Ok(value) => {
                        let text = v8::String::new(scope, &value.to_string())
                            .ok_or("wait result exceeds V8 limit")?;
                        let value = v8::json::parse(scope, text).ok_or("invalid wait result")?;
                        resolver.resolve(scope, value);
                    }
                    Err(error) => {
                        let message =
                            v8::String::new(scope, &error).ok_or("wait error exceeds V8 limit")?;
                        let error = v8::Exception::error(scope, message);
                        resolver.reject(scope, error);
                    }
                }
            }
            Completion::TimerDenied(id, error) => {
                deadline_timers.remove(&id);
                if !scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow()
                    .timers
                    .contains_key(&id)
                {
                    // A previous completion's catch/finally canceled this timer
                    // and recorded its cancellation before this queued denial.
                    continue;
                }
                if let Some(session) = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow_mut()
                    .as_mut()
                {
                    session.complete(id, &serde_json::json!({"error":error}))?;
                }
                scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .remove_timer(id);
                failure.get_or_insert(error);
            }
            Completion::Timer(id) => {
                if let Some(session) = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .session
                    .borrow_mut()
                    .as_mut()
                {
                    session.complete(id, &serde_json::json!({"fired": true}))?;
                }
                let timer = scope
                    .get_slot::<HostState>()
                    .expect("host state installed")
                    .operations
                    .borrow_mut()
                    .remove_timer(id);
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
        if let Some(error) = super::limits::failure(scope) {
            return Err(error);
        }
        if let Some(exception) = scope.exception() {
            let message = exception.to_rust_string_lossy(scope);
            failure.get_or_insert(message);
            scope.reset();
        }
    }
    failure.map_or(Ok(()), Err)
}

fn complete_json(
    scope: &mut v8::PinScope<'_, '_>,
    resolver: v8::Local<v8::PromiseResolver>,
    result: Result<serde_json::Value, String>,
) -> Result<(), String> {
    match result {
        Ok(value) => {
            let text =
                v8::String::new(scope, &value.to_string()).ok_or("JSON result exceeds V8 limit")?;
            let value = v8::json::parse(scope, text).ok_or("invalid JSON result")?;
            resolver.resolve(scope, value);
        }
        Err(error) => {
            let message = v8::String::new(scope, &error).ok_or("JSON error exceeds V8 limit")?;
            let error = v8::Exception::error(scope, message);
            resolver.reject(scope, error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[test]
    fn late_worker_delivery_is_rejected_regardless_of_select_winner() {
        assert!(
            matches!(enforce_delivery_deadline(Completion::Json((1,Ok(serde_json::json!({"text":"late"})))),true,false),Completion::Json((1,Err(error))) if error.contains("elapsed deadline"))
        );
        assert!(
            matches!(enforce_delivery_deadline(Completion::Http((2,Ok((200,"late".into())))),true,false),Completion::Http((2,Err(error))) if error.contains("elapsed deadline"))
        );
        assert!(matches!(
            enforce_delivery_deadline(
                Completion::Json((1, Ok(serde_json::Value::Null))),
                false,
                false
            ),
            Completion::Json((1, Ok(_)))
        ));
    }
    #[test]
    fn late_timer_is_denied_but_committed_history_keeps_its_outcome() {
        assert!(
            matches!(enforce_delivery_deadline(Completion::Timer(3),true,false),Completion::TimerDenied(3,error) if error.contains("elapsed deadline"))
        );
        assert!(matches!(
            enforce_delivery_deadline(Completion::Timer(3), true, true),
            Completion::Timer(3)
        ));
        assert!(
            matches!(enforce_delivery_deadline(Completion::Json((1,Ok(serde_json::json!({"text":"recorded"})))),true,true),Completion::Json((1,Ok(value))) if value["text"]=="recorded")
        );
    }
}
