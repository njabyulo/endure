# Ondure

Run JavaScript in an embedded V8 isolate with policy-controlled network access.
Use an async IIFE for `await`. The host provides `console.log`, `console.error`,
and a minimal GET-only `fetch` with `ok`, `status`, and promise-returning `json()`.

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
6. `capabilities/policy.rs`: Ondure's permission decisions and YAML loading,
   independent of V8.

The host keeps V8 scopes visible. Local handles live within those scopes;
Global handles keep rejected promises and their reasons available for inspection.

Unit tests live beside their lesson and exercise the public runtime API. For
example, `cargo test -p runtime core::microtasks` demonstrates promise handling.
`cargo test -p runtime --test network` exercises the networking integration with
real local listeners.
