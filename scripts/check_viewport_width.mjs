import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

// The page must never be wider than the phone it is on.
//
// A 411 px phone scrolled sideways by ~30 px because `.timeline` is a flex item, and a flex
// item's `min-width: auto` floors it at its min-content width — the header's account row and
// tab strip are wider than that viewport. Nothing about it shows on a desktop, so it took
// someone using a phone to report it. This asserts the invariant at the widths a phone actually
// has, signed in and signed out, and that the desktop rail still appears at 1280.
//
// Usage: node scripts/check_viewport_width.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const WIDTHS = [320, 360, 411];

const home = process.env.HOME ?? '';
const candidates = [
  `${home}/Library/Caches/ms-playwright/chromium-1217/chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`,
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
];
const chrome = process.env.CHROME ?? candidates.find((path) => existsSync(path));
if (!chrome) throw new Error('no Chrome binary found; set CHROME=...');

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const list = async () => (await fetch(`${cdp}/json/list`)).json();

async function ensureBrowser() {
  try {
    await fetch(`${cdp}/json/version`);
    return;
  } catch {
    // fall through and start our own
  }
  spawn(
    chrome,
    [
      '--headless=new',
      '--no-sandbox',
      '--disable-gpu',
      '--mute-audio',
      '--use-mock-keychain',
      '--password-store=basic',
      '--remote-debugging-port=9222',
      `--user-data-dir=${process.cwd()}/.tmp/chrome-profile`,
      'about:blank',
    ],
    { detached: true, stdio: 'ignore' },
  ).unref();
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      if ((await list()).some((target) => target.type === 'page')) return;
    } catch {
      // not up yet
    }
    await wait(250);
  }
  throw new Error('could not reach Chrome DevTools on :9222');
}

await ensureBrowser();
const page = (await list()).find((target) => target.type === 'page');
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
const run = async (expression) => {
  const message = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (message.result?.exceptionDetails) {
    return { threw: message.result.exceptionDetails.exception?.description ?? 'error' };
  }
  return message.result?.result?.value;
};

await send('Runtime.enable');
await send('Page.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });

const viewport = (width, height = 800) =>
  send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 2,
    mobile: true,
  });

/** Widths that overflow, with the widest elements, so a failure names the offender. */
const measure = () =>
  run(`(() => {
    const vw = document.documentElement.clientWidth;
    const offenders = [...document.querySelectorAll('*')]
      .filter((el) => el.getBoundingClientRect().right > vw + 1)
      .slice(0, 4)
      .map((el) => (el.className && String(el.className).slice(0, 32)) || el.tagName.toLowerCase());
    return { vw, doc: document.documentElement.scrollWidth, offenders };
  })()`);

const reload = async () => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url: app });
  await wait(4500);
};

const results = [];
const failures = [];

// Signed out first: that is the state a shared link lands in.
await viewport(411);
await reload();
await run(`localStorage.removeItem('altfirst.session')`);
await reload();
for (const width of WIDTHS) {
  await viewport(width);
  await wait(500);
  const state = await measure();
  results.push({ state: 'signed out', width, ...state });
  if (state.doc > state.vw) {
    failures.push(`signed out at ${width}px: page is ${state.doc}px wide (${state.offenders})`);
  }
}

// Signed in: the header carries an account control instead of the badge, so the min-content
// width differs — that is the state that actually broke.
await viewport(411);
await run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'a', refreshJwt: 'b', pds: 'http://127.0.0.1:9'
}))`);
await reload();
for (const width of WIDTHS) {
  await viewport(width);
  await wait(500);
  const state = await measure();
  results.push({ state: 'signed in', width, ...state });
  if (state.doc > state.vw) {
    failures.push(`signed in at ${width}px: page is ${state.doc}px wide (${state.offenders})`);
  }
}

// The phone's only navigation, and the space it needs.
const nav = await run(`(() => {
  const el = document.querySelector('.bottom-nav');
  if (!el) return { exists: false };
  const timeline = document.querySelector('.timeline');
  return {
    exists: true,
    display: getComputedStyle(el).display,
    height: Math.round(el.getBoundingClientRect().height),
    timelinePadBottom: Math.round(parseFloat(getComputedStyle(timeline).paddingBottom)),
    items: [...el.querySelectorAll('a')].map((a) => ({
      label: a.textContent.trim(),
      href: a.getAttribute('href'),
    })),
  };
})()`);
results.push({ state: 'signed in', width: 411, navigation: nav });

if (!nav.exists) failures.push('no bottom navigation exists at all');
if (nav.display === 'none') failures.push('the bottom navigation is hidden at 411px');
const hrefs = (nav.items ?? []).map((item) => item.href);
for (const wanted of ['#/', '#/following', '#/notifications', '#/feeds', '#/compose']) {
  if (!hrefs.includes(wanted)) failures.push(`the bottom navigation has no link to ${wanted}`);
}
if ((nav.timelinePadBottom ?? 0) < (nav.height ?? 0)) {
  failures.push(
    `the column's bottom padding (${nav.timelinePadBottom}) is less than the nav's height (${nav.height})`,
  );
}

// The rail has to come back on a desktop, or the "fix" would have cured the phone by
// flattening the layout everywhere.
await send('Emulation.setDeviceMetricsOverride', {
  width: 1280,
  height: 900,
  deviceScaleFactor: 1,
  mobile: false,
});
await wait(600);
const desktop = await run(`(() => {
  const rail = document.querySelector('.rail');
  const nav = document.querySelector('.bottom-nav');
  return {
    vw: document.documentElement.clientWidth,
    doc: document.documentElement.scrollWidth,
    railVisible: !!rail && getComputedStyle(rail).display !== 'none',
    navDisplay: nav ? getComputedStyle(nav).display : 'missing',
  };
})()`);
await run(`localStorage.removeItem('altfirst.session')`);
await send('Emulation.clearDeviceMetricsOverride');
await send('Page.navigate', { url: app });
ws.close();

if (!desktop.railVisible) failures.push('the rail is not visible at 1280px');
if (desktop.navDisplay !== 'none') {
  failures.push(`the bottom navigation is ${desktop.navDisplay} at 1280px, duplicating the rail`);
}
if (desktop.doc > desktop.vw) {
  failures.push(`desktop at 1280px: page is ${desktop.doc}px wide`);
}

console.log(JSON.stringify({ results, desktop, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;
