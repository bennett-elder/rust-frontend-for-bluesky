// Finds a real thread whose root has >20 direct replies (the fold threshold) and
// checks the disclosure on the device. Data-dependent by nature, hence the hunt.
const cdp = process.argv[2] ?? 'http://127.0.0.1:9223';
const PUBLIC = 'https://api.bsky.app/xrpc';

const feed = await (
  await fetch(
    `${PUBLIC}/app.bsky.feed.getFeed?limit=50&feed=${encodeURIComponent(
      'at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot',
    )}`,
  )
).json();
const candidates = (feed.feed ?? [])
  .map((item) => item.post)
  .filter((post) => (post?.replyCount ?? 0) > 25);
console.error(`candidates: ${candidates.length}`);

let target = null;
for (const post of candidates.slice(0, 12)) {
  const thread = await (
    await fetch(`${PUBLIC}/app.bsky.feed.getPostThread?depth=1&uri=${encodeURIComponent(post.uri)}`)
  ).json();
  const direct = thread.thread?.replies?.length ?? 0;
  if (direct > 20) {
    target = { uri: post.uri, direct };
    break;
  }
}
if (!target) {
  console.log('fold-check: no thread with >20 direct replies found; inconclusive');
  process.exit(0);
}
console.error(`target: ${target.direct} direct replies`);

const page = (await (await fetch(`${cdp}/json/list`)).json()).find((t) => t.type === 'page');
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
  const result = await send('Runtime.evaluate', { expression, returnByValue: true });
  return result.result.exceptionDetails
    ? { threw: result.result.exceptionDetails.exception?.description }
    : result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
const hash = `#/post/${encodeURIComponent(target.uri)}`;
await run(`location.hash = ${JSON.stringify(hash)}`);
await wait(7000);

const before = await run(`(() => {
  const summary = document.querySelector('.thread-more > summary');
  const first = document.querySelector('.thread-more .post');
  return {
    summary: summary ? summary.textContent : null,
    visibleBefore: first ? (first.checkVisibility?.() ?? null) : null,
  };
})()`);
await run(`document.querySelector('.thread-more > summary')?.click()`);
await wait(1200);
const after = await run(`(() => {
  const first = document.querySelector('.thread-more .post');
  return { visibleAfter: first ? (first.checkVisibility?.() ?? null) : null };
})()`);

console.log(JSON.stringify({ target, before, after }, null, 2));
ws.close();
process.exit(0);
