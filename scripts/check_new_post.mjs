import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';

// A new post is a `createRecord` with no `reply` refs — and nothing else.
//
// The client could only ever write by replying, so the record body was never exercised without
// a parent. This drives the compose page against a stub PDS and inspects what it actually sent:
// the collection, the record type, the text, `createdAt`, `repo`, and the absence of `reply`.
// It also checks where posting lands you, because a feed would not show the post yet.
//
// Usage: node scripts/check_new_post.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';
const POST_TEXT =
  'Sharing https://bennett-elder.github.io/rust-frontend-for-bluesky/ with @someone.test #alttest';
const LINK = 'https://bennett-elder.github.io/rust-frontend-for-bluesky/';
const MENTION = '@someone.test';
const MENTION_DID = 'did:plc:mentioned';
const TAG = '#alttest';
const TAG_NAME = 'alttest';
// Flipped before the second post, to prove an unresolvable handle stays plain text.
let resolveMode = 'ok';

const fixture = (name) =>
  JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));
const profile = fixture('profile.json');
const feed = fixture('author_feed.json');

const creates = [];
const stub = createServer((req, res) => {
  const url = new URL(req.url, PDS);
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
  if (url.pathname === '/xrpc/com.atproto.repo.createRecord') {
    let body = '';
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      creates.push(body);
      json(200, {
        uri: 'at://did:plc:stub/app.bsky.feed.post/new-post',
        cid: 'bafynewpost',
      });
    });
    return;
  }
  if (url.pathname === '/xrpc/app.bsky.actor.getPreferences') return json(200, { preferences: [] });
  if (url.pathname === '/xrpc/app.bsky.actor.getProfile') return json(200, profile);
  if (url.pathname === '/xrpc/app.bsky.feed.getAuthorFeed') return json(200, feed);
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
  // Handle resolution goes to the public AppView, so the stub cannot answer it.
  if (message.method === 'Fetch.requestPaused') {
    if (resolveMode === 'fail') {
      send('Fetch.fulfillRequest', {
        requestId: message.params.requestId,
        responseCode: 400,
        responseHeaders: [
          { name: 'content-type', value: 'application/json' },
          { name: 'access-control-allow-origin', value: '*' },
        ],
        body: Buffer.from(
          JSON.stringify({ error: 'InvalidRequest', message: 'Unable to resolve handle' }),
        ).toString('base64'),
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
      body: Buffer.from(JSON.stringify({ did: MENTION_DID })).toString('base64'),
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
await send('Fetch.enable', { patterns: [{ urlPattern: '*resolveHandle*' }] });

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
await run(`location.hash = '#/compose'`);
await wait(1500);

// Dioxus reads the DOM value on `input`, so set it the way a browser does and fire the event.
const typed = await run(`(() => {
  const area = document.querySelector('textarea');
  if (!area) return { threw: 'no compose textarea' };
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  setter.call(area, ${JSON.stringify(POST_TEXT)});
  area.dispatchEvent(new Event('input', { bubbles: true }));
  return { value: area.value };
})()`);
// Dioxus re-renders on its own tick, so the counter and the button are read after it.
await wait(500);
typed.value = await run(`document.querySelector('textarea').value`);
typed.count = await run(`document.querySelector('.reply-box__count').textContent`);
const enabled = await run(`!document.querySelector('.reply-box__send').disabled`);
await run(`document.querySelector('.reply-box__send').click()`);

for (let attempt = 0; attempt < 30 && creates.length === 0; attempt += 1) await wait(300);
await wait(800);
const landed = await run(`location.hash`);

// Second post, with the resolver failing: the mention must stay plain text.
resolveMode = 'fail';
await run(`location.hash = '#/compose'`);
await wait(1500);
await run(`(() => {
  const area = document.querySelector('textarea');
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
  setter.call(area, '@nobody.test hello');
  area.dispatchEvent(new Event('input', { bubbles: true }));
})()`);
await wait(600);
await run(`document.querySelector('.reply-box__send').click()`);
for (let attempt = 0; attempt < 30 && creates.length < 2; attempt += 1) await wait(300);
await wait(600);
const second = creates[1] ? JSON.parse(creates[1]) : {};

await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Page.navigate', { url: app });
ws.close();
stub.close();

const failures = [];
if (creates.length !== 2) failures.push(`expected 2 createRecord calls, saw ${creates.length}`);
const body = creates.length > 0 ? JSON.parse(creates[0]) : {};
if (body.repo !== 'did:plc:stub') failures.push(`repo is ${body.repo}`);
if (body.collection !== 'app.bsky.feed.post') failures.push(`collection is ${body.collection}`);
const record = body.record ?? {};
if (record.$type !== 'app.bsky.feed.post') failures.push(`$type is ${record.$type}`);
if (record.text !== POST_TEXT) failures.push(`text is ${JSON.stringify(record.text)}`);
if (!record.createdAt) failures.push('no createdAt');
if ('reply' in record) failures.push('a top-level post carried reply refs');

// Two facets: the link, and the mention that had to be resolved to a DID.
const facets = record.facets ?? [];
if (facets.length !== 3) failures.push(`expected 3 facets, saw ${facets.length}`);
const linkFacet = facets.find((f) => f.features?.[0]?.$type === 'app.bsky.richtext.facet#link');
const mentionFacet = facets.find(
  (f) => f.features?.[0]?.$type === 'app.bsky.richtext.facet#mention',
);
const tagFacet = facets.find((f) => f.features?.[0]?.$type === 'app.bsky.richtext.facet#tag');

const linkStart = POST_TEXT.indexOf(LINK);
if (linkFacet?.index?.byteStart !== linkStart) {
  failures.push(`link facet starts at ${linkFacet?.index?.byteStart}, expected ${linkStart}`);
}
if (linkFacet?.index?.byteEnd !== linkStart + LINK.length) {
  failures.push(`link facet ends at ${linkFacet?.index?.byteEnd}`);
}
if (linkFacet?.features?.[0]?.uri !== LINK) {
  failures.push(`link facet uri is ${linkFacet?.features?.[0]?.uri}`);
}

const mentionStart = POST_TEXT.indexOf(MENTION);
if (mentionFacet?.index?.byteStart !== mentionStart) {
  failures.push(
    `mention facet starts at ${mentionFacet?.index?.byteStart}, expected ${mentionStart}`,
  );
}
if (mentionFacet?.index?.byteEnd !== mentionStart + MENTION.length) {
  failures.push(`mention facet ends at ${mentionFacet?.index?.byteEnd}`);
}
if (mentionFacet?.features?.[0]?.did !== MENTION_DID) {
  failures.push(`mention did is ${JSON.stringify(mentionFacet?.features?.[0]?.did)}`);
}

const tagStart = POST_TEXT.indexOf(TAG);
if (tagFacet?.index?.byteStart !== tagStart) {
  failures.push(`tag facet starts at ${tagFacet?.index?.byteStart}, expected ${tagStart}`);
}
if (tagFacet?.index?.byteEnd !== tagStart + TAG.length) {
  failures.push(`tag facet ends at ${tagFacet?.index?.byteEnd}`);
}
if (tagFacet?.features?.[0]?.tag !== TAG_NAME) {
  failures.push(`tag facet carries ${JSON.stringify(tagFacet?.features?.[0]?.tag)}`);
}

// And the fallback: a handle that does not resolve leaves the text alone.
if (second.record?.text !== '@nobody.test hello') {
  failures.push(`second post text is ${JSON.stringify(second.record?.text)}`);
}
if ((second.record?.facets ?? []).length !== 0) {
  failures.push(`an unresolvable handle produced ${second.record.facets.length} facet(s)`);
}
if (!enabled) failures.push('the Post button was not enabled with text in the box');
if (typed.count !== `${POST_TEXT.length}/300`) failures.push(`counter showed ${typed.count}`);
if (!String(landed).startsWith('#/profile/stub.test')) {
  failures.push(`landed on ${landed}, not the author's profile`);
}

console.log('new-post:', JSON.stringify({ typed, enabled, body, landed, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;
