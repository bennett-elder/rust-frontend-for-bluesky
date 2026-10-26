// Can you open a quoted post? The quote body carries `data-post-link` (it can't be
// an anchor — quotes contain facet links), so the delegated capture-phase handler
// has to pick the *nearest* link, not the surrounding card's. Three things to
// prove: a click inside the quote opens the quoted post, the thread renders, and
// Back returns to the feed you were reading (`from` carried into the quote href).
//
// Usage: node scripts/check_quote_nav.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8097/';
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
  const result = message.result;
  if (!result) return { error: message.error?.message ?? 'no result' };
  if (result.exceptionDetails) {
    return { threw: result.exceptionDetails.exception?.description ?? 'error' };
  }
  return result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const click = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
// Boot: the wasm has to run before anything is in the DOM, and a 6 s guess is a
// coin flip on a cold debug bundle.
for (let attempt = 0; attempt < 30; attempt += 1) {
  const ready = await run(
    "document.querySelectorAll('.post').length > 0 || Boolean(document.querySelector('.feed__failure'))"
  );
  if (ready === true) break;
  await wait(500);
}
await wait(500);

// Scroll while nothing is quoted: Discover often has quotes, but not always in
// the first page, and pagination appends rather than replacing.
const find = `(() => {
  const quote = document.querySelector('.quote[data-post-link]');
  if (!quote) return null;
  const text = quote.querySelector('.quote__text') ?? quote.querySelector('.quote__head');
  const r = text.getBoundingClientRect();
  return {
    href: quote.getAttribute('data-post-link'),
    quoteText: (quote.querySelector('.quote__text')?.innerText ?? '').slice(0, 60),
    cardHref: quote.closest('.post')?.getAttribute('data-post-link') ?? null,
    x: Math.round(r.left + Math.min(40, r.width / 2)),
    y: Math.round(r.top + Math.min(12, r.height / 2)),
    scrollY: window.scrollY,
  };
})()`;
const feedHash = await run('location.hash');
let quote = await run(find);
for (let page = 0; page < 4 && !quote; page += 1) {
  await run('window.scrollTo(0, document.body.scrollHeight)');
  await wait(2500);
  quote = await run(find);
}

if (!quote) {
  console.log('quote-nav:', JSON.stringify({ feedHash, quote: null, reason: 'no quoted post in this feed' }, null, 2));
  ws.close();
  process.exit(2);
}

await run('window.scrollTo(0, 0)');
await wait(300);
// Bring the quote into the viewport before measuring: rects below the fold aim
// the synthetic click at nothing, which reads as "the handler is broken".
await run(`(() => {
  const quote = document.querySelector('.quote[data-post-link]');
  quote?.scrollIntoView({ block: 'center' });
  return Boolean(quote);
})()`);
await wait(500);
const aimed = await run(find);
const hit = await run(`(() => {
  const element = document.elementFromPoint(${aimed.x}, ${aimed.y});
  return {
    tag: element?.tagName ?? null,
    cls: typeof element?.className === 'string' ? element.className : null,
    closestLink: element?.closest?.('[data-post-link]')?.getAttribute('data-post-link') ?? null,
    inViewport: ${aimed.y} >= 0 && ${aimed.y} <= innerHeight,
  };
})()`);
const before = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);
await click(aimed.x, aimed.y);
await wait(3000);
const after = await run(`({
  hash: location.hash,
  thread: Boolean(document.querySelector('.thread__back')),
  text: (document.querySelector('.thread .post__text')?.innerText ?? '').slice(0, 60),
})`);

await run('history.back()');
await wait(2500);
const back = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);

const result = {
  feedHash,
  quoteHref: aimed.href,
  cardHref: aimed.cardHref,
  quoteText: aimed.quoteText,
  before,
  after,
  back,
  nested: aimed.cardHref !== null && aimed.href !== aimed.cardHref,
  opened: after.hash === aimed.href,
  carriedOrigin: aimed.href.includes('?from='),
  returnedToFeed: back.hash === before.hash,
};
console.log('quote-nav:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.opened && result.nested && result.returnedToFeed ? 0 : 1);
