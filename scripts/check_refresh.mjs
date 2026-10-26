// Cache-hit revalidation: returning to a feed must serve the cache instantly and
// then announce newer posts rather than swapping the list under the reader.
// Usage: node scripts/check_refresh.mjs [feed-url]
const feed = process.argv[2] ?? 'http://127.0.0.1:8099/#/';
const cdp = 'http://localhost:9222';

const targets = (await (await fetch(`${cdp}/json/list`)).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.method === 'Network.requestWillBeSent') {
    const url = message.params?.request?.url ?? '';
    if (url.includes('app.bsky.feed.getFeed')) requests.push(url);
  }
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
};
const requests = [];
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
await wait(400);
await send('Page.navigate', { url: feed });
await wait(5000);

const first = await run(`({
  posts: document.querySelectorAll('.post').length,
  head: document.querySelector('.post a[href^="#/post/"]')?.getAttribute('href') ?? null,
})`);

// Leave the feed so the next visit is a cache hit, and give the generator time
// to produce something new in the meantime.
await run(`location.hash = '#/profile/bsky.app'`);
await wait(2500);
await wait(1500);
await run(`location.hash = ${JSON.stringify(feed.split('#')[1] ? '#' + feed.split('#')[1] : '#/')}`);
await wait(4500);

const revisit = await run(`({
  posts: document.querySelectorAll('.post').length,
  head: document.querySelector('.post a[href^="#/post/"]')?.getAttribute('href') ?? null,
  banner: document.querySelector('.new-posts')?.textContent ?? null,
  error: document.querySelector('.error')?.textContent ?? null,
})`);

let accepted = null;
if (revisit.banner) {
  await run(`document.querySelector('.new-posts').click()`);
  await wait(1500);
  accepted = await run(`({
    posts: document.querySelectorAll('.post').length,
    banner: document.querySelector('.new-posts')?.textContent ?? null,
    y: Math.round(window.scrollY),
    head: document.querySelector('.post a[href^="#/post/"]')?.getAttribute('href') ?? null,
  })`);
}

console.log('refresh:', JSON.stringify({ first, revisit, accepted, requests: requests.length }, null, 2));

// The feed you sit on: refocusing the tab must ask the server again, otherwise
// the "new posts" pill can never appear without navigating away and back.
const baseline = requests.length;
const hasTrigger = await run(`Boolean(document.querySelector('.refresh-tick'))`);
await run(`document.querySelector('.refresh-tick')?.click()`);
await wait(3000);
const afterClick = requests.length;
await send('Runtime.evaluate', { expression: "window.dispatchEvent(new Event('focus'))" });
await wait(3000);
const afterFocus = requests.length;
await send('Runtime.evaluate', { expression: "window.dispatchEvent(new Event('focus'))" });
await wait(3000);
const afterSecondFocus = requests.length;
console.log('tick:', JSON.stringify({ baseline, hasTrigger, afterClick, afterFocus, hidden: await run('document.hidden'), state: await run('document.visibilityState') }, null, 0));
ws.close();
process.exit(0);
