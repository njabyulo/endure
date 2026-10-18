# Ondure

Ondure is a Rust host for durable JavaScript and TypeScript agent workflows.
V8 executes captured local modules; SQLite records registered host operations,
waits and outcomes. Recovery reconstructs execution in a fresh isolate and reuses
committed outcomes before continuing supported unfinished work.

This feature-series branch contains bounded first versions of seven runtime
features. It is a source build for review; no package distribution, production
readiness or measured performance is claimed.

## Build and try

```sh
cargo build --bin ondure
python3 examples/model-operations/run.py
python3 examples/standard-interfaces/run.py
python3 examples/onboarding/run.py
```

Rust and Python 3 are required for these local fixtures. The first build downloads
dependencies and the V8 archive. Examples use local receivers and temporary
journals; no paid credentials are required. Optional live-model configuration is
in [the model example](examples/model-operations/README.md).

## Agent entrypoint

```js
import { agent } from "ondure";

export default agent("review", async (ctx, input) => {
  await ctx.context.add({ role: "user", content: input.message });
  const decision = await ctx.human.approve({
    action: "Continue onboarding?",
    recipient: "trusted local operator",
    timeout: "1h",
    onTimeout: "reject",
  });
  return { approved: decision.approved, note: decision.note };
});
```

Both `agent(name, handler)` and `agent(name, options, handler)` are supported.
The host invokes the default agent after module evaluation with native context
and persisted JSON input. Use `--request` to bind input to request identity:

```sh
cargo run -p ondure -- run examples/agent-lifecycle/main.ts --request review-demo --input '{"name":"Ada"}'
cargo run -p ondure -- wake
# Inspect the printed run ID and use its approval wait sequence:
cargo run -p ondure -- log RUN_ID
cargo run -p ondure -- approve RUN_ID WAIT_SEQ yes --note 'Reviewed locally'
```

`wake` is a local scheduler tick, not a background daemon. Invoke it from an
external scheduler to wake ready waits and recover abandoned runs. Recipient
labels do not provide remote authentication or delivery. The first committed
approval answer or rejection timeout wins.

## Seven features

| Feature | Supported boundary | Runnable example |
| --- | --- | --- |
| Durable execution | Intent before dispatch, outcome before delivery, captured code and completed-result reuse | [Operations](examples/operations.md), [onboarding interruption](examples/onboarding/README.md) |
| Capabilities | Requested authority intersects operator grants; exact origins, tool names, memory scopes and child names | [Allowed and denied dispatch](examples/capabilities/README.md), [MCP denial](examples/mcp/README.md) |
| Memory | Awaited context, SQLite full-text remember/recall, versioned shared get/set/compareAndSet | [Context, retrieval and CAS](examples/agent-memory/README.md) |
| Budgets and scheduling | Durable token/spend reservations, model/tool concurrency and rate admission, elapsed deadlines | [Resource budgets](examples/resource-budgets/README.md) |
| Replay and observability | Structured operations/attempts, recorded inputs/outcomes, inspect and compare | [Inspection and diff](examples/inspection.md) |
| Human approval | Persisted waits, notes and rejection timeouts across restarts | [Lifecycle](examples/agent-lifecycle/README.md) |
| Standard interfaces | Configured MCP Streamable HTTP tools and captured local children with delegated resources | [Tools and children](examples/standard-interfaces/README.md), [child agents](examples/child-agents/README.md) |

Agent APIs include `ctx.sleep(duration)`, `ctx.human.approve(details)`,
`ctx.model.call(promptOrMessagesOrOptions)`, `ctx.context.add(message)`,
`ctx.context.messages()`, `ctx.memory.remember/recall`, `ctx.state.get/set/compareAndSet`,
`ctx.tools.call(name, arguments)` and `ctx.agents.run(name, input)`.

Durations accept integer milliseconds or strings such as `"1s"`, `"15m"`, `"1h"`
and `"2d"`. Shared memory/state scopes require explicit requests and grants.
Context belongs to the run; defaults are 128 messages and 256 KiB, preserving
system and recent messages. Compaction archives removed content and records its
decisions. Oversized protected context is rejected. Retrieval is lexical FTS5,
not embeddings. State mutations and their outcomes commit atomically.

## Trusted host configuration

The CLI loads `ondure.yaml` from its working directory. Missing authority is denied.

```yaml
grants:
  network_origins: ["http://127.0.0.1:8787"]
  memory_scopes: ["demo"]
credentials:
  provider: ONDURE_PROVIDER_KEY
model:
  endpoint: http://127.0.0.1:8787/v1/chat/completions
  model: fixture
  max_output_tokens: 32
  retry_interrupted: false
resources:
  token_limit: 10000
  dollar_limit_micros: 2000000 # USD 2 using configured prices
  model_concurrency: 2
  tool_concurrency: 4
  model_rate: { max_calls: 10, window_ms: 60000 }
  elapsed_deadline_ms: 900000
```

The agent requests capabilities through `options.capabilities` using the same
capability field names. Requests never elevate operator grants. Agent module
initialization starts denied; host checks apply at dispatch. Network origins are
exact canonical scheme/host/port origins; scoped adapters do not automatically
follow redirects. These controls are not IP filtering, DNS rebinding protection
or an OS sandbox.

