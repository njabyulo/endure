import { agent } from 'ondure';
export default agent('memory-demo', {
  capabilities: { memory_scopes: ['demo'] }
}, async (ctx) => {
  await ctx.context.add({ role: 'system', content: 'Help the customer.' });
  await ctx.context.add({ role: 'user', content: 'I prefer olive green.' });
  await ctx.memory.remember('Customer prefers olive green', { scope: 'demo' });
  console.log(await ctx.memory.recall('olive', { scope: 'demo' }));
  const current = await ctx.state.get('visits', { scope: 'demo' });
  console.log(await ctx.state.compareAndSet('visits', current?.version ?? 0,
    (current?.value ?? 0) + 1, { scope: 'demo' }));
});
