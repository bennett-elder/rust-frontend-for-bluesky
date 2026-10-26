// Throwaway diagnostic for the scroll-memory wiring.
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
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
  return result.result.exceptionDetails
    ? { threw: result.result.exceptionDetails.text, detail: result.result.exceptionDetails.exception?.description }
    : result.result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.navigate', { url: process.argv[2] });
await wait(6000);

const wiring = await run(`({
  flag: !!window.__altScrollMemory,
  restoreFn: typeof window.__altScrollRestore,
  source: String(window.__altScrollRestore).slice(0, 90),
  ls: localStorage.getItem('altfirst.scroll'),
  writes: (() => { const raw = localStorage.getItem('altfirst.scroll'); return raw ? Object.keys(JSON.parse(raw)) : []; })(),
})`);

// Watch a scroll happen and see whether anything lands.
await run(`window.scrollTo(0, 3000)`);
await wait(1200);
const afterScroll = await run(`({
  y: Math.round(window.scrollY),
  ls: localStorage.getItem('altfirst.scroll'),
})`);

// Synthetic event isolates "listener broken" from "scroll never fired".
await run(`window.dispatchEvent(new Event('scroll'))`);
await wait(600);
const afterSynthetic = await run(`localStorage.getItem('altfirst.scroll')`);

// Move somewhere new: restore already parked us at the saved offset, so a
// no-op scrollTo would emit no event at all and look like a broken listener.
await run(`window.scrollTo(0, 1200)`);
await wait(800);
const debug = await run(`({ state: window.__altScrollDebug?.() ?? null, ls: localStorage.getItem('altfirst.scroll') })`);

// Try the write path by hand with the same shape the listener uses.
const manual = await run(`(() => {
  try {
    const store = {};
    store[location.hash.split('?')[0]] = { y: Math.round(window.scrollY), t: Date.now() };
    localStorage.setItem('altfirst.scroll', JSON.stringify(store));
    return { ok: true, read: localStorage.getItem('altfirst.scroll') };
  } catch (error) { return { ok: false, error: String(error) }; }
})()`);

console.log('debug-scroll:', JSON.stringify({ wiring, afterScroll, afterSynthetic, debug, manual }, null, 2));
ws.close();
process.exit(0);
