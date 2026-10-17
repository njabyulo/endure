use runtime::{run_source, Policy};

#[test]
fn timer_order_arguments_and_nested_microtasks() {
    let output = run_source(
        &Policy::default(),
        "timers.js",
        r#"
        setTimeout((a, b) => {
            console.log(a, b);
            Promise.resolve().then(() => console.log('microtask'));
            setTimeout(() => console.log('nested'), 0);
        }, 0, 'timer', 42);
        Promise.resolve().then(() => console.log('promise'));
        console.log('sync');
    "#,
    )
    .unwrap();
    assert_eq!(
        output.stdout,
        "sync\npromise\ntimer 42\nmicrotask\nnested\n"
    );
}

#[test]
fn one_thousand_pending_operations_resolve() {
    let output = run_source(&Policy::default(), "many.js", r#"
        const pending = Array.from({length: 1000}, (_, i) =>
            new Promise(resolve => setTimeout(() => resolve(i), 5)));
        Promise.all(pending).then(values => console.log(values.length, values.reduce((a, b) => a + b, 0)));
    "#).unwrap();
    assert_eq!(output.stdout, "1000 499500\n");
}

#[test]
fn canceled_and_unresolved_promises_do_not_keep_runtime_alive() {
    let output = run_source(
        &Policy::default(),
        "cancel.js",
        r#"
        const id = setTimeout(() => console.log('wrong'), 2147483647);
        console.log(typeof id);
        clearTimeout(id);
        clearTimeout(id);
        clearTimeout(0);
        clearTimeout(-1);
        clearTimeout(1.5);
        new Promise(() => {});
        console.log('done');
    "#,
    )
    .unwrap();
    assert_eq!(output.stdout, "number\ndone\n");
}

#[test]
fn timer_can_cancel_another_due_timer() {
    let output = run_source(
        &Policy::default(),
        "cancel-due.js",
        r#"
        let id;
        setTimeout(() => { clearTimeout(id); console.log('first'); });
        id = setTimeout(() => console.log('wrong'));
    "#,
    )
    .unwrap();
    assert_eq!(output.stdout, "first\n");
}

#[test]
fn invalid_delays_default_to_zero_and_callback_must_be_a_function() {
    let output = run_source(&Policy::default(), "invalid.js", r#"
        for (const delay of [undefined, NaN, -5, 'invalid']) setTimeout(() => console.log('timer'), delay);
        try { setTimeout('console.log(1)', 0); } catch (e) { console.error(e.name); }
        try { setTimeout(() => {}, {valueOf() {throw Error('coercion');}}); } catch(e) {console.error(e.message);}
    "#).unwrap();
    assert_eq!(output.stdout, "timer\ntimer\ntimer\ntimer\n");
    assert_eq!(output.stderr, "TypeError\ncoercion\n");
}

#[test]
fn failures_still_drain_operations_and_retain_first_error() {
    for failure in [
        "throw Error('first');",
        "setTimeout(() => { throw Error('first'); }, 0);",
    ] {
        let source = format!(
            r#"
            {setup}
            setTimeout(() => {{
                console.log('finished');
                Promise.resolve().then(() => setTimeout(() => console.error('nested'), 0));
            }}, 5);
            {failure}
        "#,
            setup = "console.log('starting');"
        );
        let error = run_source(&Policy::default(), "failure.js", &source).unwrap_err();
        assert_eq!(error.output.stdout, "starting\nfinished\n");
        assert_eq!(error.output.stderr, "nested\n");
        assert_eq!(error.message, "Error: first");
    }
}

#[test]
fn rejection_handled_by_timer_is_inspected_only_at_idle() {
    let output = run_source(
        &Policy::default(),
        "late-handler.js",
        r#"
        const promise = Promise.reject(Error('handled'));
        setTimeout(() => promise.catch(e => console.log(e.message)), 5);
    "#,
    )
    .unwrap();
    assert_eq!(output.stdout, "handled\n");
    let error = run_source(
        &Policy::default(),
        "rejected-timer.js",
        r#"
        setTimeout(async () => { await Promise.resolve(); throw Error('timer rejection'); }, 0);
        setTimeout(() => console.log('finished'), 5);
    "#,
    )
    .unwrap_err();
    assert!(error
        .message
        .contains("unhandled promise rejection: Error: timer rejection"));
    assert_eq!(error.output.stdout, "finished\n");
}

#[test]
fn microtask_between_due_timers_can_cancel_the_next_callback() {
    let output = run_source(
        &Policy::default(),
        "checkpoint.js",
        r#"
        let second;
        setTimeout(() => {
            console.log('first');
            Promise.resolve().then(() => { clearTimeout(second); console.log('microtask'); });
        });
        second = setTimeout(() => console.log('wrong'));
    "#,
    )
    .unwrap();
    assert_eq!(output.stdout, "first\nmicrotask\n");
}

#[test]
fn rejection_stringification_cannot_hide_new_microtasks() {
    let error = run_source(
        &Policy::default(),
        "rejection-side-effect.js",
        r#"
        Promise.reject({toString() {
            Promise.resolve().then(() => setTimeout(() => console.log('finished'), 0));
            return 'reason';
        }});
    "#,
    )
    .unwrap_err();
    assert_eq!(error.message, "unhandled promise rejection: reason");
    assert_eq!(error.output.stdout, "finished\n");
}
