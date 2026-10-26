import { existsSync } from 'node:fs';
import { writeFileSync } from 'node:fs';

// Capture a PNG of the running app via CDP. Usage:
//   node scripts/screenshot_web.mjs http://127.0.0.1:8099/ .tmp/shot.png [width]
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
const out = process.argv[3] ?? '.tmp/shot.png';
const width = Number(process.argv[4] ?? 430);
const actor = process.argv[5];
const cdp = 'http://localhost:9222';

const targets = await (await fetch(`${cdp}/json/list`)).json();
const page = targets.find((target) => target.type === 'page');
if (!page) throw new Error('no page target; run verify_dom.mjs first to start Chrome');

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

await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', {
  width,
  height: 900,
  deviceScaleFactor: 2,
  mobile: true,
});
// Load about:blank first: a hash-only navigation to the same document does not reload
// it, and a screenshot then shows whatever the previous run left on screen (a theme, a
// scroll position, a stale wasm build).
await send('Page.navigate', { url: 'about:blank' });
await new Promise((resolve) => setTimeout(resolve, 400));
await send('Page.navigate', { url });
await new Promise((resolve) => setTimeout(resolve, 4000));

if (actor) {
  await send('Runtime.evaluate', {
    expression: `location.hash = '#/profile/${encodeURIComponent(actor)}'`,
  });
  await new Promise((resolve) => setTimeout(resolve, 5000));
}

// `mediafirst` as the 6th arg flips the preference off before capturing.
if (process.argv[6] === 'mediafirst') {
  await send('Runtime.evaluate', { expression: `document.querySelector('.toggle input').click()` });
  await new Promise((resolve) => setTimeout(resolve, 2500));
}

// `thread` opens the first post's thread before capturing.
if (process.argv[6] === 'thread') {
  await send('Runtime.evaluate', {
    expression: `document.querySelector('.post__open').click()`,
  });
  await new Promise((resolve) => setTimeout(resolve, 5000));
}

const shot = await send('Page.captureScreenshot', { format: 'png' });
writeFileSync(out, Buffer.from(shot.result.data, 'base64'));
console.log(`wrote ${out} (${existsSync(out) ? 'ok' : 'missing'})`);
ws.close();
process.exit(0);
