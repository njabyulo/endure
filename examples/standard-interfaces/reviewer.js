import { agent } from 'ondure';

export default agent('reviewer', async (_ctx, input) => ({
  reviewed: true,
  text: input.text,
}));
