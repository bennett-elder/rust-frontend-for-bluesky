import { existsSync } from 'node:fs';

// Inspect computed styles of selectors in the running page.
// Usage: node scripts/inspect_styles.mjs http://127.0.0.1:8099/ ".post__actions" ".post__action"
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
const selectors = process.argv.slice(3);
const cdp = 'http://localhost:9222';

const targets = await (await fetch(`${cdp}/json/list`)).json();
const page = targets.find((target) => target.type === 'page');
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

await send('Page.enable');
await send('Emulation.setDeviceMetricsOverride', {
  width: 430,
  height: 900,
  deviceScaleFactor: 2,
  mobile: true,
});
await send('Page.navigate', { url });
await new Promise((resolve) => setTimeout(resolve, 4500));

for (const selector of selectors) {
  const result = await send('Runtime.evaluate', {
    expression: `(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return { selector: ${JSON.stringify(selector)}, found: false };
      const style = getComputedStyle(el);
      const rect = el.getBoundingClientRect();
      return {
        selector: ${JSON.stringify(selector)},
        found: true,
        display: style.display,
        flexDirection: style.flexDirection,
        flexWrap: style.flexWrap,
        alignItems: style.alignItems,
        gap: style.gap,
        width: Math.round(rect.width),
        height: Math.round(rect.height),
        children: [...el.children].map((child) => {
          const childRect = child.getBoundingClientRect();
          return {
            tag: child.tagName,
            cls: child.getAttribute('class'),
            x: Math.round(childRect.x),
            y: Math.round(childRect.y),
            w: Math.round(childRect.width),
            h: Math.round(childRect.height),
          };
        }),
        text: el.textContent.trim().slice(0, 40),
        naturalWidth: el.naturalWidth ?? null,
        complete: el.complete ?? null,
        html: el.outerHTML.slice(0, 140),
      };
    })()`,
    returnByValue: true,
  });
  console.log(JSON.stringify(result.result.result.value, null, 2));
}

ws.close();
process.exit(existsSync('.') ? 0 : 0);
