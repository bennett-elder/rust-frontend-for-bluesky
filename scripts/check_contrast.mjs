// WCAG contrast, measured on the rendered page rather than read off the stylesheet.
//
// Text: 4.5:1 normal, 3:1 for large (>=24px, or >=18.66px bold) — WCAG 2.0 AA 1.4.3.
// Component boundaries: 3:1 — WCAG 2.1 1.4.11, included because a form field whose
// border is invisible on white is the same practical problem.
//
// Usage: node scripts/check_contrast.mjs [base-url] [light|dark|system-dark]
//
// `dark` sets the stored preference (the `data-theme` block); `system-dark` emulates a
// dark host with the preference left on "system" (the media-query block). Both paths
// carry the same values, and both are measured so they cannot drift apart.
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const mode = process.argv[3] ?? 'light';
const cdp = 'http://localhost:9222';
// The sign-in page is checked too: it has its own labels and notes, on its own surface.
const routes = ['#/', '#/feeds', '#/settings', '#/notifications', '#/sign-in'];

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

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

// The preference lives in localStorage, so it is set on the origin before any route is
// audited; the app reads it at boot and applies the attribute.
await send('Page.navigate', { url: base });
await wait(1500);
await run(
  mode === 'system-dark'
    ? "localStorage.setItem('altfirst.theme', 'system')"
    : `localStorage.setItem('altfirst.theme', ${JSON.stringify(mode)})`,
);
await send('Emulation.setEmulatedMedia', {
  features: [{ name: 'prefers-color-scheme', value: mode === 'system-dark' ? 'dark' : 'light' }],
});

const audit = `(() => {
  const parse = (value) => {
    const match = value.match(/rgba?\\(([^)]+)\\)/);
    if (!match) return null;
    const parts = match[1].split(',').map((part) => parseFloat(part.trim()));
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  };
  const over = (top, bottom) => ({
    r: top.r * top.a + bottom.r * (1 - top.a),
    g: top.g * top.a + bottom.g * (1 - top.a),
    b: top.b * top.a + bottom.b * (1 - top.a),
    a: 1,
  });
  const backgroundOf = (element) => {
    const stack = [];
    let node = element;
    while (node && node !== document.documentElement) {
      const colour = parse(getComputedStyle(node).backgroundColor);
      if (colour && colour.a > 0) {
        stack.push(colour);
        if (colour.a === 1) break;
      }
      node = node.parentElement;
    }
    let result = { r: 255, g: 255, b: 255, a: 1 };
    for (let index = stack.length - 1; index >= 0; index -= 1) result = over(stack[index], result);
    return result;
  };
  const luminance = ({ r, g, b }) => {
    const channel = (value) => {
      const scaled = value / 255;
      return scaled <= 0.03928 ? scaled / 12.92 : Math.pow((scaled + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
  };
  const ratio = (a, b) => {
    const first = luminance(a);
    const second = luminance(b);
    return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
  };
  const visible = (element) => {
    const rect = element.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return false;
    const style = getComputedStyle(element);
    return style.visibility !== 'hidden' && style.display !== 'none' && Number(style.opacity) > 0.1;
  };
  const name = (element) => {
    const text = (element.textContent || '').trim().replace(/\\s+/g, ' ').slice(0, 32);
    const cls = typeof element.className === 'string' ? element.className.split(' ')[0] : '';
    return element.tagName.toLowerCase() + (cls ? '.' + cls : '') + ' :: ' + text;
  };

  const failures = [];
  const seen = new Set();
  // Elements holding their own text node are what the painted colour applies to.
  const candidates = [...document.querySelectorAll('body *')].filter((element) => {
    if (!visible(element)) return false;
    return [...element.childNodes].some((node) => node.nodeType === 3 && node.textContent.trim());
  });
  for (const element of candidates) {
    const style = getComputedStyle(element);
    const colour = parse(style.color);
    if (!colour) continue;
    const background = backgroundOf(element);
    const painted = colour.a < 1 ? over(colour, background) : colour;
    const size = parseFloat(style.fontSize);
    const weight = Number(style.fontWeight) || 400;
    const large = size >= 24 || (size >= 18.66 && weight >= 700);
    const required = large ? 3 : 4.5;
    const measured = ratio(painted, background);
    const signature = [
      style.color,
      Math.round(background.r) + ',' + Math.round(background.g) + ',' + Math.round(background.b),
      size,
      weight
    ].join('|');
    if (measured + 0.005 < required && !seen.has(signature)) {
      seen.add(signature);
      failures.push({
        element: name(element),
        ratio: Number(measured.toFixed(2)),
        required,
        colour: style.color,
        background: 'rgb(' + Math.round(background.r) + ', ' + Math.round(background.g) + ', ' + Math.round(background.b) + ')',
        fontSize: size,
        weight
      });
    }
  }

  // Component boundaries: a control relying on its border to be found.
  const boundaries = [];
  // Inputs only: a button carries a text label, and that label is what identifies
  // it. A field whose only cue is a placeholder is identified by its border (1.4.11).
  for (const element of document.querySelectorAll('input, textarea, select')) {
    if (!visible(element)) continue;
    const style = getComputedStyle(element);
    if (!parseFloat(style.borderTopWidth)) continue;
    const border = parse(style.borderTopColor);
    if (!border || border.a === 0) continue;
    const background = backgroundOf(element);
    const measured = ratio(over(border, background), background);
    const label = element.tagName.toLowerCase() + (element.type ? '[' + element.type + ']' : '') +
      (element.className ? '.' + String(element.className).split(' ')[0] : '');
    if (measured + 0.005 < 3 && !seen.has('b:' + label)) {
      seen.add('b:' + label);
      boundaries.push({ element: label, ratio: Number(measured.toFixed(2)), required: 3, colour: style.borderTopColor });
    }
  }

  return { failures, boundaries, checked: candidates.length };
})()`;

