# Configured MCP tools

`await ctx.tools.call(alias, arguments)` calls an administrator-configured MCP
tool. Both the exact alias and the endpoint's exact network origin must appear
in agent requests and operator grants. Denial precedes external dispatch. Servers
are never discovered from agent-provided URLs, packages, or subprocess commands.

```yaml
grants:
  network_origins: ["https://tools.example.com"]
  tools: ["customer.lookup"]
credentials:
  support: SUPPORT_MCP_TOKEN
mcp_servers:
  support:
    endpoint: "https://tools.example.com/mcp"
    credential: support
    tools:
      customer.lookup:
        name: lookup_customer
        retry_interrupted: false
```

Aliases are unique across configured servers. The credential field names a key
in `credentials`; the host resolves that environment-variable reference only
inside adapter execution. Credential values are absent from operation arguments
and snapshots. Responses containing the resolved credential are rejected.

This is a bounded Streamable HTTP tools profile: initialization negotiates
2025-11-25, 2025-06-18, or 2025-03-26; requests use the negotiated version and
server session ID. Both JSON replies and SSE replies to POST are supported,
including notifications preceding the matching response. Notifications receive
202 acknowledgments, and sessions are deleted after each operation when
possible. No redirects, legacy HTTP+SSE endpoint discovery, stdio, server-initiated
requests, standalone GET subscriptions, SSE reconnect/resume, or asynchronous
task APIs are supported. Tools/list pagination is limited to eight pages and
512 unique tool names. Each request has a ten-second timeout.

The adapter fetches the selected tool's schema before calling it. Input and
output schema validation accepts an explicit bounded subset: object, array,
string, number, integer, boolean, and null types; properties, required,
additionalProperties, items, enum, const, min/max item and string lengths, and
minimum/maximum numbers. Boolean schemas and descriptive annotations are also
accepted. Schemas require explicit types, at most 32 nested levels and 128
properties or enum members; numeric values are limited in magnitude to
9,007,199,254,740,991. Unsupported assertions, references, definitions, regexes,
combinators, and external resolution are rejected. This deliberately rejects
some otherwise valid general JSON Schema documents.

Arguments are JSON objects bounded by the operation's 256 KiB argument limit.
HTTP/SSE responses are capped at 1 MiB, including envelopes and stream metadata;
the journaled result must fit the shared outcome limit. This profile returns
text content blocks and optional structuredContent, validating outputSchema
when provided. Binary and resource blocks are rejected. `isError: true` is a
recorded tool outcome for the caller to inspect; malformed protocol replies
reject the promise.

Completed results commit before delivery to JavaScript and replay without the
server. An interrupted tool operation defaults to an `InterruptedError`, even
if the crash happened during initialization before tools/call. The operator may
set `retry_interrupted: true` only when repeating the entire call is safe for
that adapter. Server tool annotations do not grant retry authority. Unknown
outcomes are not automatically retried; no exactly-once external effect or
remote cancellation guarantee is provided. Cancellation is cooperative and an
in-flight request can still finish remotely.

The transport and lifecycle follow the supported portions of the official
[MCP transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
and [tools specification](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
Try the [local fixture](../examples/mcp/README.md).

Credentialed MCP endpoints require HTTPS, except exact loopback hosts (`localhost`,
IPv4 loopback addresses, and IPv6 `::1`) for local development. Credentials are
rejected if reflected in JSON strings, keys, numbers, booleans, or null values.
