// Drives the Android app's WebView over CDP (adb forward tcp:9223
// localabstract:webview_devtools_remote_<pid>). Same idea as the desktop checks,
// but against the device so taps can be aimed at element rects instead of guessed.
const cdp = process.argv[2] ?? 'http://127.0.0.1:9223';

const targets = await (await fetch(`${cdp}/json/list`)).json();
const page = targets.find((t) => t.type === 'page');
if (!page) throw new Error('no page target');

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
  if (result.result.exceptionDetails) {
    return { threw: result.result.exceptionDetails.exception?.description ?? 'error' };
  }
  return result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const tap = async (x, y) => {
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

await send('Runtime.enable');

// Capture phase runs before the app's own listeners, so this records what the
// browser actually delivered, independent of whether our handler returns early.
await run(`(() => {
  window.__clicks = [];
  document.addEventListener('click', (event) => {
    const anchor = event.target.closest?.("a[href^='#/']") ?? null;
    window.__clicks.push({
      tag: event.target.tagName,
      prevented: event.defaultPrevented,
      button: event.button,
      anchorHref: anchor ? anchor.getAttribute('href') : null,
      hashBefore: location.hash,
    });
  }, true);
  return true;
})()`);

const state = await run(`({
  hash: location.hash,
  clickNav: Boolean(window.__altFirstClickNav),
  scrollNav: Boolean(window.__altScrollMemory),
  anchors: document.querySelectorAll("a[href^='#/']").length,
  posts: document.querySelectorAll('.post').length,
})`);

const avatar = await run(`(() => {
  const a = document.querySelector('.post a.avatar-link');
  if (!a) return null;
  const r = a.getBoundingClientRect();
  return { href: a.getAttribute('href'), x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
})()`);

let afterAvatar = null;
if (avatar) {
  await tap(avatar.x, avatar.y);
  await wait(2500);
  afterAvatar = await run(`({ hash: location.hash, clicks: window.__clicks, posts: document.querySelectorAll('.post').length })`);
}

console.log('android-cdp:', JSON.stringify({ state, avatar, afterAvatar }, null, 2));
ws.close();
process.exit(0);
