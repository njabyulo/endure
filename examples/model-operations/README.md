# Journaled model operations

Run the complete example from the repository root, without credentials:

```sh
python3 examples/model-operations/run.py
```

The script starts a local Chat Completions fixture, runs `main.ts` with a
host configuration in a temporary directory, then replays the completed run.
It verifies that the provider received exactly one request. Fixture usage is
invented test data, not a billing measurement.

The supported interface is non-streaming text Chat Completions with `model`,
`messages`, `max_tokens`, and `stream: false`. Messages contain a
`system`, `user`, or `assistant` role and string `content`. Calls may supply:

```js
await ctx.model.call("Hello", { max_output_tokens: 32 });
await ctx.model.call([{ role: "user", content: "Hello" }]);
await ctx.model.call({ prompt: "Hello", max_output_tokens: 32 });
await ctx.model.call({ messages, max_output_tokens: 32 });
// => { text, usage: { prompt_tokens?, completion_tokens?, total_tokens? } | null,
//      finish_reason: string | null }
```

The host's `max_output_tokens` bounds each requested output limit. The adapter
accepts the first returned choice's text content and available unsigned usage
counts; unsupported or malformed responses become recorded errors. It does
not implement streaming, images, provider tool calling, or universal provider
compatibility. A provider must support this exact request/response subset.

For an optional live provider, place trusted `ondure.yaml` beside your working
command directory and configure your own endpoint and supported model:

```yaml
grants:
  network_origins: ["https://your-provider.example"]
credentials:
  provider: ONDURE_PROVIDER_KEY
model:
  endpoint: https://your-provider.example/v1/chat/completions
  model: your-model
  credential: provider
  max_output_tokens: 128
  timeout_seconds: 120 # optional; default 30, allowed 1..3600
  retry_interrupted: false
```

Set `ONDURE_PROVIDER_KEY` in the host environment and change the agent's
requested origin to the same exact origin. Keep credentials out of source and
configuration values. Logical credential references resolve inside the host
worker just before dispatch. Credential values and provider error bodies are
not written to operation records. Automatic redirects are disabled. Credentialed
endpoints require HTTPS; plaintext HTTP is allowed only for literal `localhost`
or loopback IP addresses used by local fixtures. An uncredentialed endpoint may
use HTTP. The total request timeout is trusted configuration (`timeout_seconds`,
default 30, range 1–3600). A timeout records an unknown-outcome error; it does not
prove that the provider stopped processing or avoided charging the request.

Completed outcomes commit before JavaScript receives them and replay without
contacting the provider. The default recovery policy refuses to reissue an
unfinished request with a recorded dispatch attempt. An intent that never
reached a recorded dispatch attempt may run after restart. Set
`retry_interrupted: true` only when accepting that an interrupted request may
be issued again and incur additional charges. Process interruption can happen
after a provider accepted a request but before its result was committed.
Cancellation prevents late delivery; it cannot promise provider cancellation
or undo charges. Token usage can be absent, and output limits are not billing
caps. These process recovery tests do not establish hardware power-loss
behavior.
