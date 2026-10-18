export const INSTALL =
  "git clone --branch feat/agent-runtime-website https://github.com/njabyulo/endure.git\ncd endure\ncargo build --bin ondure\npython3 examples/model-operations/run.py";

export const APPROVAL = `const decision = await ctx.human.approve({
  action: "Continue onboarding?",
  timeout: "1h",
  onTimeout: "reject",
});
console.log(decision.approved, decision.note);`;
