# Ondure

Run JavaScript and TypeScript ES modules in an embedded V8 isolate with
policy-controlled network access. Top-level `await` is supported. The host provides `console.log`, `console.error`,
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

## Modules and TypeScript

```sh
./build/ondure run examples/typescript/agent.ts
```

This example imports `./greeting.ts` and awaits a timer without a build step.
SWC removes TypeScript syntax; V8 links and executes the resulting ES modules.
Types are not checked. `.js` entries also use ES module semantics, including
strict mode and module-local declarations.

Imports must start with `./` or `../` and name an explicit `.js` or `.ts` file.
They resolve relative to the importing module and must stay inside the entry
file's canonical parent directory, including after symlink resolution. Place
shared code beneath that directory. Shared dependencies execute once per run;
cycles and re-exports use V8's module linker.

`import type` declarations disappear without loading their targets. Inline type
bindings follow verbatim TypeScript semantics: `import { type T } from './x.ts'`
can retain `x.ts` for its side effects. No `.swcrc` or `tsconfig.json` is loaded.
CommonJS, package and remote imports, extension guessing, dynamic imports,
`.d.ts` execution, JSX, and decorators are unsupported. Dynamic imports in
prepared source fail preparation; expressions constructed through `eval` reject
at runtime without loading code.

Preparation errors identify the file and available source location. Runtime
stack locations refer to emitted JavaScript; source-map remapping is not provided.
If top-level await is still pending after all host work finishes, the run fails
with a pending-evaluation error instead of reporting success.

## Code identity for Rust callers

`ModuleGraph::load(&Path)` captures the reachable execution graph in memory.
`graph.code_hash()` returns its SHA-256 identity. The hash covers a versioned,
length-prefixed encoding of the entry ID, sorted relative module IDs, raw source,
emitted JavaScript, and resolved import edges. Absolute checkout paths are not
included. Type-only dependencies erased during transpilation are not loaded or
hashed; the importing source itself is hashed.

`run_module(&policy, &graph, Some(expected_hash))` verifies a previously recorded
hash before creating the isolate or executing code. Use `None` for a fresh run.
Both paths execute captured code without rereading files, even if the original
files change or disappear. Loading is not an atomic filesystem snapshot.

Graph hashes and source snapshots are not persisted by the CLI. Journaling and
replay will supply persistent run identity in later milestones; C3 provides the
snapshot and verification API. It does not implement replay or pin engine versions.
All compilation and hashing dependencies belong to the runtime crate.

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
const response = await fetch('http://127.0.0.1:8787/hit');
console.log(await response.json());
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

For Rust embedding, `run_source` and `run_module` are synchronous and block until idle.
`run_source` retains classic JavaScript script semantics and requires an async
IIFE for `await`. `run_module` executes a prepared ES module graph. Calls
from an entered Tokio runtime create and dispose the isolate on a separate
thread to avoid nested runtime panics. Use Tokio's `spawn_blocking` at async
call sites when the caller's executor must remain responsive.

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
`run_module` function through the public exports in `crates/runtime/src/lib.rs`,
after preparing a `ModuleGraph`. `run_source` remains available for script embedding.
Inside the runtime, each group covers a different lesson:

1. `core/embedding.rs`: V8 platform initialization, isolates, and host bindings.
   Context and handle-scope lifetimes remain visible in `host.rs`.
2. `core/execution.rs`: source compilation, filenames, execution, and exceptions.
   V8 owns JavaScript parsing and interpretation internally.
3. `core/transpilation.rs`: SWC parsing, TypeScript transformation, and import discovery.
4. `core/modules.rs`: immutable source graphs, hashing, and V8 module linking.
5. `core/microtasks.rs`: await continuations, persistent promise handles, and
   unhandled rejection tracking.
6. `integrations/io.rs`: JavaScript console callbacks and captured output.
7. `integrations/networking.rs`: fetch callbacks, permission enforcement, HTTP,
   and response promises.
8. `core/event_loop.rs`: operation registration, Rust completions, promise
   settlement, and idle detection. One Tokio current-thread runtime drives each
   isolate; HTTP workers exchange owned Rust data with it.
9. `integrations/timers.rs`: one-shot callbacks, arguments, and cancellation.
10. `capabilities/policy.rs`: Ondure's permission decisions and YAML loading,
   independent of V8.

The host keeps V8 scopes visible. Local handles live within those scopes;
Global handles keep rejected promises and their reasons available for inspection.

Unit tests live beside their lesson and exercise the public runtime API. For
example, `cargo test -p runtime core::microtasks` demonstrates promise handling.
`cargo test -p runtime --test network` exercises the networking integration with
real local listeners.
