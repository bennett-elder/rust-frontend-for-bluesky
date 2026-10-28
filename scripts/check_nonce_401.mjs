import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createServer } from 'node:http';

// A DPoP nonce challenge is not an expired token.
//
// A PDS answers a stale cached nonce with `401 use_dpop_nonce` and a fresh `DPoP-Nonce`, which
// `request_text` retries by itself — so what reaches the app is the *retry's* result. Rotation
// used to be keyed on "HTTP 401", which meant a nonce or proof problem could spend a single-use
// refresh token, and a profile page's two parallel reads could spend it twice. This holds the
// PDS to a nonce rejection the retry cannot fix, and requires that no rotation happens.
//
// Usage: node scripts/check_nonce_401.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';

let refreshes = 0;
let reads = 0;

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
  const json = (status, body) => {
    res.writeHead(status, { ...cors, 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  if (pathname === '/xrpc/com.atproto.server.refreshSession') {
    refreshes += 1;
    return json(200, {
      accessJwt: 'fresh-access',
      refreshJwt: 'fresh-refresh',
      did: 'did:plc:stub',
      handle: 'stub.test',
    });
  }
  if (pathname === '/xrpc/app.bsky.actor.getPreferences') return json(200, { preferences: [] });
  if (pathname.startsWith('/xrpc/app.bsky.')) {
    reads += 1;
    // No `DPoP-Nonce` header: the app cannot retry past this, so it is what the read sees.
    return json(401, { error: 'use_dpop_nonce', message: 'DPoP "nonce" mismatch' });
  }
  json(404, { error: 'NotFound', message: pathname });
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

await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);
const original = await run(`localStorage.getItem('altfirst.session')`);
await run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'stale-access', refreshJwt: 'stale-refresh',
  pds: ${JSON.stringify(PDS)}
}))`);
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);
await run(`location.hash = '#/profile/stub.test'`);
await wait(6000);

const stored = await run(`localStorage.getItem('altfirst.session')`);
await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Page.navigate', { url: app });
ws.close();
stub.close();

const failures = [];
if (reads === 0) failures.push('the stub saw no read at all, so nothing was tested');
if (refreshes !== 0) failures.push(`a nonce challenge rotated the token ${refreshes} time(s)`);
if (stored && JSON.parse(stored).accessJwt !== 'stale-access') {
  failures.push(`the stored token changed to ${JSON.parse(stored).accessJwt}`);
}

console.log('nonce-401:', JSON.stringify({ reads, refreshes, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;
