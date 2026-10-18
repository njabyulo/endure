# Durable agent lifecycle

From the repository root:

```sh
cargo run -- run examples/agent-lifecycle/main.ts --request onboarding-ada --input '{"name":"Ada"}'
cargo run -- wake
```

After the one-second persisted sleep becomes ready, `wake` proceeds to the
approval and prints its run ID and wait sequence. Answer using those values:

```sh
cargo run -- approve <run-id> <wait-seq> yes --note 'Reviewed locally'
cargo run -- replay <run-id>
```

`agent(name, handler)` and `agent(name, options, handler)` define a default
entrypoint invoked with a host context and persisted JSON input. Ordinary
scripts remain supported. Agent entrypoints require journaled execution.
`ctx.sleep` accepts integer milliseconds or strings ending in `ms`, `s`, `m`,
or `h`. It parks the run and releases the isolate. Registered host operations
survive interruption; arbitrary JavaScript promises and callbacks are not
checkpointed.

`ctx.human.approve(details)` returns `{ approved, note }`. Optional `timeout`
uses the same duration format; only `onTimeout: "reject"` is supported. The
first answer or timeout committed to SQLite wins. Timed waits are processed
when a run is resumed or `wake` runs. The CLI operator is trusted locally;
recipient labels do not authenticate a remote person or send notifications.
Completed answers replay from the journal without prompting again.
