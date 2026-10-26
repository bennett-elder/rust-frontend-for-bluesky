// Posts must say when they were posted. A card shows relative time ("5m", "3h", "Oct 25"),
// the post you opened shows the exact local time, and both keep the precise timestamp in
// `title` — the old version rendered a bare date and dropped the time entirely.
//
// Usage: node scripts/check_when.mjs '<base-url>' '<post-url>'
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';
const feed = `${base}#/feed/at%3A%2F%2Fdid%3Aplc%3Az72i7hdynmk6r22z27h6tvur%2Fapp.bsky.feed.generator%2Fwhats-hot`;

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
const run = async (expression) =>
  (await send('Runtime.evaluate', { expression, returnByValue: true })).result?.result?.value;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const open = async (url) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url });
  await wait(8000);
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');

await open(feed);
const cards = await run(`(() => {
  const nodes = [...document.querySelectorAll('.post__when')];
  return {
    count: nodes.length,
    samples: nodes.slice(0, 6).map((node) => ({
      text: node.textContent.trim(),
      title: node.getAttribute('title'),
      iso: node.getAttribute('data-when'),
    })),
  };
})()`);

// Open one post so the focal card shows the absolute form.
await run(`document.querySelector('.post__permalink')?.click()`);
await wait(8000);
const focal = await run(`(() => {
  const node = document.querySelector('.post__when[data-format="absolute"]');
  const fallback = document.querySelector('.post__when');
  return {
    text: node ? node.textContent.trim() : null,
    title: node ? node.getAttribute('title') : null,
    anyRelative: Boolean(document.querySelector('.post__when[data-format="relative"]')),
    hash: location.hash.slice(0, 12),
  };
})()`);

// A relative label is "now" or a number+unit; a bare "Oct 25" is the failure this guards.
const relativePattern = /^(·\s)?(now|\d+\s?(m|min|h|hr|d|day|wk|w)s?(\. ago| ago)?|\d+ (min|hr|hrs|min|mins)\. ago|[A-Z][a-z]{2} \d{1,2}(, \d{4})?)$/;
const relativeOk = cards.samples.length > 0 && cards.samples.every((sample) => relativePattern.test(sample.text));
const titlesOk = cards.samples.every((sample) => /\d{1,2}:\d{2}/.test(sample.title ?? ''));
const focalOk = typeof focal.text === 'string' && /\d{1,2}:\d{2}/.test(focal.text) && /\d{1,2}:\d{2}/.test(focal.title ?? '');

const result = { cards, focal, relativeOk, titlesOk, focalOk, pass: relativeOk && titlesOk && focalOk };
console.log('when:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);
