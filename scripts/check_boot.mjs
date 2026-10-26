// Opening the app must load whatever the route says, without touching a tab.
// This is the regression that showed "Loading public feed…" forever.
// Usage: node scripts/check_boot.mjs [url]
// Usage: node scripts/check_boot.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const urls = [base, `${base}#/`, `${base}#/following`];
const cdp = 'http://localhost:9222';

const targets = (await (await fetch(`${cdp}/json/list`)).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
};
await new Promise((resolve) => (ws.onopen = resolve));
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const messageId = ++id;
    pending.set(messageId, resolve);
    ws.send(JSON.stringify({ id: messageId, method, params }));
  });
const run = async (expression) => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true });
  return result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

const results = [];
for (const url of urls) {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url });
  await wait(8000);
  results.push(
    await run(`({
      url: ${JSON.stringify(url)},
      hash: location.hash,
      title: document.title,
      header: document.querySelector('.app-header h1')?.textContent ?? null,
      posts: document.querySelectorAll('.post').length,
      // The root route is the Explore page now, so the boot check looks for it.
      explore: Boolean(document.querySelector('.explore')),
      loading: document.body.textContent.includes('Loading public feed'),
      readOnlyNote: document.body.textContent.includes('Read-only'),
      // ErrorPanel renders .feed-error. This used to read .error, which never
      // exists, so the assertion could not fail.
      error: document.querySelector('.feed-error')?.textContent ?? null,
    })`),
  );
}

console.log('boot:', JSON.stringify(results, null, 2));
ws.close();
process.exit(0);
