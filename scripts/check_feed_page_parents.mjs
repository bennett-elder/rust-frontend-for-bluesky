import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createServer } from 'node:http';

// A reply appended by scrolling must still say what it answers.
//
// `record.reply.parent` is only an at-uri, so the card's "↳ Replying to @x" line comes from a
// `getPosts` lookup that fills `ParentHandles`. Only the first page ever ran that lookup: the
// scroll trigger and the "new posts" pill both set `feed_items` themselves, so page two onward
// drew every reply as a top-level post until a fresh page replaced the list. That is why the
// notes looked intermittent — they were a property of which page a post landed on.
//
// Usage: node scripts/check_feed_page_parents.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';

const author = (handle) => ({ did: `did:plc:${handle.replace(/\W/g, '')}`, handle });
const parent = (name, text) => ({
  uri: `at://did:plc:stub/app.bsky.feed.post/${name}`,
  cid: `bafy${name}`,
  author: author(`${name}.test`),
  record: { $type: 'app.bsky.feed.post', text, createdAt: '2026-01-01T00:00:00.000Z' },
  indexedAt: '2026-01-01T00:00:00.000Z',
});
const reply = (name, text, parentName) => ({
  ...parent(name, text),
  record: {
    $type: 'app.bsky.feed.post',
    text,
    createdAt: '2026-01-01T00:00:00.000Z',
    reply: {
      parent: { uri: `at://did:plc:stub/app.bsky.feed.post/${parentName}`, cid: `bafy${parentName}` },
      root: { uri: `at://did:plc:stub/app.bsky.feed.post/${parentName}`, cid: `bafy${parentName}` },
    },
  },
});

const pageOne = [reply('reply-one', 'First page reply', 'parent-a')];
const pageTwo = [reply('reply-two', 'Second page reply', 'parent-b')];
const parents = [parent('parent-a', 'Parent A'), parent('parent-b', 'Parent B')];

const calls = [];
let parentLookups = 0;
// Flipped for the last phase: the lookup must be allowed to fail.
let parentMode = 'ok';
const stub = createServer((req, res) => {
  const url = new URL(req.url, PDS);
  calls.push(`${req.method} ${url.pathname}${url.search}`);
  const json = (status, body) => {
    res.writeHead(status, {
      'access-control-allow-origin': '*',
      'access-control-allow-headers': '*',
      'access-control-allow-methods': 'GET, POST, OPTIONS',
      'content-type': 'application/json',
    });
    res.end(JSON.stringify(body));
  };
  if (req.method === 'OPTIONS') return json(204, {});
  if (url.pathname === '/xrpc/app.bsky.actor.getPreferences') return json(200, { preferences: [] });
  if (url.pathname === '/xrpc/app.bsky.feed.getTimeline') {
    // Page one carries the cursor; page two ends the feed.
    return url.searchParams.get('cursor') === 'page-2'
      ? json(200, { feed: pageTwo.map((post) => ({ post })) })
      : json(200, { feed: pageOne.map((post) => ({ post })), cursor: 'page-2' });
  }
  if (url.pathname === '/xrpc/app.bsky.feed.getPosts') return json(200, { posts: parents });
  json(404, { error: 'NotFound', message: url.pathname });
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
  // Parent handles are looked up on the public AppView, not on the account's PDS, so the stub
  // cannot serve them: those requests are answered here instead.
  if (message.method === 'Fetch.requestPaused') {
    parentLookups += 1;
    if (parentMode === 'fail') {
      send('Fetch.fulfillRequest', {
        requestId: message.params.requestId,
        responseCode: 500,
        responseHeaders: [{ name: 'access-control-allow-origin', value: '*' }],
        body: '',
      });
      return;
    }
    send('Fetch.fulfillRequest', {
      requestId: message.params.requestId,
      responseCode: 200,
      responseHeaders: [
        { name: 'content-type', value: 'application/json' },
        { name: 'access-control-allow-origin', value: '*' },
      ],
      body: Buffer.from(JSON.stringify({ posts: parents })).toString('base64'),
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
  if (message.result?.exceptionDetails) {
    return { threw: message.result.exceptionDetails.exception?.description ?? 'error' };
  }
  return message.result?.result?.value;
};

await send('Runtime.enable');
await send('Page.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
// The parent lookup goes to api.bsky.app; answer it with the two parents this feed refers to.
await send('Fetch.enable', {
  patterns: [{ urlPattern: '*api.bsky.app/xrpc/app.bsky.feed.getPosts*' }],
});

// Boot signed out, park the session, reload so the app restores it, then move to Following
// without a reload — a reload into Following also races the restore (BUGS.md #10).
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);
const original = await run(`localStorage.getItem('altfirst.session')`);
await run(`localStorage.setItem('altfirst.session', JSON.stringify({
  handle: 'stub.test', did: 'did:plc:stub',
  accessJwt: 'live-access', refreshJwt: 'live-refresh',
  pds: ${JSON.stringify(PDS)}
}))`);
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app });
await wait(4500);
await run(`location.hash = '#/following'`);

for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run(`document.querySelectorAll('.post').length >= ${pageOne.length}`)) break;
  await wait(400);
}
const notesOnFirstPage = await run(
  `[...document.querySelectorAll('.post__reply-context')].map((node) => node.textContent.trim())`,
);

// Page two, through the same trigger endless scroll uses.
await run(`document.querySelector('.load-more--auto').click()`);
for (let attempt = 0; attempt < 30; attempt += 1) {
  const count = await run(`document.querySelectorAll('.post').length`);
  if (count >= pageOne.length + pageTwo.length) break;
  await wait(400);
}
// `remember_parents` is one round trip after the page lands.
let notesAfterScroll = [];
for (let attempt = 0; attempt < 20; attempt += 1) {
  notesAfterScroll = await run(
    `[...document.querySelectorAll('.post__reply-context')].map((node) => node.textContent.trim())`,
  );
  if (notesAfterScroll.some((note) => note.includes('parent-b.test'))) break;
  await wait(300);
}

// A lookup that never resolves must still leave the card saying it is a reply. This is the
// reported symptom: the line vanished whenever the parent lookup had not landed, leaving the
// reply indistinguishable from a top-level post.
parentMode = 'fail';
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: app + '#/following' });
for (let attempt = 0; attempt < 40; attempt += 1) {
  if (await run(`document.querySelectorAll('.post').length > 0`)) break;
  await wait(500);
}
await wait(1500);
const notesWithoutLookup = await run(
  `[...document.querySelectorAll('.post__reply-context')].map((node) => node.textContent.trim())`,
);

await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Page.navigate', { url: app });
ws.close();
stub.close();

const failures = [];
if (!notesOnFirstPage.some((note) => note.includes('parent-a.test'))) {
  failures.push(`page one lost its note: ${JSON.stringify(notesOnFirstPage)}`);
}
if (!notesAfterScroll.some((note) => note.includes('parent-b.test'))) {
  failures.push(`the appended page has no note: ${JSON.stringify(notesAfterScroll)}`);
}
if (!notesWithoutLookup.some((note) => note.includes('Replying to a post'))) {
  failures.push(
    `a failed parent lookup left no note at all: ${JSON.stringify(notesWithoutLookup)}`,
  );
}
if (parentLookups === 0) {
  failures.push('no parent lookup ran at all');
}

console.log(
  'feed-page-parents:',
  JSON.stringify(
    { notesOnFirstPage, notesAfterScroll, notesWithoutLookup, parentLookups, calls, failures },
    null,
    2,
  ),
);
if (failures.length > 0) process.exitCode = 1;