const report = { mode, routes: {}, failures: [], boundaries: [], checked: 0 };

const auditRoute = async (route) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url: base + route });
  await wait(2500);
  const result = await run(audit);
  const palette = await run(
    `({ attribute: document.documentElement.dataset.theme ?? null, body: getComputedStyle(document.body).backgroundColor })`,
  );
  if (result?.threw) {
    report.routes[route] = { error: result.threw };
    return null;
  }
  report.routes[route] = {
    attribute: palette?.attribute ?? null,
    body: palette?.body ?? null,
    checked: result.checked,
    failures: result.failures.length,
    boundaries: result.boundaries.length,
  };
  report.checked += result.checked;
  for (const failure of result.failures) report.failures.push({ route, ...failure });
  for (const boundary of result.boundaries) report.boundaries.push({ route, ...boundary });
  return result;
};

// Small text lives in post cards, not on the empty-ish shell routes: a feed, a
// profile, and a thread reached from the feed, since signing in is not possible here.
const whatsHot = 'at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot';
for (const route of [...routes, '#/feed/' + whatsHot, '#/profile/bsky.app']) {
  await auditRoute(route);
}

await auditRoute('#/feed/' + whatsHot);
const threadHref = await run(`(() => {
  const link = document.querySelector('a[href*="/post/"]');
  return link ? link.getAttribute('href') : null;
})()`);
if (typeof threadHref === 'string' && threadHref.includes('/post/')) {
  await auditRoute(threadHref);
} else {
  report.routes.thread = { error: 'no post link found on the feed' };
}

const unique = (list, key) => {
  const seen = new Map();
  for (const item of list) {
    const signature = key(item);
    if (!seen.has(signature)) seen.set(signature, { ...item, routes: [item.route] });
    else seen.get(signature).routes.push(item.route);
  }
  return [...seen.values()];
};

report.failures = unique(report.failures, (item) => [item.colour, item.background, item.fontSize, item.weight].join('|'));
report.boundaries = unique(report.boundaries, (item) => item.element);
console.log('contrast:', JSON.stringify({ ...report, pass: report.failures.length === 0 }, null, 2));
ws.close();
process.exit(report.failures.length === 0 ? 0 : 1);

await send('Page.enable');
await send('Page.bringToFront');

