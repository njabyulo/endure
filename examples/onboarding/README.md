# Integrated onboarding and process-crash recovery

From the repository root:

```sh
python3 examples/onboarding/run.py
```

The runner starts local Chat Completions and MCP fixtures, creates a temporary
project, and invokes the real CLI. No paid provider or credentials are required.
It records a model response, parks for human approval, supplies the trusted local
operator's answer, and kills the approving process after the mock receiver has
committed its effect but before the runtime receives its reply. It verifies the
worker holds the run lease, waits for that OS lease to be released after the
kill, and only then releases the withheld reply. It then removes
the source/config files and resumes from captured modules.

Recovery reuses the completed model response and human answer, retries the
explicitly safe mock tool with the same application key, invokes a child with
empty authority, and updates scoped state. A second replay matches the final
output without external dispatch. Assertions check one model request, two tool
attempts, one deduplicated mock effect, and recorded interrupted/completed
attempts. The mock receiver's deduplication is specific to this fixture; it is
not an exactly-once guarantee for arbitrary email, payments, or tool effects.
This test demonstrates process interruption, not hardware power-loss safety.

Set `ONDURE_BIN=/absolute/path/to/ondure` to use a built executable. Otherwise
the runner prebuilds the binary before starting any timed CLI invocation, then
uses `cargo run` from the repository manifest. Temporary journals are
removed on completion. The explicit operator answer is automated test input,
not remote authentication or delivery to a human recipient.
