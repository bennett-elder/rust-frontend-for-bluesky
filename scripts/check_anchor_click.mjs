// Do in-app anchors navigate via the delegated handler? Android's WebView ignores
// fragment-only hrefs, so anchors are routed through JS; this checks that path.
// Usage: node scripts/check_anchor_click.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
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
const click = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
await wait(6000);

const handle = await run(`(() => {
  const a = document.querySelector('.post a[href^="#/profile"]');
  const r = a.getBoundingClientRect();
  return { href: a.getAttribute('href'), x: Math.round(r.left + 8), y: Math.round(r.top + r.height / 2), start: location.hash };
})()`);
await click(handle.x, handle.y);
await wait(3000);
const afterHandle = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);

await run(`history.back()`);
await wait(2500);
const permalink = await run(`(() => {
  const a = document.querySelector('.post a.post__permalink');
  const r = a.getBoundingClientRect();
  return { href: a.getAttribute('href'), x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
})()`);
await click(permalink.x, permalink.y);
await wait(3000);
const afterPermalink = await run(`({ hash: location.hash, back: Boolean(document.querySelector('.thread__back')) })`);

console.log('anchor-click:', JSON.stringify({ handle, afterHandle, permalink, afterPermalink }, null, 2));
ws.close();
process.exit(0);
