import { agent } from "ondure";
export default agent(
  "parent",
  { capabilities: { child_agents: ["reviewer"] } },
  async (ctx, input) => {
    const result = await ctx.agents.run("reviewer", { name: input.name });
    console.log(JSON.stringify(result));
    return result;
  },
);
