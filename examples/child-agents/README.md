# Durable local child agents

From this directory, run:

```sh
cargo run --manifest-path ../../Cargo.toml -- run main.ts --request child-demo --input '{"name":"Ada"}'
cargo run --manifest-path ../../Cargo.toml -- replay <parent-run-id>
```

No provider or credentials are required. The parent requests the `reviewer`
child capability. The trusted configuration grants it and explicitly delegates
an empty capability set plus a token/dollar allocation to the child. A child
can only request capabilities shared by the parent's effective authority and
its configured delegation. Agent code cannot increase these grants.

`ctx.agents.run(name, input)` resolves to the child handler's JSON return
value (`undefined` becomes `null`). New top-level runs ignore handler return
values; child return values are persisted for their waiting parent. Previously
recorded return-capture markers remain honored during replay. Returns are bounded JSON; arbitrary V8
objects, closures, and JavaScript promises are not persisted. The runtime
captures local child modules before the parent's first execution. Module
paths must remain under the parent's entry directory. Captured modules are
reused after restart even when source files are removed.

The recorded parent operation sequence defines a stable child request ID.
Replaying or recovering a parent rejoins that request and reuses completed
child results. It reuses the same parent allocation and does not create another completed child run. This does not
turn an interrupted unsafe provider/tool operation inside the child into an
exactly-once effect: each adapter retains its own recovery policy.

If a child sleeps or awaits approval, the parent also parks without retaining
an isolate or worker. Inspect the child run's wait and answer it with the
trusted local approval command, then run `wake` to continue the parent. A
wake batch may need another invocation when it wakes a child after examining
its parent. A failed child rejects the parent's await. If another scheduler already owns a
live child, the parent parks on that same child instead of treating lease
contention as a permanent failure.

Child allocations reserve parent resources before launch. Children receive
finite token and dollar limits and the parent's absolute elapsed deadline.
Reported child reservations are conservative when provider usage is absent.
A child that parks keeps its full parent allocation charged, even if later
completion uses less. This bounded first version limits nesting to eight levels.
The initial child invocation uses 1000 ms of active execution and 128 MiB of
V8 managed heap. Resumed invocations use the trusted replay, wake, or approval
caller's execution and heap limits. These heap limits do not bound all
host/process allocations.

Cancellation does not roll back a child or an already dispatched provider request.
A child may finish its bounded work before cooperative cancellation is observed.
