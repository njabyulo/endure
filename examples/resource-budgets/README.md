# Resource admission, scheduling, and elapsed deadlines

Run the credential-free local model fixture:

```sh
python3 examples/resource-budgets/run.py
```

The first two responses report five invented tokens each. Before each dispatch,
the runtime reserves ten estimated input plus ten maximum output tokens. With a
25-token limit, the third 20-token reservation is rejected before contacting the
fixture. The example also configures model/tool concurrency, rolling rate limits,
elapsed deadlines, and prices, then verifies replay makes no further request.
Fixture usage and prices are test data, not measured provider billing.

Trusted `ondure.yaml` accepts:

```yaml
resources:
  token_limit: 10000
  dollar_limit_micros: 500000 # $0.50; one dollar = 1,000,000 micros
  model_input_token_estimate: 4096
  input_price_micros_per_million_tokens: 1000000 # $1 / million input tokens
  output_price_micros_per_million_tokens: 2000000
  tool_estimate_micros: 100
  model_concurrency: 2
  tool_concurrency: 4
  model_rate: {max_calls: 10, window_ms: 60000}
  tool_rate: {max_calls: 20, window_ms: 60000}
  elapsed_deadline_ms: 300000
```

Limits apply per run. Omitted financial/deadline/rate limits are unlimited.
Default concurrency is four model requests and sixteen tool requests. Prices
and tool estimates default to zero: configure them to account for spend.
Input estimates reserve configured tokens; requested output limits reserve
output tokens. These estimates may differ from provider usage.

Admission and reservation are a SQLite transaction. Deferred concurrency/rate
requests remain queued without dispatch. Completed model usage reconciles token
counts when total usage (or both component counts) is supplied. Dollar usage
requires both component counts and uses the configured prices, rounding each
component up to a micro. Unknown usage, transport failures, interrupted attempts,
and canceled requests retain their estimates. Restart releases active slots but
retains financial reservations and rate history. Retrying an interrupted request
makes a fresh reservation because the provider may charge again.

The elapsed clock starts when the run record is committed and includes parked
time. `ctx.sleep` and human approval waits can be resumed with `wake`; an expired
wait without a committed answer rejects with a recorded `BudgetError`, even if
its wake time was earlier than the deadline and the scheduler resumed late.
Previously committed answers and operation outcomes replay unchanged. New
dispatch after the deadline is
denied. In-flight adapter delivery is canceled at the deadline; late results
cannot replace the recorded error. Cancellation cannot undo provider effects or
charges. Ordinary timers fail the run on elapsed expiration. Checks occur at
scheduler boundaries; active JavaScript is separately bounded by execution limits.

Reservations, configured estimates, and reported usage do not provide an exact
provider billing cap. The runtime preserves history on exhaustion and does not
retry ambiguous effects automatically. These resources are separate from active
execution time and V8-managed heap limits. The integration tests cover parked
deadline recovery, late provider completion, concurrency/rate admission, and
replay; ledger tests cover competing reservations and restart accounting.
