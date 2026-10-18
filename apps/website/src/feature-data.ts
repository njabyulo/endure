export type FeatureExample = { title: string; detail: string; code: string; path: string; language?: string; syntax?: "javascript" | "yaml" | "shellscript" };

export const featureExamples: FeatureExample[] = [
  {
    title: "Durable execution",
    detail: "A committed model response is reused after interruption. Replay verifies captured code and history, then resumes supported unfinished work under its retry policy.",
    path: "examples/onboarding/README.md",
    code: `const welcome = await ctx.model.call(
  \`Welcome \${input.name}\`, { max_output_tokens: 32 });
const answer = await ctx.human.approve({
  action: "Register customer in the local mock receiver",
  rationale: welcome.text,
});
// After restart, committed model + approval outcomes replay.
// The linked fixture kills the process and verifies recovery.`,
  },
  {
    title: "Capability permissions",
    detail: "An agent requests authority. The operator grants it. Their intersection controls dispatch; a missing tool grant rejects the call before the server receives a request.",
    path: "examples/mcp/README.md",
    code: `// Requested by the agent; omitted from operator grants:
try {
  await ctx.tools.call("fixture.denied", {
    name: "Reader",
  });
} catch (error) {
  console.log(String(error));
}`,
  },
  {
    title: "Memory primitives",
    detail: "Keep conversation context, retrieve scoped facts with SQLite full-text search, and update shared state with version checks. Retrieval and state outcomes are recorded for replay.",
    path: "examples/agent-memory/README.md",
    code: `await ctx.context.add({ role: "user", content: "Olive, please" });
await ctx.memory.remember("Prefers olive", { scope: "demo" });
const facts = await ctx.memory.recall("olive", { scope: "demo" });
const current = await ctx.state.get("visits", { scope: "demo" });
await ctx.state.compareAndSet("visits", current?.version ?? 0,
  (current?.value ?? 0) + 1, { scope: "demo" });`,
  },
  {
    title: "Budgets and scheduling",
    detail: "Reserve token and spend estimates before dispatch. Queue work behind concurrency and rate limits. Elapsed deadlines include parked time; usage and configured prices guide reconciliation.",
    path: "examples/resource-budgets/README.md",
    language: "OPERATOR YAML",
    syntax: "yaml",
    code: `resources:
  token_limit: 10000
  dollar_limit_micros: 2000000 # USD 2
  model_concurrency: 2
  tool_concurrency: 4
  model_rate: { max_calls: 10, window_ms: 60000 }
  elapsed_deadline_ms: 900000`,
  },
  {
    title: "Replay and observability",
    detail: "Inspect recorded inputs, permissions, outcomes and usage. Compare registrations and outcomes across runs. Supplied rationale can be recorded; hidden model reasoning is not exposed.",
    path: "examples/inspection.md",
    language: "TERMINAL",
    syntax: "shellscript",
    code: `ondure log inspect-a
ondure explain inspect-a
ondure diff inspect-a inspect-b
ondure replay inspect-a

# Inspection does no model or tool work.
# Replay can resume supported unfinished operations.`,
  },
  {
    title: "Durable human approval",
    detail: "Release the isolate while an approval is pending. A trusted local CLI operator answers with an optional note; the first committed answer or rejection timeout wins.",
    path: "examples/agent-lifecycle/README.md",
    code: `const decision = await ctx.human.approve({
  action: "Continue onboarding?",
  recipient: "trusted local operator",
  timeout: "1h",
  onTimeout: "reject",
});
console.log(decision.approved, decision.note);`,
  },
  {
    title: "Standard interfaces",
    detail: "Call an administrator-configured MCP tool over Streamable HTTP, then invoke a captured local child agent. Children receive explicit subsets of authority and reserved parent resources.",
    path: "examples/standard-interfaces/README.md",
    code: `const greeting = await ctx.tools.call("fixture.greet", {
  name: "Reader",
});
const review = await ctx.agents.run("reviewer", {
  text: greeting.structuredContent.greeting,
});
console.log(review);`,
  },
];
