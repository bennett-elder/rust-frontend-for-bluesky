// Feeds live on their own page (`#/feeds`), not as a list inside the navigation rail.
// This asserts the page renders, the rail no longer duplicates feeds, the signed-out
// state says how to get your saved feeds, and a row actually navigates.
//
// Usage: node scripts/check_feeds_page.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
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
  const message = await send('Runtime.evaluate', { expression, returnByValue: true });
  if (message.result?.exceptionDetails) return { threw: message.result.exceptionDetails.exception?.description };
  return message.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const click = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
// about:blank first so a fresh build is actually loaded (hash-only nav reuses it).
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: `${base}#/feeds` });
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("document.querySelectorAll('.feed-list li').length > 0")) break;
  await wait(500);
}
await wait(600);

const page = await run(`(() => ({
  hash: location.hash,
  title: document.title,
  heading: document.querySelector('.page-title')?.innerText ?? null,
  rows: [...document.querySelectorAll('.feed-list__link')].map((link) => ({
    name: link.querySelector('.feed-list__name')?.innerText ?? null,
    href: link.getAttribute('href'),
  })),
  note: document.querySelector('.feeds-page .note')?.innerText ?? null,
  railItems: [...document.querySelectorAll('.rail__item')].map((item) => item.innerText.trim()).filter(Boolean),
  activeRail: document.querySelector('.rail__item--active')?.innerText.trim() ?? null,
  signedIn: !document.body.innerText.includes('PUBLIC · LOGGED OUT'),
}))()`);

const row = await run(`(() => {
  const link = document.querySelector('.feed-list__link');
  if (!link) return null;
  const rect = link.getBoundingClientRect();
  return { href: link.getAttribute('href'), x: Math.round(rect.left + 20), y: Math.round(rect.top + rect.height / 2) };
})()`);
if (row) await click(row.x, row.y);
await wait(2200);
const after = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);

const linkedFeeds = (page.rows ?? []).filter((entry) => entry.href?.startsWith('#/feed/'));
const result = {
  page,
  clicked: row,
  after,
  pass:
    Array.isArray(page.rows) &&
    page.rows.length > 0 &&
    page.heading === 'Feeds' &&
    page.title.startsWith('Feeds —') &&
    page.activeRail === 'Feeds' &&
    page.railItems.includes('Feeds') &&
    // The rail links to the page; it must not list feeds itself.
    linkedFeeds.length === 0 &&
    page.rows.every((entry) => typeof entry.href === 'string' && entry.href.startsWith('#/')) &&
    (page.signedIn || /sign in/i.test(page.note ?? '')) &&
    after.hash !== '#/feeds',
};
console.log('feeds-page:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

