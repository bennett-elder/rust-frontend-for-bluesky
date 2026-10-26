// Double-click Home (or the feed tab you are on) to get back to the top, instead of
// being dropped 900 px down by the saved offset.
//
// Asserts the deliberate part: one click must NOT jump, two must, the offset must be
// forgotten so a reload stays at the top, and a signed-out Home double-click must not
// wedge the restore-skip flag.
//
// Usage: node scripts/check_double_click_top.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const feed = 'at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot';
const feedUrl = `${base}#/feed/${feed}`;
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
const doubleClick = async (x, y) => {
  await click(x, y);
  await wait(110);
  await click(x, y);
};
const centreOf = (selector) =>
  run(`(() => {
    const element = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((candidate) => candidate.getClientRects().length > 0);
    if (!element) return null;
    const rect = element.getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
  })()`);
const scrollY = () => run('Math.round(window.scrollY)');
const storedFor = (route) =>
  run(`(() => {
    try {
      const store = JSON.parse(localStorage.getItem('altfirst.scroll') || '{}') || {};
      return store[${JSON.stringify(route)}] ?? null;
    } catch { return null; }
  })()`);
const open = async (url) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(350);
  await send('Page.navigate', { url });
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (await run("document.querySelectorAll('.post').length > 5")) break;
    await wait(400);
  }
  await wait(900);
  await run('window.scrollTo(0, 0)');
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

// ---- The tab you are already on (narrow layout, where the tabs are visible) --------
await send('Emulation.setDeviceMetricsOverride', {
  width: 430,
  height: 900,
  deviceScaleFactor: 2,
  mobile: true,
});
await open(feedUrl);
await run("localStorage.removeItem('altfirst.scroll')");
await wait(200);

const activeTab = await run(`(() => {
  const tab = document.querySelector('.feed-tab--active[data-scroll-top="true"]');
  return tab ? tab.textContent.trim() : null;
})()`);

// Scroll, let the debounced listener persist it, then click once.
await run('window.scrollTo(0, 1200)');
await wait(600);
// Pinned: the tab row is the column's top bar, so it must still be at the top of the
// viewport while the feed is scrolled. It lived inside `.app-header` at one point and
// scrolled away with it, and before that it was hidden entirely on desktop.
const pinned = await run(`(() => {
  const tabs = document.querySelector('.feed-tabs');
  if (!tabs) return { found: false };
  const rect = tabs.getBoundingClientRect();
  return {
    found: true,
    display: getComputedStyle(tabs).display,
    top: Math.round(rect.top),
    scrollY: Math.round(window.scrollY),
  };
})()`);
const savedBefore = await storedFor(`#/feed/${feed}`);
await run(`document.querySelector('.feed-tab--active[data-scroll-top="true"]').click()`);
await wait(700);
const afterSingle = await scrollY();

// Two clicks within the window.
await run(`(() => {
  const tab = document.querySelector('.feed-tab--active[data-scroll-top="true"]');
  tab.click();
  setTimeout(() => tab.click(), 110);
})()`);
await wait(1400);
const afterDouble = await scrollY();
const savedAfter = await storedFor(`#/feed/${feed}`);
const savedAfterY = savedAfter ? savedAfter.y : null;

// A fresh load must not restore the old offset.
await open(feedUrl);
await wait(1200);
const afterReload = await scrollY();

// After normalising the key, the same feed must not be stored twice (encoded + raw).
const keysForFeed = await run(`(() => {
  try {
    return Object.keys(JSON.parse(localStorage.getItem('altfirst.scroll') || '{}'))
      .filter((key) => key.includes('whats-hot')).length;
  } catch { return -1; }
})()`);

// The skip flag is what Home uses to land at the top rather than on the saved offset.
await run('window.scrollTo(0, 1200)');
await wait(600);
await run('window.__altSkipRestore = true; window.__altScrollRestore()');
await wait(400);
const afterSkipRestore = await scrollY();
const skipConsumed = await run('Boolean(window.__altSkipRestore)');

await send('Emulation.setDeviceMetricsOverride', {
  width: 1280,
  height: 900,
  deviceScaleFactor: 1,
  mobile: false,
});
await open(feedUrl);
const homeMarked = await run(
  `document.querySelector('.rail a[href="#/following"]')?.getAttribute('data-scroll-top') ?? null`,
);
// Desktop keeps the tab row: it is the only feed navigation in the column there.
const desktopTabs = await run(`(() => {
  const tabs = document.querySelector('.feed-tabs');
  if (!tabs) return { found: false };
  const rect = tabs.getBoundingClientRect();
  return { found: true, display: getComputedStyle(tabs).display, top: Math.round(rect.top) };
})()`);
await run('window.scrollTo(0, 1200)');
await wait(500);
const homeSpot = await centreOf('.rail a[href="#/following"]');
if (homeSpot) await doubleClick(homeSpot.x, homeSpot.y);
await wait(1200);
const afterHomeDouble = await scrollY();
const homeState = await run(`({
  hash: location.hash,
  login: Boolean(document.querySelector('.login-form')),
})`);
await wait(3200);
const skipFlag = await run('Boolean(window.__altSkipRestore)');
await send('Emulation.clearDeviceMetricsOverride');

const result = {
  activeTab,
  pinned,
  desktopTabs,
  savedBefore: savedBefore?.y ?? null,
  afterSingle,
  afterDouble,
  savedAfter: savedAfterY,
  afterReload,
  keysForFeed,
  afterSkipRestore,
  skipConsumed,
  homeMarked,
  afterHomeDouble,
  homeState,
  skipFlag,
  pass:
    activeTab !== null &&
    pinned.found === true &&
    pinned.display !== 'none' &&
    pinned.top === 0 &&
    desktopTabs.found === true &&
    desktopTabs.display !== 'none' &&
    (savedBefore?.y ?? 0) > 800 &&
    afterSingle > 800 &&
    afterDouble < 100 &&
    (savedAfterY === null || savedAfterY === 0) &&
    afterReload === 0 &&
    keysForFeed === 1 &&
    afterSkipRestore === 0 &&
    skipConsumed === false &&
    homeMarked === 'home' &&
    afterHomeDouble < 100 &&
    homeState.login === true &&
    skipFlag === false,
};
console.log('double-click-top:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

