import { agent } from "ondure";
export default agent("reviewer", async (_ctx, input) => {
  return { greeting: `Hello ${input.name}`, reviewed: true };
});
