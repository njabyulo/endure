# Agent runtime verification contract

The feature examples exercise registered host operations. They do not checkpoint
arbitrary JavaScript promises, provide an OS sandbox, cap actual provider billing,
or make remote effects exactly once. Test fixtures establish local runtime
behavior; process-kill tests do not establish hardware power-loss guarantees.

The integrated [onboarding fixture](../examples/onboarding/README.md) is also
executed by the CLI `agent_feature_proof` test. It prepares a model response,
parks for an approval, commits an operator answer, interrupts the real CLI after
a mock tool effect but before its reply commits, and resumes from captured
modules. The safe mock receiver deduplicates by explicit application key.
Continuation runs a local child and updates scoped state, then replays without
repeating committed work. Assertions distinguish request count, durable attempt
count, and deduplicated mock effects.

Review these boundaries independently:

| Boundary | Evidence in the supported suite |
| --- | --- |
| Authority | `capability_grants` tests exact origins, redirects, default denial, and request identity; host tests prove requests cannot elevate grants; MCP and child tests check delegated tool/name/origin authority before dispatch. |
| Duplicate effects | MCP process-kill test refuses unsafe unknown outcomes; explicitly safe recovery records another attempt; onboarding retries only a keyed idempotent mock and checks one effect. |
| Resource accounting | Resource-budget tests cover atomic reservations, unknown usage, restart accounting, concurrency, rates, and elapsed deadlines; child allocations reserve parent resources. Configured price accounting is separate from provider invoices. |
| Replay integrity | Committed model/tool outcomes and memory retrieval are reused; captured modules/config survive source removal; low-level replay/divergence, clock/randomness, cancellation, journal migrations, and crash tests remain. |
| Credentials | Shared recursive detection checks response string values, object keys, and canonical JSON number/boolean/null text before model/MCP outcomes commit. This exact reflection check does not decode arbitrary obfuscations. Subprocess fixtures send real Authorization headers and verify credentials never appear in recorded arguments, results, output, or JavaScript responses. Raw remote error bodies are not propagated. |
| Approval | Agent lifecycle tests cover parking, notes, timeouts, first-answer behavior, and continuation; the integrated workflow reuses the committed approval after process interruption. |

Agent contexts use `ctx.sleep`, `ctx.human`, `ctx.context`, `ctx.memory`,
`ctx.state`, `ctx.model`, `ctx.tools`, and `ctx.agents`. New graphs importing the
captured `ondure` agent module do not receive the obsolete `globalThis.ondure`
namespace. Ordinary scripts and their captured journals retain the compatibility
namespace so existing parked approvals and timers can still replay. The legacy
parking/limits/checkpoint regression tests intentionally cover that compatibility
path. Public examples use the agent context or useful low-level script APIs.

The old scratch agent and global-approval examples are superseded by the feature
examples. Low-level timer and TypeScript module examples remain because they
verify supported ordinary-script behavior. Tests, local validation, remote CI,
PR opening, merge, release, and production use are separate delivery stages;
this document does not assert a deployment or performance result.

The integrated onboarding and standard-interface fixtures explicitly use a
10-second active execution allowance per CLI invocation so slow CI storage does
not turn workflow correctness checks into timing benchmarks. The CLI default
and dedicated execution-limit regression assertions remain unchanged.

Captured agent API compatibility is versioned separately from return capture.
Legacy snapshots without an API-version marker retain `globalThis.ondure`,
including children first created during parent recovery; new agent runs omit
that global. Regressions cover parked approval, completed replay, inherited
child compatibility, and rejection of unsupported snapshot API versions.
