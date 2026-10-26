// Tapping a reply must move focus to it: the route becomes that post, so it is the
// focal node and gets the counts line; Back returns to the thread you were reading.
//
// Usage: node scripts/check_reply_focus.mjs
//
// Note: a plain tap on reply text reaches a facet link when one is under the finger,
// and that opens a profile in a new tab — which looks like "the reply did nothing".
// This drives the text element itself so it measures the card, not the link.
import { writeFileSync } from 'node:fs';

// Supplied by the caller: a thread with replies, so there is something to tap.
const threadUri = process.argv[2];
if (!threadUri) {
  console.error('usage: node scripts/check_reply_focus.mjs <thread-uri>');
  process.exit(2);
}
const targets = (await (await fetch('http://localhost:9222/json/list')).json()).filter((t) => t.type === 'page');
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
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const run = async (expression) => {
  const message = await send('Runtime.evaluate', { expression, returnByValue: true });
  if (message.result?.exceptionDetails) return { threw: message.result.exceptionDetails.exception?.description };
  return message.result?.result?.value;
};
const click = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', { width: 760, height: 1000, deviceScaleFactor: 2, mobile: false });
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: `http://127.0.0.1:8099/#/post/${encodeURIComponent(threadUri)}` });
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("document.querySelectorAll('.thread-node--reply').length > 0")) break;
  await wait(500);
}
await wait(1000);

const target = await run(`(() => {
  const reply = document.querySelector('.thread-node--reply .post');
  if (!reply) return null;
  return {
    href: reply.getAttribute('data-post-link'),
    text: reply.querySelector('.post__text')?.textContent?.slice(0, 40) ?? null,
    author: reply.querySelector('.post__author')?.innerText ?? null,
  };
})()`);
// The focal post *before* the tap, so the check can prove the view changed and not
// just the URL. That distinction is the whole bug: the route moved and the page did not.
const focalBefore = await run(`(() => {
  const focal = document.querySelector('.thread-node--focal .post');
  return {
    author: focal?.querySelector('.post__author')?.innerText ?? null,
    text: focal?.querySelector('.post__text')?.textContent?.slice(0, 40) ?? null,
  };
})()`);
const before = await run(`({ hash: location.hash, lines: document.querySelectorAll('.post__engagement').length })`);
// The reply text is full of facet links, so drive the element itself: this exercises
// the delegated handler, which is what a plain tap on the text reaches.
await run("document.querySelector('.thread-node--reply .post__text').click()");
await wait(2600);
const after = await run(`(() => {
  const node = document.querySelector('.thread-node--focal');
  const line = node?.querySelector('.post__engagement');
  const card = node?.querySelector('.post');
  return {
    hash: location.hash,
    focalIsReply: Boolean(node?.classList.contains('thread-node--focal')),
    focal: {
      author: card?.querySelector('.post__author')?.innerText ?? null,
      text: card?.querySelector('.post__text')?.textContent?.slice(0, 40) ?? null,
    },
    line: line
      ? [...line.querySelectorAll('.post__engagement-link, .post__engagement-count')].map((n) => n.textContent.trim())
      : null,
  };
})()`);
await run('history.back()');
await wait(2000);
const back = await run(`({ hash: location.hash, nodes: document.querySelectorAll('.thread-node').length })`);
await send('Emulation.clearDeviceMetricsOverride');

const shot = await send('Page.captureScreenshot', { format: 'png' }).catch(() => null);
if (shot) writeFileSync('.tmp/reply-focus.png', Buffer.from(shot.result.data, 'base64'));

const result = {
  target,
  focalBefore,
  before,
  after,
  back,
  pass:
    Boolean(target?.href) &&
    typeof target?.text === 'string' &&
    target.text.length > 0 &&
    after.hash === target.href &&
    // The view must actually be the reply now — same author, same opening text —
    // and different from the post that was focal before the tap.
    after.focal.author === target.author &&
    after.focal.text === target.text &&
    (after.focal.author !== focalBefore.author || after.focal.text !== focalBefore.text) &&
    Boolean(after.line) &&
    after.line.length === 4 &&
    back.hash === before.hash &&
    back.nodes > 0,
};
console.log('reply-focus:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

