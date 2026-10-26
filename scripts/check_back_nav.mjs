// Does returning from a post keep the scrolled feed, or refetch page one?
// Usage: node scripts/check_back_nav.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/#/';
const cdp = 'http://localhost:9222';

const targets = await (await fetch(`${cdp}/json/list`)).json();
const page = targets.find((target) => target.type === 'page');
if (!page) throw new Error('no page target');

const ws = new WebSocket(page.webSocketDebuggerUrl);
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
const evaluate = async (expression) => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true });
  return result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Page.enable');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
await wait(5000);

// Build a scrolled feed: two pages deep, parked mid-list.
await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
await wait(3000);
await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
await wait(3000);

const scrolled = await evaluate(`({
  hash: location.hash,
  posts: document.querySelectorAll('.post').length,
  scrollY: Math.round(window.scrollY),
  height: document.documentElement.scrollHeight,
})`);

// Open a post from deep in the list, then come back the way a user would.
await evaluate(`document.querySelectorAll('.post a[href^="#/post/"]')[40]?.click()`);
await wait(2500);
const onPost = await evaluate(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);

await evaluate(`history.back()`);
await wait(3500);
const back = await evaluate(`({
  hash: location.hash,
  posts: document.querySelectorAll('.post').length,
  scrollY: Math.round(window.scrollY),
  height: document.documentElement.scrollHeight,
})`);

console.log('back-nav:', JSON.stringify({ scrolled, onPost, back }, null, 2));
ws.close();
process.exit(0);
