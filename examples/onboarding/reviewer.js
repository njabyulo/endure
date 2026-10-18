import { agent } from 'ondure';

// An empty request grants no tools, network, memory scopes, or further children.
export default agent('reviewer', async (_ctx, input) => ({
  reviewed: true,
  text: input.text,
}));
