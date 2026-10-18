# Compare recorded runs

```sh
cargo run -p ondure -- run examples/inspection.js --run-id inspect-a
cargo run -p ondure -- run examples/inspection.js --run-id inspect-b
cargo run -p ondure -- log inspect-a
cargo run -p ondure -- explain inspect-a
cargo run -p ondure -- diff inspect-a inspect-b
cargo run -p ondure -- replay inspect-a
```

Use fresh run IDs for a new pair. These commands share `.ondure/journal.sqlite3`;
`--journal PATH` selects another database. `diff` compares recorded operations by
registration sequence and reports metadata changes. It does no model or tool
work. An identical comparison describes recorded data, not external effect
identity or provider equivalence. Replay verifies the original captured history
and can resume supported unfinished operations; it does not fork or override it.

`log` exposes structured operation arguments, policies, outcomes, completion
order, waits and adapter attempts. `explain` also includes recorded input,
permissions and console output. Usage and supplied rationale appear when an
adapter or application recorded them in an outcome. No hidden model reasoning
is requested or reconstructed. Application data in the journal and inspection
output needs appropriate access controls.
