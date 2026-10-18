import { agent } from "ondure";

export default agent(
  "model-greeting",
  { capabilities: { network_origins: ["http://127.0.0.1:8787"] } },
  async (ctx, input) => {
    const result = await ctx.model.call({
      messages: [{ role: "user", content: `Say hello to ${input.name}` }],
      max_output_tokens: 32,
    });
    console.log(result.text);
    console.log(JSON.stringify(result.usage));
  },
);
