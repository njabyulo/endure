// Only the configured exact origin is allowed.
try {
  await fetch('http://127.0.0.1:8788/unauthorized');
} catch (error) {
  console.log(String(error));
}
const response = await fetch('http://127.0.0.1:8787/sample.json');
console.log((await response.json()).message);
