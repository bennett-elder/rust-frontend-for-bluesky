// Navigate a page and print uncaught exceptions / console errors.
// Usage: node scripts/console_errors.mjs http://127.0.0.1:8099/
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
const cdp = 'http://localhost:9222';

const targets = await (await fetch(`${cdp}/json/list`)).json();
const page = targets.find((target) => target.type === 'page');
if (!page) throw new Error('no page target');

const ws = new WebSocket(page.webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
const events = [];

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
    return;
  }
  if (message.method === 'Runtime.exceptionThrown') {
    const detail = message.params.exceptionDetails;
    events.push(`EXCEPTION: ${detail.text} ${detail.exception?.description ?? ''}`);
  }
  if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
    events.push(
      `CONSOLE ERROR: ${message.params.args.map((a) => a.value ?? a.description).join(' ')}`,
    );
  }
};

await new Promise((resolve) => (ws.onopen = resolve));
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const messageId = ++id;
    pending.set(messageId, resolve);
    ws.send(JSON.stringify({ id: messageId, method, params }));
  });

await send('Runtime.enable');
await send('Page.enable');
await send('Page.navigate', { url });
await new Promise((resolve) => setTimeout(resolve, 4500));

const body = await send('Runtime.evaluate', {
  expression: `(() => ({
    text: document.body.innerText.slice(0, 600),
    posts: document.querySelectorAll('.post').length,
    note: document.querySelector('.note')?.textContent ?? null,
    error: document.querySelector('.error')?.textContent ?? null,
  }))()`,
  returnByValue: true,
});

console.log('page text:', JSON.stringify(body.result.result.value));
console.log('events:');
for (const event of events.slice(0, 8)) console.log(' ', event);

ws.close();
process.exit(0);
