import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';

// An expired access token on a *read* must be rotated, not silently downgraded.
//
// The restore path deliberately does not rotate on load — it costs a round trip per reload,
// kills the cached DPoP nonce and races other tabs for a single-use refresh token — and says
// so: "a request that needs a new token still refreshes on its own 401". Feed loads and likes
// kept that promise; reads did not. So a profile opened after the ~2 h token lifetime answered
// `401` on `getAuthorFeed` (to the account's own PDS, not the AppView), fell back to the public
// AppView, and that answers `viewer: null`: every heart read as unliked, and liking one created
// a *second* record.
//
// A stub PDS stands in for the real one: it refuses the stale token, accepts exactly one
// refresh (a second is refused, as a real PDS refuses a spent token), then serves the retried
// reads. The page has to come up showing the stub's own posts — a public fallback would show
// none of them.
//
// Usage: node scripts/check_expired_read.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';
const STALE = 'Bearer stale-access';
const STALE_REFRESH = 'Bearer stale-refresh';
const FRESH = 'Bearer fresh-access';

const fixture = (name) =>
  JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));
const feed = fixture('author_feed.json');
const profile = fixture('profile.json');
// Present in the fixture only: api.bsky.app has never heard of this account.
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
    // Single use, like the real endpoint: the second attempt with the same token is refused.
    return refreshes === 1 && auth === STALE_REFRESH
      ? json(200, {
          accessJwt: 'fresh-access',
          refreshJwt: 'fresh-refresh',
          did: 'did:plc:stub',
          handle: 'stub.test',
        })
      : json(400, { error: 'invalid_grant', message: 'Refresh token already used' });
  }
  // 200 on purpose: the profile page's reads are the subject, not the saved-feeds call.
  if (pathname === '/xrpc/app.bsky.actor.getPreferences') return json(200, { preferences: [] });
  if (pathname === '/xrpc/app.bsky.actor.getProfile') {
    return auth === FRESH ? json(200, profile) : expired();
  }
  if (pathname === '/xrpc/app.bsky.feed.getAuthorFeed') {
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

async function connect() {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      const targets = await (await fetch(`${cdp}/json/list`)).json();
      const page = targets.find((target) => target.type === 'page');
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // not up yet
    }
    await wait(250);
  }
  throw new Error('could not reach Chrome DevTools on :9222');
}

/** Launch a browser if none is already listening, as verify_dom.mjs does. */
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
      // Avoid the macOS "Chrome Safe Storage" keychain prompt, which blocks headless startup.
      '--use-mock-keychain',
      '--password-store=basic',
      '--remote-debugging-port=9222',
      `--user-data-dir=${process.cwd()}/.tmp/chrome-profile`,
      'about:blank',
    ],
    { detached: true, stdio: 'ignore' },
  ).unref();
  await connect();
}

await ensureBrowser();
const ws = new WebSocket(await connect());
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
  if (!message.result) return { error: message.error?.message ?? 'no result' };
  if (message.result.exceptionDetails) {
    return { threw: message.result.exceptionDetails.exception?.description ?? 'error' };
  }
  return message.result.result.value;
};

await send('Runtime.enable');
await send('Page.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });

// Land on the app's own origin first: localStorage is per origin, and the session has to be in
// place before the next load for that load to take the restore path.
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4000);
const original = await run(`localStorage.getItem('altfirst.session')`);

await run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'stale-access', refreshJwt: 'stale-refresh',
  pds: ${JSON.stringify(PDS)}
}))`);

// about:blank first: a hash-only change would not reload, so the restore path would not run,
// and the expired token is exactly what a reload hands to the first read.
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: `${app}#/profile/stub.test` });
for (let attempt = 0; attempt < 40; attempt += 1) {
  if (await run("document.querySelectorAll('.post').length > 0")) break;
  await wait(500);
}

const state = await run(`(() => {
  const raw = localStorage.getItem('altfirst.session');
  const stored = raw ? JSON.parse(raw) : null;
  return {
    posts: document.querySelectorAll('.post').length,
    mentionsMarker: document.body.innerText.includes(${JSON.stringify(marker)}),
    error: document.querySelector('.feed-error')?.innerText ?? null,
    accessJwt: stored?.accessJwt ?? null,
    refreshJwt: stored?.refreshJwt ?? null,
  };
})()`);

// Put the browser back the way it was, so the next check is not signed in as a stub.
await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Page.navigate', { url: app });
ws.close();
stub.close();

const failures = [];
// Two reads on one page can both spend the token before either publishes: the second attempt is
// refused and that read adopts the first one's pair. One wasted request is the accepted cost of
// not carrying a lock — what must never happen is a read downgrading to the public AppView,
// which the marker assertion below catches.
if (refreshes < 1) failures.push('no rotation happened at all');
if (refreshes > 2) failures.push(`rotated ${refreshes} times for two reads`);
if (state.posts === 0) failures.push('no posts rendered');
if (!state.mentionsMarker) {
  failures.push('the post came from somewhere else: a public fallback, not the retried read');
}
if (state.error) failures.push(`error panel shown: ${state.error}`);
if (state.accessJwt !== 'fresh-access') failures.push(`stored access token is ${state.accessJwt}`);
if (state.refreshJwt !== 'fresh-refresh') {
  failures.push(`stored refresh token is ${state.refreshJwt}`);
}
for (const method of ['/xrpc/app.bsky.actor.getProfile', '/xrpc/app.bsky.feed.getAuthorFeed']) {
  if (!calls.includes(`GET ${method} ${FRESH}`)) {
    failures.push(`${method} was never retried with the new token`);
  }
}
if (calls.some((call) => call.endsWith('(no auth)'))) {
  failures.push('an authenticated read went out without a token');
}

console.log('expired-read:', JSON.stringify({ state, refreshes, calls, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;
