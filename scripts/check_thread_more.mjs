// Folded replies must be openable, not just counted.
// Usage: node scripts/check_thread_more.mjs <post-url>
const url = process.argv[2];
if (!url) throw new Error('pass a post URL');
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
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url });
await wait(7000);

// The fold is a button with the overflow mounted as siblings, so counting `.post`
// before and after is the only honest measure — the old `.thread-more .post` probe
// described `<details>` markup this view no longer uses.
const before = await run(`({
  posts: document.querySelectorAll('.post').length,
  labels: [...document.querySelectorAll('.thread-more:not(.thread-more--junction)')].map((s) => s.textContent.trim()),
  staleNote: document.body.textContent.includes('not shown'),
})`);

if (before.labels.length > 0) {
  await run(`document.querySelector('.thread-more:not(.thread-more--junction)').click()`);
  await wait(1500);
}

const after = await run(`({
  posts: document.querySelectorAll('.post').length,
  open: [...document.querySelectorAll('.thread-more')].filter((b) => b.getAttribute('aria-expanded') === 'true').length,
  sample: (document.querySelector('.thread-more')?.innerText ?? '').slice(0, 90),
  hideLabel: [...document.querySelectorAll('.thread-more:not(.thread-more--junction)')]
    .map((button) => button.innerText.trim())
    .find((label) => /^Hide /.test(label)) ?? null,
})`);
after.opened = after.posts > before.posts;
after.pass = before.labels.length > 0 && after.open > 0 && after.opened && Boolean(after.hideLabel);

console.log('thread-more:', JSON.stringify({ before, after }, null, 2));
process.exit(after.pass ? 0 : 1);
ws.close();
