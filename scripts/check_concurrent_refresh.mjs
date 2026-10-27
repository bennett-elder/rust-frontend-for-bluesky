import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';

// Two tabs, one single-use refresh token.
//
// Refresh tokens are single-use: if both tabs rotate at the same moment, one is refused. Its
// session is not dead — the winner's pair is in storage — but the loser only finds that out by
// being told. Before this, the refused tab called `forget_session` and silently signed the
// reader out (and its `localStorage.removeItem` could undo the winner's write).
//
// Both tabs here load Following, because that is the path that signs out on a dead grant;
// a profile read never did. The stub accepts exactly one refresh and refuses the rest, like a
// real PDS. Both tabs must end up showing the stub's posts with one live session between them.
//
// Usage: node scripts/check_concurrent_refresh.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';
const FRESH = 'Bearer fresh-access';
const STALE_REFRESH = 'Bearer stale-refresh';

const feed = JSON.parse(
  readFileSync(new URL('./fixtures/author_feed.json', import.meta.url), 'utf8'),
);
const marker = 'replier.test';

const calls = [];
let refreshes = 0;

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
  const auth = req.headers.authorization ?? '';
  calls.push(`${req.method} ${pathname} ${auth || '(no auth)'}`);
  const json = (status, body) => {
    res.writeHead(status, { ...cors, 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  };
  const expired = () => json(401, { error: 'ExpiredToken', message: 'Token has expired' });

  if (pathname === '/xrpc/com.atproto.server.refreshSession') {
    refreshes += 1;
    return refreshes === 1 && auth === STALE_REFRESH
      ? json(200, {
          accessJwt: 'fresh-access',
          refreshJwt: 'fresh-refresh',
          did: 'did:plc:stub',
          handle: 'stub.test',
        })
      : json(400, { error: 'invalid_grant', message: 'Refresh token already used' });
  }
  if (pathname === '/xrpc/app.bsky.actor.getPreferences') return json(200, { preferences: [] });
  if (pathname === '/xrpc/app.bsky.feed.getTimeline') {
    return auth === FRESH ? json(200, feed) : expired();
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

async function connect(target) {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
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
  return { send, run, close: () => ws.close() };
}

await ensureBrowser();

const pages = (await list()).filter((target) => target.type === 'page');
const a = await connect(pages[0]);
const boot = async (tab) => {
  await tab.send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await tab.send('Page.navigate', { url: app });
  await wait(5000);
};

await boot(a);
const original = await a.run(`localStorage.getItem('altfirst.session')`);
await a.run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'stale-access', refreshJwt: 'stale-refresh',
  pds: ${JSON.stringify(PDS)}
}))`);

// Reload so this tab restores the stub session, then a second tab of the same origin.
await boot(a);
const created = await a.send('Target.createTarget', { url: 'about:blank' });
let second = null;
for (let attempt = 0; attempt < 20 && !second; attempt += 1) {
  second = (await list()).find((target) => target.id === created.result?.targetId);
  if (!second) await wait(250);
}
if (!second) throw new Error('could not open the second tab');
const b = await connect(second);
await boot(b);

// Switch both to Following by hash, so neither reloads: each tab already holds the same stale
// session in memory, and both meet the 401 within milliseconds of each other. (A reload would
// add the first-paint "not signed in yet" race, which is a different bug.)
await a.run(`location.hash = '#/following'`);
await b.run(`location.hash = '#/following'`);

const read = (tab) => tab.run(`(() => {
  const raw = localStorage.getItem('altfirst.session');
  return {
    posts: document.querySelectorAll('.post').length,
    marker: document.body.innerText.includes(${JSON.stringify(marker)}),
    error: document.querySelector('.feed-error')?.innerText ?? null,
    accessJwt: raw ? JSON.parse(raw).accessJwt : null,
  };
})()`);

const state = { a: null, b: null };
for (let attempt = 0; attempt < 50; attempt += 1) {
  state.a = await read(a);
  state.b = await read(b);
  const ready = (s) => s.posts > 0 || s.error;
  if (ready(state.a) && ready(state.b)) break;
  await wait(500);
}

await a.run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await a.send('Target.closeTarget', { targetId: second.id });
await a.send('Page.navigate', { url: app });
a.close();
b.close();
stub.close();

const failures = [];
// One or two: the loser may adopt from storage before it spends the token, or be refused and
// adopt after. Both are correct; what must not happen is a third attempt or a sign-out.
if (refreshes < 1) failures.push('no rotation happened at all');
if (refreshes > 2) failures.push(`${refreshes} rotations for two tabs`);
for (const [name, tab] of Object.entries(state)) {
  if (!tab || tab.posts === 0) failures.push(`tab ${name} rendered no posts`);
  if (!tab?.marker) failures.push(`tab ${name} did not render the stub's posts`);
  if (tab?.error) failures.push(`tab ${name} showed an error panel: ${tab.error}`);
  if (tab?.accessJwt !== 'fresh-access') {
    failures.push(`tab ${name} ended with access token ${tab?.accessJwt}`);
  }
}

console.log('concurrent-refresh:', JSON.stringify({ state, refreshes, calls, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;

