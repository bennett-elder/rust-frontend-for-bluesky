import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createServer } from 'node:http';

// The feed strip must be usable with a mouse, not just a fingertip.
//
// It is a horizontal scroller with its scrollbar hidden: on a touchscreen you swipe, and with a
// mouse there was nothing to grab, so anything past the right edge was reachable only by
// tabbing. This drags the strip with real CDP mouse input and asserts it moved — and that the
// drag did not also open the tab it ended on — then walks the row with the arrow keys.
//
// Usage: node scripts/check_feed_strip_drag.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';

// Five saved feeds: a real account has some, and they are what makes the strip wider than a
// phone. Without them three tabs fit at 411 px and there is nothing to drag.
const FEEDS = [
  { uri: 'at://did:plc:stub/app.bsky.feed.generator/alt-text-pals', name: 'Alt Text Pals' },
  { uri: 'at://did:plc:stub/app.bsky.feed.generator/described-media', name: 'Described Media' },
  { uri: 'at://did:plc:stub/app.bsky.feed.generator/design-systems', name: 'Design Systems' },
  { uri: 'at://did:plc:stub/app.bsky.feed.generator/rust-lang', name: 'Rust' },
  { uri: 'at://did:plc:stub/app.bsky.feed.generator/mutuals', name: 'Mutuals' },
];

const stub = createServer((req, res) => {
  const { pathname } = new URL(req.url, PDS);
  const cors = {
    'access-control-allow-origin': '*',
    'access-control-allow-headers': '*',
    'access-control-allow-methods': 'GET, POST, OPTIONS',
  };
  if (req.method === 'OPTIONS') {
    res.writeHead(204, cors);
    res.end();
    return;
  }
  const json = (body) => {
    res.writeHead(200, { ...cors, 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (pathname === '/xrpc/app.bsky.actor.getPreferences') {
    return json({
      preferences: [
        {
          $type: 'app.bsky.actor.defs#savedFeedsPrefV2',
          items: FEEDS.map((feed, index) => ({
            id: `saved-${index}`,
            type: 'feed',
            value: feed.uri,
            pinned: false,
          })),
        },
      ],
    });
  }
  json({ preferences: [] });
});
await new Promise((resolve) => stub.listen(8199, '127.0.0.1', resolve));

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
    return;
  }
  // Feed labels come from the public AppView, so the stub cannot serve them.
  if (message.method === 'Fetch.requestPaused') {
    send('Fetch.fulfillRequest', {
      requestId: message.params.requestId,
      responseCode: 200,
      responseHeaders: [
        { name: 'content-type', value: 'application/json' },
        { name: 'access-control-allow-origin', value: '*' },
      ],
      body: Buffer.from(
        JSON.stringify({
          feeds: FEEDS.map((feed) => ({ uri: feed.uri, displayName: feed.name })),
        }),
      ).toString('base64'),
    });
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
  return message.result?.result?.value;
};

await send('Runtime.enable');
await send('Page.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Fetch.enable', { patterns: [{ urlPattern: '*getFeedGenerators*' }] });
// 411 px: a real phone, and the width this was reported at.
await send('Emulation.setDeviceMetricsOverride', {
  width: 411,
  height: 788,
  deviceScaleFactor: 2,
  mobile: true,
});

await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);
const original = await run(`localStorage.getItem('altfirst.session')`);
await run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'a', refreshJwt: 'b', pds: ${JSON.stringify(PDS)}
}))`);
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);

const strip = await run(`(() => {
  const el = document.querySelector('.feed-tabs');
  if (!el) return { exists: false };
  const rect = el.getBoundingClientRect();
  return {
    exists: true,
    scrollLeft: el.scrollLeft,
    scrollWidth: el.scrollWidth,
    clientWidth: el.clientWidth,
    x: Math.round(rect.left + rect.width * 0.75),
    y: Math.round(rect.top + rect.height / 2),
    tabs: el.children.length,
    hash: location.hash,
  };
})()`);

const failures = [];
if (!strip.exists) failures.push('no feed tab strip rendered');
if (strip.exists && strip.scrollWidth <= strip.clientWidth + 1) {
  failures.push('the strip does not overflow at 411px, so there is nothing to drag');
}

// A real mouse drag: press, move left in steps, release.
if (strip.exists) {
  await send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: strip.x,
    y: strip.y,
    button: 'left',
    clickCount: 1,
  });
  for (const step of [20, 40, 60, 80, 100, 120]) {
    await send('Input.dispatchMouseEvent', {
      type: 'mouseMoved',
      x: strip.x - step,
      y: strip.y,
      button: 'left',
      buttons: 1,
    });
    await wait(16);
  }
  await send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: strip.x - 120,
    y: strip.y,
    button: 'left',
    clickCount: 1,
  });
}
await wait(400);

const afterDrag = await run(`(() => {
  const el = document.querySelector('.feed-tabs');
  return { scrollLeft: el.scrollLeft, hash: location.hash };
})()`);
if ((afterDrag.scrollLeft ?? 0) < 90) {
  failures.push(`dragging moved the strip to ${afterDrag.scrollLeft}px, expected ~120`);
}
if (afterDrag.hash !== strip.hash) {
  failures.push(`the drag navigated to ${afterDrag.hash}; a drag must not open a tab`);
}

// Arrow keys walk the row, so a keyboard can reach the tabs that are off-screen.
await run(`document.querySelector('.feed-tabs > *').focus()`);
const before = await run(`document.activeElement?.textContent?.trim()`);
await send('Input.dispatchKeyEvent', {
  type: 'keyDown',
  key: 'ArrowRight',
  code: 'ArrowRight',
  windowsVirtualKeyCode: 39,
});
await send('Input.dispatchKeyEvent', {
  type: 'keyUp',
  key: 'ArrowRight',
  code: 'ArrowRight',
  windowsVirtualKeyCode: 39,
});
await wait(200);
const after = await run(`document.activeElement?.textContent?.trim()`);
if (!before || before === after) {
  failures.push(`ArrowRight did not move focus (stayed on ${before})`);
}

await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Emulation.clearDeviceMetricsOverride');
await send('Page.navigate', { url: app });
ws.close();
stub.close();

console.log(
  JSON.stringify({ strip, afterDrag, focus: { before, after }, failures }, null, 2),
);
if (failures.length > 0) process.exitCode = 1;
