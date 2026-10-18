# Durable host-operation foundation

From the repository root:

```sh
cargo run -p ondure -- run examples/operations.js --run-id operation-demo
cargo run -p ondure -- log operation-demo
cargo run -p ondure -- replay operation-demo
```

The replay prints the same recorded clock and random sample. It reuses the
committed timer completion without waiting again. Use a fresh run ID for a new
execution. The default database is `.ondure/journal.sqlite3`.

Every registered operation has a stable identity `(run ID, sequence)`, a name,
validated JSON arguments, an explicit recovery policy, and a committed JSON
outcome. New arguments are bounded to 256 KiB and new outcomes to 1 MiB in encoded JSON.
Previously committed history is replayed under its original limits.
New records retain arguments alongside their digest; migrated historical records
can have no retained arguments. `log` includes adapter dispatch attempts.

External adapters commit their intent and dispatch attempt before sending work.
The current GET adapter explicitly allows retry of unfinished requests; servers
must still treat GET as safe. A committed response is reused during replay.
An operation using the interrupt recovery policy stops recovery when its outcome
is unknown. Unknown historical policies also remain non-retryable. This does not
provide exactly-once external effects.

Cancellation is cooperative: adapters can check a host cancellation token before
and after work. Exiting the event loop signals that token. It cannot undo an
external effect or forcibly cancel an in-flight blocking HTTP request; those
requests retain their own timeout. Oversized HTTP responses become recorded
errors before JavaScript receives them.

Durability applies to registered host operations, not arbitrary JavaScript
promises or callback state. Process-crash recovery does not establish hardware
power-loss guarantees.
