// Cold-loads any URL and reports what rendered. Exists because hash-only
// navigation hid a real bug: deep links typed into the address bar are parsed at
// boot, not by clicking, and that path was never covered.
// Usage: node scripts/check_route.mjs <url> [wait-ms]
const url = process.argv[2];
if (!url) throw new Error('pass a URL');
const settle = Number(process.argv[3] ?? 8000);
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
  return result.result.exceptionDetails
    ? { threw: result.result.exceptionDetails.exception?.description }
    : result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
await wait(settle);

console.log(
  'route:',
  JSON.stringify(
    await run(`({
      asked: ${JSON.stringify(url)},
      hash: location.hash,
      title: document.title,
      profile: Boolean(document.querySelector('.profile')),
      thread: Boolean(document.querySelector('.thread')),
      posts: document.querySelectorAll('.post').length,
      error: document.querySelector('.feed-error__headline')?.textContent ?? null,
      firstText: document.body.innerText.slice(0, 60).replace(/\\n/g, ' | '),
    })`),
    null,
    2,
  ),
);
ws.close();
process.exit(0);
