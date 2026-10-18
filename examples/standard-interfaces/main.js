import { agent } from 'ondure';

export default agent('standard-interfaces', {
  capabilities: {
    network_origins: ['http://127.0.0.1:8789'],
    tools: ['fixture.greet'],
    child_agents: ['reviewer'],
  },
}, async ctx => {
  const greeting = await ctx.tools.call('fixture.greet', { name: 'Reader' });
  const reviewed = await ctx.agents.run('reviewer', {
    text: greeting.structuredContent.greeting,
  });
  console.log(JSON.stringify(reviewed));
});
