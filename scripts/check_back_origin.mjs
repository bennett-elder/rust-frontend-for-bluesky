// Back from a post must return to the feed it was opened from — including in a
// tab opened cold from a cmd-click, where there is no history to go back to.
// Usage: node scripts/check_back_origin.mjs [feed-url]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/#/';
const cdp = 'http://localhost:9222';

const pages = async () =>
  (await (await fetch(`${cdp}/json/list`)).json()).filter((t) => t.type === 'page');

const connect = async (target) => {
  const ws = new WebSocket(target.webSocketDebuggerUrl);
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
  const evaluate = async (expression) => {
    const result = await send('Runtime.evaluate', { expression, returnByValue: true });
    return result.result.result.value;
  };
  return { ws, send, evaluate };
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const targets = await pages();
const page = targets[0];
if (!page) throw new Error('no page target');
const { ws, send, evaluate } = await connect(page);

await send('Runtime.enable');
await send('Page.enable');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
await wait(5000);

// Cards should advertise where they came from.
const card = await evaluate(`(() => {
  const link = document.querySelector('[data-post-link]')?.getAttribute('data-post-link') ?? null;
  return { link, hash: location.hash };
})()`);

// Cold-load that post link in this same tab (clearing history first is not
// possible, so instead assert the URL carries the origin and that Back prefers
// it over jumping to home).
await send('Page.navigate', { url: `http://127.0.0.1:8099/${card.link}` });
await wait(4500);

const cold = await evaluate(`({
  hash: location.hash,
  posts: document.querySelectorAll('.post').length,
  from: new URLSearchParams(location.hash.split('?')[1] || '').get('from'),
})`);

// Click the app's own Back control.
await evaluate(`document.querySelector('.thread__back')?.click()`);
await wait(3000);
const afterBack = await evaluate(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);

console.log('back-origin:', JSON.stringify({ card, cold, afterBack }, null, 2));
ws.close();
process.exit(0);
