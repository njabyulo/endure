console.log('start');

const canceled = setTimeout(() => console.log('canceled timer ran'), 1000);
clearTimeout(canceled);

(async () => {
  const value = await new Promise(resolve => setTimeout(resolve, 25, 'ready'));
  console.log(value);
})();

Promise.resolve().then(() => console.log('microtask'));
console.log('end');
