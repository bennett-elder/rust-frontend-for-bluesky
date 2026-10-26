// Prints document.title per route, since the tab title is only observable live.
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
  const result = await send('Runtime.evaluate', { expression, returnByValue: true });
  return result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: 'http://127.0.0.1:8099/#/' });
await wait(5000);

const titles = [];
titles.push({ hash: '#/', title: await run('document.title') });

for (const hash of [
  '#/',
  process.argv[2],
  '#/following',
  '#/feeds',
  '#/settings',
  '#/profile/bsky.app',
  '#/post/at%3A%2F%2Fdid%3Aplc%3A4sswczmqlthkhyaj2nzjridq%2Fapp.bsky.feed.post%2F3my6npvi4cw7y',
]) {
  if (!hash) continue;
  await run(`location.hash = ${JSON.stringify(hash)}`);
  await wait(2500);
  titles.push({
    hash,
    title: await run('document.title'),
    // The surface you are on and the title must agree; that mismatch was a real bug.
    // Rail on desktop, tab row on narrow screens — whichever is showing.
    activeNav: await run(`(() => {
      const visible = (element) => element.getClientRects().length > 0;
      const rail = [...document.querySelectorAll('.rail__item--active')].find(visible);
      const tab = [...document.querySelectorAll('.feed-tab--active')].find(visible);
      return (rail ?? tab)?.textContent?.trim() ?? null;
    })()`),
  });
}

console.log('titles:', JSON.stringify(titles, null, 2));
ws.close();
process.exit(0);
