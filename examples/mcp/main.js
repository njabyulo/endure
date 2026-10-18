import { agent } from 'ondure';

export default agent('mcp-demo', {
  capabilities: {
    network_origins: ['http://127.0.0.1:8789'],
    tools: ['fixture.greet', 'fixture.denied'],
  },
}, async ctx => {
  try {
    await ctx.tools.call('fixture.denied', { name: 'Reader' });
  } catch (error) {
    console.log(String(error));
  }
  const result = await ctx.tools.call('fixture.greet', { name: 'Reader' });
  console.log(result.structuredContent.greeting);
});