Credential configuration stores environment-variable names, not values. Model
and MCP adapters resolve secrets inside the host at dispatch and reject reflected
credential responses before delivery. Application prompts, results and archived
context can contain sensitive data; control journal and inspection access.

The complete trusted configuration and request input participate in request
identity. Matching retries reuse the same run; changed code, input or config
conflicts. Runs use captured grants when resumed. Editing today's YAML does not
revoke a persisted run. Local journal and run/lease access are trusted operator
capabilities. This is not a distributed executor.

See [capability details](docs/capabilities.md), [MCP protocol profile](docs/mcp.md)
and the resource/child example documentation for configuration and limits.

## Durability, retry and accounting

Every registered operation has `(run ID, sequence)` identity, validated JSON,
explicit recovery policy and a recorded outcome. External dispatch attempts are
committed before invoking adapters. New encoded arguments are bounded to 256 KiB
and outcomes to 1 MiB; migrated completed history remains replayable. Additive,
versioned journal migrations preserve existing journals.

Committed results replay without another provider/tool request. A new intent
with no recorded dispatch attempt can resume. After an uncertain dispatch,
unsafe operations stop for reconciliation; only explicitly retryable operations
may repeat. Legacy ambiguous records remain conservative. A crash after a receiver
accepted a request but before local outcome commit remains uncertain. The
onboarding mock uses receiver idempotency keys; it establishes no general
exactly-once effect guarantee.

Model support is one non-streaming text Chat Completions schema, not universal
provider compatibility. Retried interrupted requests may incur additional charges.
Resource admission reserves configured token/spend estimates before dispatch,
reconciles available usage using configured prices, and retains reservations when
usage is unknown. Dollar micros are millionths of USD. These are conservative
runtime accounts, not an exact provider billing cap. Exhaustion records an error
and preserves history. Elapsed deadlines include parked time.

Cancellation is cooperative: it prevents late delivery and further dispatch in
the canceled execution. It cannot undo external effects or promise provider-side
cancellation. Child allocations are reserved from the parent, and child authority
and deadlines are attenuated. Completed child identity is reused on restart.

Durability covers registered host operations, not every JavaScript promise,
callback, or heap object. There is no unrestricted `ctx.step(callback)` checkpoint.
Process-kill tests do not establish hardware power-loss durability. SQLite WAL
with synchronous commits still relies on storage honoring synchronization requests.

## Inspect and replay

```sh
cargo run -p ondure -- log RUN_ID
cargo run -p ondure -- explain RUN_ID
cargo run -p ondure -- diff RUN_A RUN_B
cargo run -p ondure -- replay RUN_ID
```

The default journal is `.ondure/journal.sqlite3`; use `--journal PATH` on commands
to select another. `run` prints generated run IDs before execution; explicit
`--run-id` values cannot replace an existing run. Request identity supplies retry
deduplication rather than run replacement.

Inspection shows recorded inputs, permissions, operation outcomes/attempts,
usage/reservations and supplied rationale. It does not expose hidden model
reasoning or invoke adapters. `diff` compares recorded registrations and metadata;
it does not fork execution. Replay verifies captured code, operation arguments,
recovery policy and delivery order. Completed histories remain immutable even
if replay diverges. Supported unfinished runs may continue live.

## Scripts and embedding

Ordinary ES module scripts remain available with top-level await, console,
GET-only fetch, one-shot timers and recorded clocks/randomness in durable runs.
Legacy `permissions: [net]` retains broad network access for ordinary scripts;
scoped agent requests do not inherit it. Legacy durable-script globals remain
only for compatibility and old journal replay; new agent examples use `ctx`.
New runs record the agent API version. Captured agents without that marker keep
their historical globals when parked, resumed, or replayed; newly created
children inherit the captured parent's compatibility version.

Relative imports must name `.js`/`.ts` files and stay inside the entry directory
through canonical paths. The captured `"ondure"` built-in is the only package-like
import. SWC removes TypeScript syntax without type checking. CommonJS, remote/npm
resolution, dynamic imports, JSX and decorators are unsupported. Captured code
identity does not pin V8 versions, timezone or locale.

Rust callers can use `ModuleGraph::load`, `run_durable_with_config` and
`run_request_with_config` with `HostConfig`/`RunLimits`. Ephemeral `run_source` and
`run_module` and existing `Policy` APIs remain available. They block their caller;
async hosts should use `spawn_blocking` where appropriate.

The CLI supervises execution in a worker process, defaulting to a 1,000 ms active
elapsed-execution budget and a 128 MiB V8-managed heap limit. Override with
`--cpu-ms` and `--heap-mb` (zero disables that limit). These differ from durable
resource budgets: waiting does not consume active execution time, each resume
receives a fresh active allowance, and the heap limit is not total process RSS.
Console retention is capped at a combined 8 MiB UTF-8 per execution. Native
abort containment is a CLI process property, not a promise that all logs flush
on native crash or that the Rust library survives fatal allocator failures.

## Verify and extend

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
```

Linux/macOS CI runs the supported suite. Local and remote results are reported
separately. [Proof fixtures](docs/feature-proof.md) distinguish process-crash,
receiver-idempotency and provider-mock evidence.

Deferred work includes streaming, embeddings, model-generated compaction,
execution forks/model overrides, local inference, stdio MCP, filesystem writes,
real payment/email adapters, OS sandboxing and package/skill distribution.
