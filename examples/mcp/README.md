# Configured MCP tool example

In one terminal, from the repository root:

```sh
cd examples/mcp
python3 fixture.py
```

In another:

```sh
cd examples/mcp
cargo run --manifest-path ../../Cargo.toml -p ondure -- run main.js --run-id mcp-demo
cargo run --manifest-path ../../Cargo.toml -p ondure -- replay mcp-demo
cargo run --manifest-path ../../Cargo.toml -p ondure -- log mcp-demo
```

The first call requests an ungranted configured alias and is denied before any
server request. The granted call initializes an MCP session, lists tool schemas,
validates arguments, invokes the local `greet` tool, and prints `Hello, Reader`.
The server logs `tools/call greet` once; replay reuses the committed outcome.
No provider credentials are needed. Choose a fresh run ID for another live run.

See [the MCP profile](../../docs/mcp.md) for transport, schema, and recovery limits.
