// The counts on a post should open who is behind them. This opens the likes,
// reposts and quotes pages for a real post and asserts each renders its list with
// the right heading, title and back control.
//
// Usage: node scripts/check_engagement_pages.mjs [base-url] [post-uri]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
// No baked default: supply a post that has likes, reposts and quotes.
const postUri = process.argv[3];
if (!postUri) {
  console.error('usage: node scripts/check_engagement_pages.mjs <base-url> <post-uri>');
  process.exit(2);
}
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
  const message = await send('Runtime.evaluate', { expression, returnByValue: true });
  if (message.result?.exceptionDetails) return { threw: message.result.exceptionDetails.exception?.description };
  return message.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const click = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

const visit = async (suffix) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url: `${base}#/post/${encodeURIComponent(postUri)}/${suffix}` });
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const ready = await run(
      "Boolean(document.querySelector('.actor-list, .post, .note'))"
    );
    if (ready) break;
    await wait(500);
  }
  await wait(1200);
  return run(`(() => ({
    hash: location.hash,
    title: document.title,
    heading: document.querySelector('.page-title')?.innerText ?? null,
    actors: document.querySelectorAll('.actor-list__item').length,
    firstActor: document.querySelector('.actor__handle')?.innerText?.trim() ?? null,
    posts: document.querySelectorAll('.post').length,
    back: Boolean(document.querySelector('.thread__back')),
    note: document.querySelector('.engagement .note')?.innerText ?? null,
  }))()`);
};

const likes = await visit('likes');
const reposts = await visit('reposts');
const quotes = await visit('quotes');

// The counts on the card have to be the way in, or the pages are unreachable.
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: `${base}#/post/${encodeURIComponent(postUri)}` });
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("document.querySelectorAll('.thread-node .post').length > 0")) break;
  await wait(500);
}
await wait(1200);
// The way in is the "Liked by / Reposted by / N quotes" line on the post you opened
// (above the action icons, as in the official app) — never on feed cards.
const lines = await run(`(() => {
  const card = document.querySelector('.thread-node--focal .post');
  const line = card?.querySelector('.post__engagement');
  const actions = card?.querySelector('.post__actions');
  return {
    linesOnPage: document.querySelectorAll('.post__engagement').length,
    // Links *and* plain-text slots: zero counts are not links, and they still occupy
    // a slot. Separators are excluded.
    text: [...(line?.querySelectorAll('.post__engagement-link, .post__engagement-count') ?? [])]
      .map((node) => node.textContent.trim()),
    hrefs: [...(line?.querySelectorAll('.post__engagement-link') ?? [])].map((a) => a.getAttribute('href')),
    aboveActions: line && actions
      ? line.getBoundingClientRect().bottom <= actions.getBoundingClientRect().top
      : null,
  };
})()`);
const quoteLink = await run(`(() => {
  const link = document.querySelector('.thread-node--focal .post__engagement-link[href$="/quotes"]');
  if (!link) return null;
  link.scrollIntoView({ block: 'center' });
  const rect = link.getBoundingClientRect();
  return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
})()`);
if (quoteLink) {
  await click(quoteLink.x, quoteLink.y);
  await wait(2500);
}
const afterQuoteClick = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);
const result = {
  likes,
  reposts,
  quotes,
  lines,
  afterQuoteClick,
  pass:
    likes.title.startsWith('Likes —') &&
    likes.heading === 'Liked by' &&
    likes.back === true &&
    (likes.actors > 0 || /no likes yet/i.test(likes.note ?? '')) &&
    (likes.actors === 0 || /^@/.test(likes.firstActor ?? '')) &&
    reposts.title.startsWith('Reposts —') &&
    reposts.heading === 'Reposted by' &&
    (reposts.actors > 0 || /no reposts yet/i.test(reposts.note ?? '')) &&
    quotes.title.startsWith('Quotes —') &&
    quotes.heading === 'Quoted by' &&
    (quotes.posts > 0 || /nothing quotes/i.test(quotes.note ?? '')) &&
    lines.aboveActions === true &&
    // The official order: reposts, quotes, likes — then saves, which are private
    // and therefore not a link.
    // Four slots every time, zeros included, so the line cannot reflow.
    lines.text.length === 4 &&
    /^\d+ repost(s)?$/.test(lines.text[0] ?? '') &&
    /^\d+ quote(s)?$/.test(lines.text[1] ?? '') &&
    /^\d+ like(s)?$/.test(lines.text[2] ?? '') &&
    /^\d+ save(s)?$/.test(lines.text[3] ?? '') &&
    afterQuoteClick.hash.endsWith('/quotes') &&
    afterQuoteClick.posts > 0,
};
console.log('engagement-pages:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

