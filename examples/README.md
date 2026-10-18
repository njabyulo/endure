# Runnable agent runtime examples

All default model and tool workflows use local fixtures without paid credentials.
Run commands from the repository root unless an example says otherwise.

| Feature | Example |
| --- | --- |
| Durable operations and replay | [Operations](operations.md) |
| Capability requests and grants | [Allowed and denied dispatch](capabilities/README.md) |
| Context, retrieval, and transactional state | [Agent memory](agent-memory/README.md) |
| Model calls | [Local Chat Completions fixture](model-operations/README.md) |
| Budgets and scheduling | [Resource budgets](resource-budgets/README.md) |
| Replay and observability | [Inspect and compare runs](inspection.md) |
| Durable human approval | [Agent lifecycle](agent-lifecycle/README.md) |
| MCP tools | [Streamable HTTP tools](mcp/README.md) |
| Local child agents | [Child delegation](child-agents/README.md) |
| Standard interfaces together | [MCP tool followed by a child](standard-interfaces/README.md) |
| Integrated interruption and recovery | [Onboarding workflow](onboarding/README.md) |

`timers.js` and `typescript/agent.ts` retain small ordinary-script examples.
See [the verification contract](../docs/feature-proof.md) for guarantees and
compatibility boundaries.
