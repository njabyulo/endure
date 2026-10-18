import { agent } from "ondure";

export default agent("onboarding-review", async (ctx, input) => {
  console.log("Preparing review for", input.name);
  await ctx.sleep("1s");
  const answer = await ctx.human.approve({
    action: `Continue onboarding ${input.name}`,
    recipient: "trusted local operator",
    timeout: "1h",
    onTimeout: "reject",
  });
  console.log(answer.approved ? "Approved" : "Rejected", answer.note);
});
