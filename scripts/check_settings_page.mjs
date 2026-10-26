// Settings is a page (`#/settings`), not a popover over the feed. This asserts the
// destination exists and is reached by both the rail link and the narrow-screen gear,
// that the one preference renders there, and that toggling it does not navigate.
//
// Usage: node scripts/check_settings_page.mjs [base-url]
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
const centreOf = (selector) =>
  run(`(() => {
    const element = [...document.querySelectorAll(${JSON.stringify(selector)})]
      .find((candidate) => candidate.getClientRects().length > 0);
    if (!element) return null;
    const rect = element.getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
  })()`);
const boot = async (width, height, mobile) => {
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: mobile ? 2 : 1, mobile });
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url: base });
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (await run("Boolean(document.querySelector('.explore'))")) break;
    await wait(500);
  }
  await wait(500);
  await run('window.scrollTo(0, 0)');
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

// Desktop: the rail carries the Settings link.
await boot(1280, 900, false);
const railSettings = await centreOf('.rail a[aria-label="Settings"]');
if (railSettings) await click(railSettings.x, railSettings.y);
await wait(1500);
const page = await run(`(() => ({
  hash: location.hash,
  title: document.title,
  heading: document.querySelector('.page-title')?.innerText ?? null,
  toggle: document.querySelector('.settings-row input[type="checkbox"]')?.checked ?? null,
  label: document.querySelector('.settings-row__control')?.innerText.trim() ?? null,
  hint: document.querySelector('.settings-row__hint')?.innerText.trim() ?? null,
  activeRail: document.querySelector('.rail__item--active')?.innerText.trim() ?? null,
  popover: Boolean(document.querySelector('.app-menu')),
}))()`);

const toggled = await (async () => {
  const box = await centreOf('.settings-row input[type="checkbox"]');
  const before = await run('location.hash');
  if (box) await click(box.x, box.y);
  await wait(600);
  const after = await run(`({
    checked: document.querySelector('.settings-row input[type="checkbox"]')?.checked ?? null,
    hash: location.hash,
    before: ${JSON.stringify(before)},
  })`);
  return after;
})();

// Narrow: the header gear reaches the same page.
await boot(390, 844, true);
const gear = await centreOf('.app-header a[aria-label="Settings"]');
if (gear) await click(gear.x, gear.y);
await wait(1500);
const mobile = await run(`({ hash: location.hash, heading: document.querySelector('.page-title')?.innerText ?? null })`);
await send('Emulation.clearDeviceMetricsOverride');

const result = {
  page,
  toggled,
  mobile,
  pass:
    page.hash === '#/settings' &&
    page.title.startsWith('Settings —') &&
    page.heading === 'Settings' &&
    typeof page.toggle === 'boolean' &&
    /alt text/i.test(page.label ?? '') &&
    typeof page.hint === 'string' &&
    page.hint.length > 20 &&
    page.activeRail === 'Settings' &&
    page.popover === false &&
    toggled.checked === !page.toggle &&
    toggled.hash === toggled.before &&
    mobile.hash === '#/settings' &&
    mobile.heading === 'Settings',
};
console.log('settings-page:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

