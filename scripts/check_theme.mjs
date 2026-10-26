// The theme switch: three choices on Settings, persisted, and System really hands the
// decision back to the media query rather than freezing one palette.
//
// Colours are asserted, not just the attribute — an attribute that nothing reads would
// pass a weaker check while the screen stayed white.
//
// Usage: node scripts/check_theme.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const DARK = 'rgb(11, 15, 20)';
const LIGHT = 'rgb(255, 255, 255)';

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
const state = () =>
  run(`(() => {
    const active = [...document.querySelectorAll('.theme-choice__option')]
      .filter((button) => button.getAttribute('aria-pressed') === 'true')
      .map((button) => button.getAttribute('data-theme-value'));
    return {
      attribute: document.documentElement.dataset.theme ?? null,
      body: getComputedStyle(document.body).backgroundColor,
      active,
      stored: localStorage.getItem('altfirst.theme'),
    };
  })()`);
const load = async (hash) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url: base + hash });
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (await run("Boolean(document.querySelector('.theme-choice'))")) break;
    await wait(400);
  }
  await wait(600);
};
const choose = async (value) => {
  const spot = await centreOf(`.theme-choice__option[data-theme-value="${value}"]`);
  if (spot) await click(spot.x, spot.y);
  await wait(700);
  return state();
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Emulation.setEmulatedMedia', {
  features: [{ name: 'prefers-color-scheme', value: 'light' }],
});

// Start from "system on a light host", the state a new install is in.
await send('Page.navigate', { url: base });
await wait(1200);
await run("localStorage.removeItem('altfirst.theme')");
await load('#/settings');
const initial = await state();

const afterDark = await choose('dark');
await load('#/settings');
const darkReloaded = await state();

// A route change must not reset it.
await load('#/');
const darkOnFeed = await state();

await load('#/settings');
const afterLight = await choose('light');
await load('#/settings');
const lightReloaded = await state();

const afterSystem = await choose('system');
await load('#/settings');
const systemReloaded = await state();

// System on a dark host: no attribute, but the media query paints dark. This is the
// path that keeps the first paint correct before the app boots.
await send('Emulation.setEmulatedMedia', {
  features: [{ name: 'prefers-color-scheme', value: 'dark' }],
});
await load('#/settings');
const systemOnDarkHost = await state();
await send('Emulation.setEmulatedMedia', {
  features: [{ name: 'prefers-color-scheme', value: 'light' }],
});

const result = {
  initial,
  afterDark,
  darkReloaded,
  darkOnFeed,
  afterLight,
  lightReloaded,
  afterSystem,
  systemReloaded,
  systemOnDarkHost,
  pass:
    initial.attribute === null &&
    initial.body === LIGHT &&
    initial.active.join() === 'system' &&
    afterDark.attribute === 'dark' &&
    afterDark.body === DARK &&
    afterDark.active.join() === 'dark' &&
    darkReloaded.attribute === 'dark' &&
    darkReloaded.body === DARK &&
    darkOnFeed.attribute === 'dark' &&
    darkOnFeed.body === DARK &&
    afterLight.attribute === 'light' &&
    afterLight.body === LIGHT &&
    afterLight.active.join() === 'light' &&
    lightReloaded.attribute === 'light' &&
    lightReloaded.body === LIGHT &&
    afterSystem.attribute === null &&
    afterSystem.body === LIGHT &&
    afterSystem.active.join() === 'system' &&
    systemReloaded.attribute === null &&
    systemOnDarkHost.attribute === null &&
    systemOnDarkHost.body === DARK &&
    systemOnDarkHost.active.join() === 'system',
};
console.log('theme:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

