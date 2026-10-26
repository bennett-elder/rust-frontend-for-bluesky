// A thread must not just stop where `getPostThread`'s depth ran out. This opens a
// thread the API truncates (a node at the cut still reports replies it did not send),
// unfolds the reply sets, and expects a "Continue thread (N replies)" junction that
// navigates to that node so its own branch can be read.
//
// Usage: node scripts/check_thread_continue.mjs [base-url] [thread-uri]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
// No baked default: a real post identifier does not belong in the repository.
const threadUri = process.argv[3];
if (!threadUri) {
  console.error('usage: node scripts/check_thread_continue.mjs <base-url> <thread-uri>');
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
// about:blank first: navigating to the same document with a different hash does not
// reload it, so a fresh build would never be picked up.
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: `${base}#/post/${encodeURIComponent(threadUri)}` });
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("document.querySelectorAll('.thread-node').length > 0")) break;
  await wait(500);
}
await wait(800);

// Junctions inside a folded reply set only mount when the fold opens, so open them.
for (let round = 0; round < 4; round += 1) {
  const folds = await run(`[...document.querySelectorAll('.thread-more:not(.thread-more--junction)')]
    .filter((button) => button.getAttribute('aria-expanded') === 'false').length`);
  if (!folds) break;
  await run(`[...document.querySelectorAll('.thread-more:not(.thread-more--junction)')]
    .filter((button) => button.getAttribute('aria-expanded') === 'false')
    .forEach((button) => button.click())`);
  await wait(700);
}

const junctions = await run(`[...document.querySelectorAll('.thread-more--junction')]
  .map((button) => button.innerText.trim())`);
const before = await run('location.hash');
const target = await run(`(() => {
  const button = document.querySelector('.thread-more--junction');
  if (!button) return null;
  button.scrollIntoView({ block: 'center' });
  const rect = button.getBoundingClientRect();
  return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2) };
})()`);
if (target) await click(target.x, target.y);
await wait(2500);
const after = await run(`({
  hash: location.hash,
  nodes: document.querySelectorAll('.thread-node').length,
  text: (document.querySelector('.thread .post__text')?.innerText ?? '').slice(0, 50),
})`);

const result = {
  junctions: junctions ?? [],
  before,
  after,
  pass:
    Array.isArray(junctions) &&
    junctions.length > 0 &&
    junctions.every((label) => /^Continue thread \(\d+ repl(y|ies)\)$/.test(label)) &&
    after.hash !== before &&
    after.hash.startsWith('#/post/') &&
    after.nodes > 0,
};
console.log('thread-continue:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

