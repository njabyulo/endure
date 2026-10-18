// Run, inspect, and replay using examples/operations.md.
// Registered clocks, randomness, and timer outcomes are committed before use.
const started = Date.now();
const sample = Math.random();
await new Promise(resolve => setTimeout(resolve, 10));
console.log(JSON.stringify({ started, sample, completed: true }));
