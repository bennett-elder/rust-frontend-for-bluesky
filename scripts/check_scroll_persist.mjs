// Does a cold load of a feed return to the scroll offset you left it at, pulling
// the pages needed to get there? This is what makes Back work in a cmd-opened tab.
// Usage: node scripts/check_scroll_persist.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/#/';
const cdp = 'http://localhost:9222';

const pages = async () =>
  (await (await fetch(`${cdp}/json/list`)).json()).filter((t) => t.type === 'page');

const targets = await pages();
const page = targets[0];
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
await send('Network.enable');
// Unhashed wasm filename + python's http.server = the browser may reuse a stale
// build, which silently tests old code.
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
// Focus the page: an occluded window throttles timers, which would make the
// save path look broken when it is only being starved of time slices.
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
await wait(5000);

// Scroll two pages deep and let the offset reach localStorage.
await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
await wait(3000);
await evaluate(`window.scrollTo(0, document.documentElement.scrollHeight)`);
await wait(3000);

const scrolled = await evaluate(`({
  y: Math.round(window.scrollY),
  posts: document.querySelectorAll('.post').length,
  // Same key the app writes: one decoded route, not the raw hash.
  saved: (() => {
    const raw = location.hash.split('?')[0];
    let key = raw;
    try { key = decodeURIComponent(raw); } catch {}
    return JSON.parse(localStorage.getItem('altfirst.scroll') || '{}')[key] ?? null;
  })(),
})`);

// Cold load: fresh document, empty in-memory cache, same localStorage.
await send('Page.navigate', { url: 'about:blank' });
await wait(500);
await send('Page.navigate', { url });
await wait(1200);
const early = await evaluate(`({ y: Math.round(window.scrollY), posts: document.querySelectorAll('.post').length })`);
await wait(11000);

const restored = await evaluate(`({
  y: Math.round(window.scrollY),
  posts: document.querySelectorAll('.post').length,
  error: document.querySelector('.error')?.textContent ?? null,
})`);

console.log('scroll-persist:', JSON.stringify({ scrolled, early, restored }, null, 2));
ws.close();
process.exit(0);
