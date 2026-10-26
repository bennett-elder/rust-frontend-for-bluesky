// What does endless scroll actually cost? Measure before optimising — which is why this
// because `content-visibility: auto` may already be doing enough. This walks a feed
// up to a few checkpoints and, at each one, records DOM size plus the browser's own
// work for a fixed scroll burst: nodes, layout/style/task time, JS heap.
//
// The burst is the comparable bit: same 12 x 400px scrolls, one rAF each, at every
// checkpoint. Wall-clock alone is noisy; the Performance metrics deltas are not.
//
// Usage: node scripts/measure_feed_cost.mjs [feed-url] [max-posts]
//
// ALT_FIRST_CDP points it at another endpoint — the Android WebView, after
// `adb forward tcp:9223 localabstract:webview_devtools_remote_<pid>`, is the
// interesting one, because that is where jank would actually be felt.
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
const maxPosts = Number(process.argv[3] ?? 240);
const cdp = process.env.ALT_FIRST_CDP ?? 'http://localhost:9222';

import http from 'node:http';

// The Android WebView's devtools HTTP server drops keep-alive connections, which
// undici (node's fetch) treats as a failure. Ask for Connection: close instead.
const fetchTargets = (endpoint) =>
  new Promise((resolve, reject) => {
    const request = http.get(`${endpoint}/json/list`, { headers: { Connection: 'close' } }, (response) => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', (chunk) => (body += chunk));
      response.on('end', () => {
        try {
          resolve(JSON.parse(body));
        } catch (error) {
          reject(error);
        }
      });
    });
    request.on('error', reject);
  });

const targets = (await fetchTargets(cdp)).filter((t) => t.type === 'page');
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
const run = async (expression, awaitPromise = false) => {
  const message = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise });
  if (message.result?.exceptionDetails) {
    return { threw: message.result.exceptionDetails.exception?.description ?? 'error' };
  }
  return message.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Performance.enable');
// The Android WebView has no `Page.bringToFront` and is already the only page;
// with ALT_FIRST_NO_NAV=1 the run measures whatever is on screen right now.
const noNav = process.env.ALT_FIRST_NO_NAV === '1';
if (!noNav) {
  await send('Page.bringToFront');
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url });
}
for (let attempt = 0; attempt < 30; attempt += 1) {
  if ((await run("document.querySelectorAll('.post').length")) > 0) break;
  await wait(500);
}

const metrics = async () => {
  const result = await send('Performance.getMetrics');
  return Object.fromEntries(result.result.metrics.map((entry) => [entry.name, entry.value]));
};

const count = () => run("document.querySelectorAll('.post').length");

// The feed appends a page when scrolled near the bottom. Scrolling to the same
// bottom twice fires no scroll event, so the trigger never runs: step back and in
// again each attempt. Stop when the feed says it has ended — otherwise the loop
// burns its attempts waiting for pages that do not exist.
const growTo = async (target) => {
  for (let attempt = 0; attempt < 24; attempt += 1) {
    if ((await count()) >= target) return true;
    const ended = await run(
      "/(end of this feed)/i.test(document.querySelector('.feed-end')?.innerText ?? '')"
    );
    if (ended) return false;
    await run('window.scrollTo(0, Math.max(0, document.body.scrollHeight - innerHeight))');
    await wait(200);
    await run('window.scrollTo(0, document.body.scrollHeight)');
    await wait(1300);
  }
  return (await count()) >= target;
};

const sample = async () => {
  // Let the page finish whatever the last append started: a freshly added page
  // lays out 30 cards, and catching that in the measurement window reads as
  // "scrolling got expensive" when it was really "a page arrived".
  await wait(1500);
  // Sit mid-feed so the burst cannot trigger a pagination fetch and pollute the
  // numbers with network work.
  await run(`(() => {
    window.scrollTo(0, Math.floor((document.body.scrollHeight - innerHeight) * 0.6));
    return window.scrollY;
  })()`);
  await wait(600);

  const before = await metrics();
  // No requestAnimationFrame here: an unfocused or headless window throttles it and
  // the burst hangs forever (the trap in README's verify notes). Reading
  // `offsetHeight` after each scroll forces the same style+layout work a frame
  // would do, synchronously, so the metric deltas are comparable at every size.
  const wall = await run(`(() => {
    const start = performance.now();
    for (let step = 0; step < 12; step += 1) {
      window.scrollBy(0, 400);
      void document.body.offsetHeight;
    }
    return performance.now() - start;
  })()`);
  await wait(250);
  const after = await metrics();

  const dom = await run(`(() => {
    const posts = document.querySelectorAll('.post');
    const visible = [...posts].filter((post) => {
      const rect = post.getBoundingClientRect();
      return rect.bottom > 0 && rect.top < innerHeight;
    }).length;
    return {
      posts: posts.length,
      nodes: document.querySelectorAll('*').length,
      visiblePosts: visible,
      pageHeight: Math.round(document.body.scrollHeight),
      scrollY: Math.round(window.scrollY),
    };
  })()`);

  const delta = (key) => Number(((after[key] ?? 0) - (before[key] ?? 0)).toFixed(3));

  // The other thing a long feed can cost is update work: every card reads the
  // alt-first preference, so flipping it re-renders all of them. Dioxus schedules
  // the render, so measure the browser work in the window after the click rather
  // than around it.
  const toggleBefore = await metrics();
  const clicked = await run(`(() => {
    const box = document.querySelector('input[type="checkbox"]');
    if (!box) return false;
    box.click();
    return true;
  })()`);
  await wait(500);
  await run('void document.body.offsetHeight');
  const toggleAfter = await metrics();
  if (clicked) await run('document.querySelector(\'input[type="checkbox"]\')?.click()');
  await wait(350);

  const toggle = (key) => Number(((toggleAfter[key] ?? 0) - (toggleBefore[key] ?? 0)).toFixed(3));

  return {
    ...dom,
    burstMs: Number(wall.toFixed(1)),
    layoutMs: Number((delta('LayoutDuration') * 1000).toFixed(1)),
    styleMs: Number((delta('RecalcStyleDuration') * 1000).toFixed(1)),
    scriptMs: Number((delta('ScriptDuration') * 1000).toFixed(1)),
    taskMs: Number((delta('TaskDuration') * 1000).toFixed(1)),
    layouts: delta('LayoutCount'),
    styleRecalcs: delta('RecalcStyleCount'),
    heapMB: Number(((after.JSHeapUsedSize ?? 0) / 1048576).toFixed(1)),
    toggle: {
      styleMs: Number((toggle('RecalcStyleDuration') * 1000).toFixed(1)),
      layoutMs: Number((toggle('LayoutDuration') * 1000).toFixed(1)),
      taskMs: Number((toggle('TaskDuration') * 1000).toFixed(1)),
      styleRecalcs: toggle('RecalcStyleCount'),
      layouts: toggle('LayoutCount'),
    },
  };
};

const checkpoints = [30, 60, 120, 180, maxPosts].filter((value, index, all) => value <= maxPosts && all.indexOf(value) === index);
const rows = [];
for (const target of checkpoints) {
  const reached = await growTo(target);
  if (!reached) {
    rows.push({ target, reached: false, posts: await count() });
    break;
  }
  rows.push({ target, reached: true, ...(await sample()) });
}

console.log('feed-cost:', JSON.stringify({ url, rows }, null, 2));
ws.close();
process.exit(0);
