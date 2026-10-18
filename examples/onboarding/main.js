import { agent } from 'ondure';

export default agent('onboarding', {
  capabilities: {
    network_origins: ['http://127.0.0.1:8790'],
    tools: ['onboarding.register'],
    memory_scopes: ['onboarding'],
    child_agents: ['reviewer'],
  },
}, async (ctx, input) => {
  await ctx.context.add({ role: 'system', content: 'Prepare a brief welcome.' });
  await ctx.memory.remember(`Customer ${input.name} prefers olive green`, { scope: 'onboarding' });
  const welcome = await ctx.model.call(`Welcome ${input.name}`, { max_output_tokens: 32 });
  const answer = await ctx.human.approve({
    action: 'Register customer in the local mock receiver',
    recipient: 'trusted local CLI operator',
    rationale: welcome.text,
  });
  if (!answer.approved) {
    console.log(JSON.stringify({ approved: false, note: answer.note }));
    return;
  }
  // The fixture deduplicates this explicit application key. This is a mock,
  // not a claim that arbitrary email/payment effects run exactly once.
  const receipt = await ctx.tools.call('onboarding.register', {
    operation_id: input.operation_id,
    name: input.name,
  });
  const review = await ctx.agents.run('reviewer', { text: welcome.text });
  await ctx.state.set('last-onboarded', input.name, { scope: 'onboarding' });
  console.log(JSON.stringify({
    approved: answer.approved,
    note: answer.note,
    receipt: receipt.structuredContent,
    review,
    saved: await ctx.state.get('last-onboarded', { scope: 'onboarding' }),
  }));
});
