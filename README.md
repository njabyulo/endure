# Ondure

Run JavaScript in an embedded V8 isolate with policy-controlled network access.
Use an async IIFE for `await`. The host provides `console.log`, `console.error`,
a minimal GET-only `fetch` with `ok`, `status`, and promise-returning `json()`,
and one-shot `setTimeout` / `clearTimeout` timers.

## Build and run

```sh
cargo build --release --bin ondure
mkdir -p build
cp target/release/ondure build/ondure
./build/ondure run examples/agent.js
```

The first build needs internet for dependencies and the prebuilt V8 archive.

## Permissions

`ondure` reads `./ondure.yaml` from the current working directory:

```yaml
permissions: []
```

Empty permissions or a missing file denies network with `permission denied: net`.
Use `permissions: [net]` to allow network. Unknown permission names fail to load.
Allowed requests need a reachable server at the script's destination. A caught
denial exits successfully. Use `examples/agent.js` for your own experiments;
integration tests provide their own scripts and local servers.

## Async execution

`fetch` returns a pending promise immediately. HTTP runs on Tokio's blocking
worker pool with the existing 10-second request timeout; JavaScript and all V8
handles stay on the host thread. Multiple requests can run concurrently.

```js
(async () => {
  const response = await fetch('http://127.0.0.1:8787/hit');
  console.log(await response.json());
})();
```

`setTimeout(callback, delayMs, ...args)` returns a numeric ID for `clearTimeout`.
Callbacks run once, after synchronous JavaScript and its microtasks. Missing,
NaN, or negative delays become zero; delays are capped at 2,147,483,647 ms.
String callbacks are unsupported. Run the timer example without network access:

```sh
./build/ondure run examples/timers.js
```

The event loop waits for submitted HTTP operations and uncanceled timers,
including work submitted by their continuations. An unresolved JavaScript promise
alone does not keep the runtime alive. Microtasks drain between completions.

If a script or timer callback throws, submitted work still completes before the
first uncaught error is reported. Unhandled promise rejections are checked at
idle. Output from the entire run is retained on failure. Infinite JavaScript or
endlessly scheduled work can still prevent exit; CPU limits and cancellation of
HTTP requests are not implemented. Journaling and replay are also not implemented.

## Check

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Rust integration tests supply local HTTP listeners to verify zero denied
connections, computed-property fetch denial, and successful JSON responses.

## Reading the code

Start with `crates/runtime/src/host.rs` for execution flow. The CLI calls its
`run_source` function through the public exports in `crates/runtime/src/lib.rs`.
Inside the runtime, each group covers a different lesson:

1. `core/embedding.rs`: V8 platform initialization, isolates, and host bindings.
   Context and handle-scope lifetimes remain visible in `host.rs`.
2. `core/execution.rs`: source compilation, filenames, execution, and exceptions.
   V8 owns JavaScript parsing and interpretation internally.
3. `core/microtasks.rs`: await continuations, persistent promise handles, and
   unhandled rejection tracking.
4. `integrations/io.rs`: JavaScript console callbacks and captured output.
5. `integrations/networking.rs`: fetch callbacks, permission enforcement, HTTP,
   and response promises.
6. `core/event_loop.rs`: operation registration, Rust completions, promise
   settlement, and idle detection. One Tokio current-thread runtime drives each
   isolate; HTTP workers exchange owned Rust data with it.
7. `integrations/timers.rs`: one-shot callbacks, arguments, and cancellation.
8. `capabilities/policy.rs`: Ondure's permission decisions and YAML loading,
   independent of V8.

The host keeps V8 scopes visible. Local handles live within those scopes;
Global handles keep rejected promises and their reasons available for inspection.

Unit tests live beside their lesson and exercise the public runtime API. For
example, `cargo test -p runtime core::microtasks` demonstrates promise handling.
`cargo test -p runtime --test network` exercises the networking integration with
real local listeners.
