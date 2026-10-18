# Standard tools and local child agents

From the repository root:

```sh
python3 examples/standard-interfaces/run.py
```

The runner starts a local MCP fixture and creates a temporary project with
explicit operator grants and child budget allocation. The parent calls
`ctx.tools.call('fixture.greet', { name: 'Reader' })`, then passes its structured
result to `ctx.agents.run('reviewer', input)`. The local child returns JSON and
requests empty authority. No paid credentials are needed.

The runner checks one external tool call, one persisted child run, and identical
replay output after removing source and configuration files. Set
`ONDURE_BIN=/absolute/path/to/ondure` to use a built executable; otherwise it uses
`cargo run` through the repository manifest.

Tool endpoints and local child modules are administrator configured. Children
receive explicit subsets of parent capabilities and reserved parent resources.
A child does not receive the parent's network or tool grants automatically.
See [the MCP profile](../../docs/mcp.md) and
[the integrated onboarding crash example](../onboarding/README.md).
