import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createServer } from 'node:http';

// A mention must open the account *here*, not a bsky.app tab.
//
// Mentions were rendered as `https://bsky.app/profile/<did>` with `target="_blank"`, so every
// "who is this?" click left the client — the one class of link that is unambiguously internal.
// The same applies to a link facet that is itself a bsky.app profile URL. Ordinary links still
// go out in a new tab; that is what they are for.
//
// Usage: node scripts/check_mention_navigation.mjs [app-url]
const app = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const PDS = 'http://127.0.0.1:8199';

const fixture = (name) =>
  JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));
const thread = fixture('thread.json');
const profile = fixture('profile.json');
const feed = fixture('author_feed.json');

// Extend the fixture's own text with a bsky.app profile link, so the remap is exercised
// without editing a fixture other checks share. ASCII text, so byte offsets are char offsets.
const LINK_URL = 'https://bsky.app/profile/bluesky.test';
const EXTRA = ` and ${LINK_URL}`;
const record = thread.thread.post.record;
const base = record.text;
record.text = base + EXTRA;
const linkStart = base.length + ' and '.length;
record.facets = [
  ...(record.facets ?? []),
  {
    index: { byteStart: linkStart, byteEnd: linkStart + LINK_URL.length },
    features: [{ $type: 'app.bsky.richtext.facet#link', uri: LINK_URL }],
  },
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
  if (pathname === '/xrpc/app.bsky.feed.getPostThread') return json(thread);
  if (pathname === '/xrpc/app.bsky.actor.getProfile') return json(profile);
  if (pathname === '/xrpc/app.bsky.feed.getAuthorFeed') return json(feed);
  if (pathname === '/xrpc/app.bsky.actor.getPreferences') return json({ preferences: [] });
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
const root = encodeURIComponent(thread.thread.post.uri);
await send('Page.navigate', { url: `${app}#/post/${root}` });

for (let attempt = 0; attempt < 40; attempt += 1) {
  if (await run(`document.querySelectorAll('.facet--mention').length > 0`)) break;
  await wait(500);
}

const anchors = await run(`[...document.querySelectorAll('.post__text a')].map((a) => ({
  text: a.textContent.trim(),
  href: a.getAttribute('href'),
  target: a.getAttribute('target'),
  cls: a.className,
}))`);

const mention = anchors.find((a) => a.cls.includes('facet--mention'));
const appProfile = anchors.find((a) => a.text.includes('bsky.app/profile/bluesky.test'));
const external = anchors.find((a) => a.text.includes('example.com/root'));

const failures = [];
if (!mention) failures.push('no mention anchor rendered');
if (mention && !mention.href.startsWith('#/profile/')) {
  failures.push(`the mention links to ${mention.href}, leaving the app`);
}
if (mention && mention.target) failures.push(`the mention still opens a ${mention.target} tab`);
if (!appProfile) failures.push('the bsky.app profile link did not render');
if (appProfile && !appProfile.href.startsWith('#/profile/')) {
  failures.push(`a bsky.app profile link goes to ${appProfile.href} instead of the profile page`);
}
if (!external) failures.push('the ordinary link did not render');
if (external && (external.target !== '_blank' || !external.href.startsWith('https://'))) {
  failures.push(`the ordinary link became ${external.href} (target ${external.target})`);
}

// Click the mention: the route must change in place, on this origin.
await run(`document.querySelector('.facet--mention').click()`);
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (String(await run(`location.hash`)).startsWith('#/profile/')) break;
  await wait(300);
}
const after = await run(`({ hash: location.hash, origin: location.origin, profile: !!document.querySelector('.profile') })`);
if (!String(after.hash).startsWith('#/profile/')) {
  failures.push(`clicking the mention went to ${after.hash}`);
}
if (!decodeURIComponent(String(after.hash)).includes('did:plc:examplereplier')) {
  failures.push(`the profile route lost the DID: ${after.hash}`);
}

await run(
  original === null
    ? `localStorage.removeItem('altfirst.session')`
    : `localStorage.setItem('altfirst.session', ${JSON.stringify(original)})`,
);
await send('Page.navigate', { url: app });
ws.close();
stub.close();

console.log(JSON.stringify({ anchors, after, failures }, null, 2));
if (failures.length > 0) process.exitCode = 1;
