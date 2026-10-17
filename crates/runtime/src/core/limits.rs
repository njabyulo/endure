//! Cooperative V8 termination for execution budgets and managed-heap pressure.
//!
//! This is not a process-memory sandbox: external allocations, host buffers,
//! and fatal native allocation failures require process isolation.

use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Optional per-run limits. Zero disables the corresponding limit.
#[derive(Clone, Copy, Debug, Default)]
pub struct RunLimits {
    /// Accumulated active execution time in milliseconds, excluding async waits.
    /// This measures elapsed active time rather than OS thread CPU time.
    pub cpu_ms: u64,
    /// V8 managed-heap limit in MiB; external and Rust allocations are excluded.
    pub heap_mb: usize,
}

struct Clock {
    remaining: Duration,
    active_since: Option<Instant>,
    stopped: bool,
}

struct Control {
    clock: Mutex<Clock>,
    changed: Condvar,
    reason: AtomicU8,
    handle: v8::IsolateHandle,
}

pub(crate) struct Guard {
    control: Arc<Control>,
    worker: Option<JoinHandle<()>>,
    heap: bool,
}

impl Guard {
    pub(crate) fn install(isolate: &mut v8::Isolate, limits: RunLimits) -> Self {
        let control = Arc::new(Control {
            clock: Mutex::new(Clock {
                remaining: Duration::from_millis(limits.cpu_ms),
                active_since: (limits.cpu_ms != 0).then(Instant::now),
                stopped: false,
            }),
            changed: Condvar::new(),
            reason: AtomicU8::new(0),
            handle: isolate.thread_safe_handle(),
        });
        isolate.set_slot(control.clone());
        if limits.heap_mb != 0 {
            isolate.add_near_heap_limit_callback(heap_limit, Arc::as_ptr(&control) as *mut c_void);
        }
        let worker = (limits.cpu_ms != 0).then(|| {
            let control = control.clone();
            std::thread::spawn(move || {
                let mut clock = control.clock.lock().unwrap();
                while !clock.stopped {
                    if let Some(started) = clock.active_since {
                        let left = clock.remaining.saturating_sub(started.elapsed());
                        if left.is_zero() {
                            control
                                .reason
                                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                                .ok();
                            control.handle.terminate_execution();
                            break;
                        }
                        clock = control.changed.wait_timeout(clock, left).unwrap().0;
                    } else {
                        clock = control.changed.wait(clock).unwrap();
                    }
                }
            })
        });
        Self {
            control,
            worker,
            heap: limits.heap_mb != 0,
        }
    }

    pub(crate) fn finish(mut self, isolate: &mut v8::Isolate) -> Option<String> {
        pause(isolate);
        {
            let mut clock = self.control.clock.lock().unwrap();
            clock.stopped = true;
            self.control.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            worker.join().expect("watchdog thread");
        }
        if self.heap {
            isolate.remove_near_heap_limit_callback(heap_limit, 0);
        }
        let failure = failure(isolate);
        isolate.remove_slot::<Arc<Control>>();
        failure
    }
}

// V8 asks for extra room before fatal OOM. Terminate JavaScript and provide
// emergency headroom for the interrupted allocation and termination unwinding.
unsafe extern "C" fn heap_limit(data: *mut c_void, current: usize, _: usize) -> usize {
    // SAFETY: Guard retains the Arc until the callback has been removed.
    let control = unsafe { &*(data as *const Control) };
    control.reason.store(2, Ordering::SeqCst);
    control.handle.terminate_execution();
    current.saturating_add(64 * 1024 * 1024)
}

pub(crate) fn pause(isolate: &v8::Isolate) {
    if let Some(control) = isolate.get_slot::<Arc<Control>>() {
        let mut clock = control.clock.lock().unwrap();
        if let Some(started) = clock.active_since.take() {
            clock.remaining = clock.remaining.saturating_sub(started.elapsed());
            if clock.remaining.is_zero() {
                control
                    .reason
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .ok();
                control.handle.terminate_execution();
            }
        }
        control.changed.notify_all();
    }
}

pub(crate) fn resume(isolate: &v8::Isolate) {
    if let Some(control) = isolate.get_slot::<Arc<Control>>() {
        let mut clock = control.clock.lock().unwrap();
        if !clock.remaining.is_zero() && clock.active_since.is_none() {
            clock.active_since = Some(Instant::now());
        }
        control.changed.notify_all();
    }
}

pub(crate) fn failure(isolate: &v8::Isolate) -> Option<String> {
    match isolate
        .get_slot::<Arc<Control>>()?
        .reason
        .load(Ordering::SeqCst)
    {
        1 => Some("execution time limit exceeded".into()),
        2 => Some("V8 heap limit exceeded".into()),
        3 => Some("console output limit exceeded".into()),
        _ => None,
    }
}

/// Console retention is bounded even when execution/heap budgets are disabled.
pub(crate) fn exceed_output(isolate: &v8::Isolate) {
    if let Some(control) = isolate.get_slot::<Arc<Control>>() {
        control
            .reason
            .compare_exchange(0, 3, Ordering::SeqCst, Ordering::SeqCst)
            .ok();
        control.handle.terminate_execution();
    }
}
