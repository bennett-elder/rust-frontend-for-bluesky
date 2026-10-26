// Focused pagination check: count posts, click Load more, count again.
// Usage: node scripts/check_pagination.mjs [url]
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

await send('Runtime.enable');
await send('Page.enable');
// A hash-only navigation does not reload the document, which would keep an older
// build's runtime alive. Go via about:blank to force a fresh load.
await send('Page.navigate', { url: 'about:blank' });
await new Promise((resolve) => setTimeout(resolve, 400));
await send('Page.navigate', { url });
await new Promise((resolve) => setTimeout(resolve, 5000));

const before = await evaluate(`({
  posts: document.querySelectorAll('.post').length,
  hasMore: Boolean(document.querySelector('.load-more')),
  end: document.querySelector('.feed-end')?.textContent ?? null,
})`);

await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
await new Promise((resolve) => setTimeout(resolve, 5500));

const after = await evaluate(`({
  posts: document.querySelectorAll('.post').length,
  hasMore: Boolean(document.querySelector('.load-more')),
  end: document.querySelector('.feed-end')?.textContent ?? null,
  error: document.querySelector('.error')?.textContent ?? null,
})`);

// Scroll repeatedly until the generator stops handing out cursors, so the
// terminal state is actually exercised rather than assumed.
let pages = 1;
let exhausted = false;
for (let step = 0; step < 5; step += 1) {
  const state = await evaluate(`({
    posts: document.querySelectorAll('.post').length,
    more: Boolean(document.querySelector('.load-more')),
    end: document.querySelector('.feed-end')?.textContent ?? null,
  })`);
  if (!state.more || state.end === "You've reached the end of this feed.") {
    exhausted = true;
    break;
  }
  await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
  await new Promise((resolve) => setTimeout(resolve, 2500));
  pages += 1;
}

console.log('pagination:', JSON.stringify({ before, after, pages, exhausted }, null, 2));
console.log('terminal:', JSON.stringify(await evaluate(`({
  posts: document.querySelectorAll('.post').length,
  unique: new Set([...document.querySelectorAll('.post a[href^="#/post/"]')].map((a) => a.getAttribute('href'))).size,
  more: Boolean(document.querySelector('.load-more')),
  end: document.querySelector('.feed-end')?.textContent ?? null,
  error: document.querySelector('.error')?.textContent ?? null,
})`), null, 2));
ws.close();
process.exit(0);
