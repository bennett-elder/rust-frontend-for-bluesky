// Post cards are divs (so text stays selectable), which means the browser has no
// anchor to apply its own link behavior to. This checks that we reproduce it:
// plain click navigates in place, cmd/ctrl-click and middle-click open a tab.
// Usage: node scripts/check_new_tab.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/#/';
const cdp = 'http://localhost:9222';

const pages = async () =>
  (await (await fetch(`${cdp}/json/list`)).json()).filter((t) => t.type === 'page');

const targets = await pages();
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

// Aim at the post text: inside the card, but not on the permalink anchor.
const spot = await evaluate(`(() => {
  const card = document.querySelector('[data-post-link]');
  const text = card?.querySelector('.post__text') ?? card;
  const rect = text.getBoundingClientRect();
  return {
    x: Math.round(rect.left + 16),
    y: Math.round(rect.top + 8),
    link: card?.getAttribute('data-post-link') ?? null,
  };
})()`);
if (!spot?.link) throw new Error('no post card with data-post-link');

// Real input events: synthetic dispatchEvent clicks would not exercise the
// browser's own modified-click handling.
const click = async (x, y, modifiers, button = 'left') => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button, clickCount: 1, modifiers });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button, clickCount: 1, modifiers });
};

const before = (await pages()).length;

// 1. Plain click: same tab, hash navigation, no extra tab.
await click(spot.x, spot.y, 0);
await wait(2000);
const plain = await evaluate(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);
const afterPlain = (await pages()).length;

await evaluate(`history.back()`);
await wait(2500);

// 2. Cmd-click (Meta = 4): a new tab at the post, current tab untouched.
await click(spot.x, spot.y, 4);
await wait(2000);
const metaOpened = (await pages()).length;
const metaHash = await evaluate(`location.hash`);

// 3. Middle-click: same expectation.
await click(spot.x, spot.y, 0, 'middle');
await wait(2000);
const middleOpened = (await pages()).length;

const opened = (await pages()).map((t) => t.url).filter((u) => u.includes('#/post/'));

// A tab at the right URL is worthless if the app boots blank, so look inside one.
let rendered = null;
const postTabs = (await pages()).filter((t) => t.url.includes('#/post/'));
if (postTabs.length > 0) {
  const tab = postTabs[postTabs.length - 1];
  const tabWs = new WebSocket(tab.webSocketDebuggerUrl);
  const tabPending = new Map();
  let tabId = 0;
  tabWs.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && tabPending.has(message.id)) {
      tabPending.get(message.id)(message);
      tabPending.delete(message.id);
    }
  };
  await new Promise((resolve) => (tabWs.onopen = resolve));
  const tabSend = (method, params = {}) =>
    new Promise((resolve) => {
      const messageId = ++tabId;
      tabPending.set(messageId, resolve);
      tabWs.send(JSON.stringify({ id: messageId, method, params }));
    });
  await tabSend('Runtime.enable');
  await wait(3000);
  const result = await tabSend('Runtime.evaluate', {
    expression: `({
      hash: location.hash,
      posts: document.querySelectorAll('.post').length,
      heading: document.querySelector('.thread h2, .thread__title')?.textContent ?? null,
      text: (document.querySelector('.post__text')?.textContent ?? '').slice(0, 40),
    })`,
    returnByValue: true,
  });
  rendered = result.result.result.value;
  tabWs.close();
}

console.log(
  'new-tab:',
  JSON.stringify(
    { expectedHash: spot.link, plain, afterPlain, metaOpened, metaHash, middleOpened, opened: opened.slice(0, 3), rendered },
    null,
    2,
  ),
);
ws.close();
process.exit(0);
