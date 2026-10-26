// Failure messages must say which side failed, and offer a retry.
// Usage: node scripts/check_errors.mjs
const cdp = 'http://localhost:9222';
const base = 'http://127.0.0.1:8099/';

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
const panel = () =>
  run(`({
    headline: document.querySelector('.feed-error__headline')?.textContent ?? null,
    hint: document.querySelector('.feed-error .note')?.textContent ?? null,
    detail: document.querySelector('.feed-error__detail')?.textContent ?? null,
    retry: Boolean([...document.querySelectorAll('.feed-error button')].find((b) => b.textContent === 'Try again')),
  })`);

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

const results = {};

// 1. Session-only route while signed out: our own precondition, not a network fault.
await send('Page.navigate', { url: `about:blank` });
await wait(300);
await send('Page.navigate', { url: `${base}#/following` });
await wait(3000);
results.signedOutFollowing = await panel();

// 2. Retry must re-attempt rather than dead-end.
await run(`[...document.querySelectorAll('.feed-error button')].find((b) => b.textContent === 'Try again')?.click()`);
await wait(2500);
results.afterRetry = await panel();

// 3. A generator that does not exist: 400/404 -> "not available", not "no network".
await run(`location.hash = '#/feed/at%3A%2F%2Fdid%3Aplc%3Aaaaaaaaaaaaaaaaaaaaaaaaa%2Fapp.bsky.feed.generator%2Fnope'`);
await wait(3000);
results.missingFeed = await panel();

// Threads and profiles use the same panel, with the right noun.
await run(`location.hash = '#/post/at%3A%2F%2Fdid%3Aplc%3Aaaaaaaaaaaaaaaaaaaaaaaaa%2Fapp.bsky.feed.post%2Fnope'`);
await wait(3000);
results.missingPost = await panel();

await run(`location.hash = '#/profile/this-handle-does-not-exist-zzz.invalid'`);
await wait(3000);
results.missingProfile = await panel();

// Happy path must still render the author feed (the arm was just rewritten).
await run(`location.hash = '#/profile/bsky.app'`);
await wait(5000);
results.profilePosts = await run(`({
  posts: document.querySelectorAll('.post').length,
  panel: Boolean(document.querySelector('.feed-error')),
})`);

console.log('errors:', JSON.stringify(results, null, 2));
ws.close();
process.exit(0);
