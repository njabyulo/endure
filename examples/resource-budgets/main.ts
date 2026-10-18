import { agent } from 'ondure';
export default agent('budget-demo', {
  capabilities: { network_origins: ['http://127.0.0.1:8787'] }
}, async ctx => {
  const results = await Promise.allSettled([
    ctx.model.call('First', { max_output_tokens: 10 }),
    ctx.model.call('Second', { max_output_tokens: 10 }),
    ctx.model.call('Third', { max_output_tokens: 10 }),
  ]);
  for (const result of results) {
    console.log(result.status === 'fulfilled' ? result.value.text : result.reason.message);
  }
});
