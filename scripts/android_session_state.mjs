// What does the Android WebView think the session state is? Prints the header
// text and the names of any `altfirst.*` localStorage keys — never their values,
// so this is safe to run against a signed-in device.
//
// Usage:
//   adb forward tcp:9223 localabstract:webview_devtools_remote_<pid>
//   node scripts/android_session_state.mjs
//
// What it settles: force-stop + relaunch with "remember me" off must come back
// logged out with no altfirst.session key; with it on, the key must be there and
// the header must still be signed in after several restarts.
import http from 'node:http';

const endpoint = process.env.ALT_FIRST_CDP ?? 'http://127.0.0.1:9223';
const targets = await new Promise((resolve, reject) => {
  const request = http.get(`${endpoint}/json/list`, { headers: { Connection: 'close' } }, (response) => {
    let body = '';
    response.setEncoding('utf8');
    response.on('data', (chunk) => (body += chunk));
    response.on('end', () => resolve(JSON.parse(body)));
  });
  request.on('error', reject);
});
const page = targets.find((target) => target.type === 'page');
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
const run = async (expression) =>
  (await send('Runtime.evaluate', { expression, returnByValue: true })).result?.result?.value;

await send('Runtime.enable');
const state = await run(`(() => ({
  header: (document.querySelector('.app-header')?.innerText ?? '').replace(/\\s+/g, ' ').trim().slice(0, 48),
  signedIn: Boolean(localStorage.getItem('altfirst.session')),
  keys: Object.keys(localStorage).filter((key) => key.startsWith('altfirst')).sort(),
  posts: document.querySelectorAll('.post').length,
}))()`);
console.log('device-state:', JSON.stringify(state, null, 2));
ws.close();
process.exit(0);

