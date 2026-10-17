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

The CLI persists graph snapshots and hashes with journaled runs. In-memory
`run_module` remains available without journaling. Replay uses the captured graph and verifies its integrity; code identity does
not pin engine versions.
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
HTTP requests are not implemented. Recorded operations can be replayed without repeating completed effects.

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
`run_durable` function through the public exports in `crates/runtime/src/lib.rs`,
after preparing a `ModuleGraph`. `run_module` and `run_source` remain available
for execution without a journal.
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

## Durable operation journal

`ondure run` writes `.ondure/journal.sqlite3` by default. SQLite uses WAL and
`synchronous=FULL`. Fetch and timer intents commit before dispatch; their results
commit before promises settle or callbacks run. Cancellation and both success
and failure results are recorded with a completion order. Intent batches flush
at dispatch and at 256 entries or a 2 ms batching threshold during registration.
Results commit before individual JS delivery; no fsync is deferred past an await.

```sh
./build/ondure run examples/typescript/agent.ts --run-id example --journal .ondure/journal.sqlite3
./build/ondure log example
```

Run IDs are generated when omitted and printed on stderr before execution.
Duplicate IDs fail rather than replacing a run. The Rust `run_durable` API
provides the same boundary; `run_source` and `run_module` remain ephemeral
embedding APIs. Captured code and the run's network
policy are stored with the run. Console output is captured again at finalization;
it is not an external exactly-once effect.

A killed process can leave an intent without a result. Replay retries unfinished
GET and timer operations under their `safe` policy; completed results are reused. A committed result survives process death;
power-loss durability additionally depends on the filesystem and storage device
honoring SQLite's synchronization requests. The crash regression kills 100
processes after an external witness sees a JS await continuation and verifies
that every acknowledged result remains in the journal.

## Crash recovery and replay

```sh
./build/ondure replay example --journal .ondure/journal.sqlite3
```

Replay executes the captured module graph with its recorded network policy.
It validates operation names, argument digests, replay policies, and completion
order, supplying recorded results without opening sockets or repeating timer
waits. After the recorded prefix it may continue an unfinished run. Completed
runs reject unexpected extra operations. A changed graph passed through the Rust
`replay_run` verification API fails its code-hash check before execution.
Replaying a completed or failed run preserves its original status, output, and
operation history, including when replay diverges. An operating-system lease
allows only one process to execute a given run at a time and is released on
process exit, including a crash.

An intent without a result is ambiguous: a safe GET can be repeated. This is not
an exactly-once guarantee for outside effects; unsafe tools are not exposed by
this milestone. Divergence is fatal even if agent JavaScript catches the error.
Console output is reconstructed during replay. Time and random values exposed through the supported durable globals are
journaled as synchronous operations too.

## Recorded time and randomness

Durable runs replace `Date.now()`, no-argument `new Date()`, `Date()`,
`performance.now()`, `Math.random()`, and `crypto.randomUUID()` with journal-backed
bindings. Each value commits before returning to JavaScript; replay returns the
recorded value. Explicit Date arguments, `Date.parse`, `Date.UTC`, subclasses,
and `instanceof` retain their normal behavior. The original Date constructor is
kept in a closure; replacing `Reflect.construct` cannot expose it.

`WeakRef`, `FinalizationRegistry`, and `Temporal.Now` are removed from durable
runs. Intl date formatting requires an explicit non-undefined date; omitted dates
would otherwise read the current clock outside the journal. Ephemeral
`run_source`/`run_module` embedding APIs keep their original globals. This is not
engine/environment pinning: locale-sensitive formatting and explicit local-date
parsing still depend on the host timezone and locale. Keep the execution
environment stable and use UTC/explicit offsets for portable dates. Unprovided
APIs such as workers and external entropy sources remain unavailable.

Journal JSON uses round-trip float parsing so recorded random/performance values
return identical floating-point values rather than nearby approximations.
