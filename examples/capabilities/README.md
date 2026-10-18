# Scoped capability dispatch

In one terminal, from the repository root:

```sh
cd examples/capabilities
python3 -m http.server 8787 --bind 127.0.0.1
```

In another terminal:

```sh
cd examples/capabilities
cargo run --manifest-path ../../Cargo.toml -p ondure -- run main.js --run-id capability-demo
cargo run --manifest-path ../../Cargo.toml -p ondure -- log capability-demo
cargo run --manifest-path ../../Cargo.toml -p ondure -- replay capability-demo
```

The CLI reads `ondure.yaml` from its working directory. The request to port 8788
is denied before dispatch; the request to port 8787 succeeds. Replay prints the
same output without another request to the local server. A new run needs a fresh
run ID, or a fresh database using `--journal /tmp/capability-demo.sqlite3`.

See [the capability contract](../../docs/capabilities.md) for operator grants,
agent request attenuation, credential references, and replay semantics.
