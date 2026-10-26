// Every reply in a list must carry "↳ Replying to @handle".
//
// The lookup is batched 25 uris per getPosts call against a 50-post page, so the
// interesting failure is the one that shipped once: replies past the 25th parent
// rendered as if they were top-level posts. Asserting coverage (not just "some card
// has a label") is what catches that.
//
// Usage: node scripts/check_reply_context.mjs [feed-uri]
const base = 'http://127.0.0.1:8099/';
const feed =
  process.argv[2] ?? 'at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot';
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
  const message = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  if (message.result?.exceptionDetails) {
    throw new Error(message.result.exceptionDetails.exception?.description ?? 'evaluate failed');
  }
  return message.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: `${base}#/feed/${feed}` });
// The parent lookup is a follow-up request after the page lands.
await wait(6000);

const readCards = () => run(`(() => {
  const decode = (link) => {
    let raw = link.slice(link.indexOf('#/post/') + '#/post/'.length);
    raw = raw.split('?')[0];
    let uri = raw;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try { uri = decodeURIComponent(uri); } catch { break; }
    }
    return uri;
  };
  return [...document.querySelectorAll('article.post[data-post-link]')]
    .map((element) => element.getAttribute('data-post-link'))
    .filter((link) => link && link.includes('/post/'))
    .map((link) => ({ uri: decode(link), link }));
})()`);

const readLabels = () => run(`(() => {
  const decode = (link) => {
    let raw = link.slice(link.indexOf('#/post/') + '#/post/'.length);
    raw = raw.split('?')[0];
    let uri = raw;
    for (let attempt = 0; attempt < 2; attempt += 1) {
      try { uri = decodeURIComponent(uri); } catch { break; }
    }
    return uri;
  };
  const out = {};
  for (const element of document.querySelectorAll('article.post[data-post-link]')) {
    const link = element.getAttribute('data-post-link');
    if (!link || !link.includes('/post/')) continue;
    const line = element.querySelector('.post__reply-context');
    out[decode(link)] = {
      text: line ? line.textContent.trim() : null,
      author: element.querySelector('.post__meta')?.textContent.trim() ?? '',
      preview: (element.querySelector('.post__text')?.textContent ?? '').trim().slice(0, 40),
    };
  }
  return out;
})()`);

// Which of these are actually replies, per the AppView.
const replyStatusFor = async (uris) => {
  const replyUris = new Set();
  for (let index = 0; index < uris.length; index += 25) {
    const chunk = uris.slice(index, index + 25);
    const query = chunk.map((uri) => 'uris=' + encodeURIComponent(uri)).join('&');
    const response = await fetch(`https://api.bsky.app/xrpc/app.bsky.feed.getPosts?${query}`);
    const payload = await response.json();
    for (const post of payload.posts ?? []) {
      if (post.record?.reply) replyUris.add(post.uri);
    }
  }
  return replyUris;
};

// Sample surfaces until one actually contains replies: a check that can pass on a
// page with no replies proves nothing, so "found no replies anywhere" is a failure.
const sampleRoutes = [`#/feed/${feed}`, '#/profile/bsky.app', '#/profile/jay.bsky.team'];
let cards = [];
let labelled = {};
let replyUris = new Set();
let used = null;
for (const route of sampleRoutes) {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url: base + route });
  // The parent lookup is a follow-up request after the page lands.
  await wait(6000);
  cards = await readCards();
  labelled = await readLabels();
  replyUris = await replyStatusFor(cards.map((card) => card.uri));
  used = route;
  if (replyUris.size > 0) break;
}

const missing = [];
const stray = [];
const seen = new Set();
for (const card of cards) {
  if (seen.has(card.uri)) continue;
  seen.add(card.uri);
  const found = labelled[card.uri];
  const isReply = replyUris.has(card.uri);
  if (isReply && !found?.text) {
    missing.push({ uri: card.uri, author: found?.author, preview: found?.preview });
  }
  if (!isReply && found?.text) {
    stray.push({ uri: card.uri, text: found.text });
  }
}

const report = {
  route: used,
  cards: seen.size,
  replies: [...seen].filter((uri) => replyUris.has(uri)).length,
  labelled_cards: Object.values(labelled).filter((entry) => entry.text).length,
  missing,
  stray,
  pass: missing.length === 0 && stray.length === 0 && seen.size > 0 && replyUris.size > 0,
};

// Thread nodes must NOT carry the line: their nesting already says what they answer,
// and the depth-capped levels get their own "replying to" marker instead.
const permalink = await run(`(() => {
  const link = document.querySelector('article.post a.post__permalink');
  return link ? link.getAttribute('href') : null;
})()`);
if (typeof permalink === 'string' && permalink.includes('/post/')) {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url: base + permalink });
  await wait(5000);
  const threadLabels = await run(`document.querySelectorAll('article.post .post__reply-context').length`);
  report.thread_labels = threadLabels;
  if (threadLabels !== 0) report.pass = false;
} else {
  report.thread_labels = 'not sampled';
  report.pass = false;
}

if (replyUris.size === 0) {
  report.reason = `no replies on any sampled route (${sampleRoutes.join(', ')}) — nothing was proven`;
}
console.log('reply context:', JSON.stringify(report, null, 2));
ws.close();
process.exit(report.pass ? 0 : 1);

