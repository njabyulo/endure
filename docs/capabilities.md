# Operator-granted capabilities

Trusted host configuration grants exact network origins, tool names, memory
scopes, and child-agent names. Missing grants deny access. These grants authorize
registered host operations; they do not provide an operating-system sandbox.

```yaml
grants:
  network_origins: ["https://api.example.com"]
  tools: ["customer.lookup"]
  memory_scopes: ["support"]
  child_agents: ["reviewer"]
credentials:
  provider: OPENAI_API_KEY
```

The CLI loads `ondure.yaml` from its current working directory. The Rust APIs
`run_durable_with_config` and `run_request_with_config` accept a `HostConfig` plus
`RunLimits`. Unknown configuration keys, malformed grants, wildcards, and
credential references that are not environment-variable names are rejected.
Credential values remain in the host environment; configuration stores names,
never resolved values. The capability layer does not yet dispatch configured
tools, memory, children, or provider requests; subsequent adapters must invoke
the same checks before dispatch.

Agent declarations request `Capabilities`; their effective authority is the
intersection of those requests and operator grants. Empty requests grant
nothing, and requests cannot elevate legacy broad network policy. The agent
lifecycle uses this host boundary before invoking a handler. Ordinary scripts
using scoped configuration receive the configured operator grants directly.

Network grants are canonical HTTP(S) origins: scheme, host, and nondefault port,
without a trailing slash, path, credentials, query, or fragment. Granting one
origin grants neither its subdomains nor another port or scheme. Scoped requests
reject URL userinfo and never follow automatic redirects, even to another
otherwise granted origin; the caller must make an explicitly authorized new
request. The runtime checks before registration and again immediately before
external dispatch. This does not implement IP-address filtering or DNS rebinding
protection; an operator grant trusts the named origin's resolution.

The complete trusted configuration, including environment-variable references,
is captured with the run and included in request identity. Reusing a request ID
with different grants or references conflicts. Replay uses captured grants;
editing the current YAML does not revoke a persisted run's authority. Access to
the journal and ability to start or resume runs therefore belong to the trusted
operator boundary. Completed operations reuse recorded outcomes.

Legacy `Policy { net: true }` Rust callers and YAML `permissions: [net]` retain
ordinary-script behavior, including broad network access and redirects. Existing
journals continue to load their recorded legacy policy. Scoped configuration
cannot be combined with nonempty legacy permissions.

Try the [local allowed/denied example](../examples/capabilities/README.md).
