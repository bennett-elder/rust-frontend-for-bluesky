// Can you tell what is the opened post, what is an ancestor, and who a deep reply
// answers? The old thread view drew the same rail on everything, indented without a
// cap, and left ancestors as bare cards. This asserts the four fixes at phone width:
// the focal post is marked and draws no rail, replies do, the indent stops at level 3,
// past which the parent is named, and ancestors sit in a labelled group.
//
// Usage: node scripts/check_thread_hierarchy.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
// Two threads: one deep enough to hit the indent cap, one bushy enough to have replies.
// Both supplied by the caller, because baked post identifiers date badly and do not
// belong in the repository.
const deepPost = process.argv[3];
const bushyThread = process.argv[4];
if (!deepPost || !bushyThread) {
  console.error('usage: node scripts/check_thread_hierarchy.mjs <base-url> <deep-thread-uri> <bushy-thread-uri>');
  process.exit(2);
}
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
  const message = await send('Runtime.evaluate', { expression, returnByValue: true });
  if (message.result?.exceptionDetails) return { threw: message.result.exceptionDetails.exception?.description };
  return message.result?.result?.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const openThread = async (uri) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url: `${base}#/post/${encodeURIComponent(uri)}` });
  for (let attempt = 0; attempt < 30; attempt += 1) {
    if (await run("document.querySelectorAll('.thread-node').length > 0")) break;
    await wait(500);
  }
  await wait(900);
  // Depth beyond the cap only mounts where a fold is open; open them.
  for (let round = 0; round < 3; round += 1) {
    const closed = await run(`[...document.querySelectorAll('.thread-more:not(.thread-more--junction)')]
      .filter((button) => button.getAttribute('aria-expanded') === 'false').length`);
    if (!closed) break;
    await run(`[...document.querySelectorAll('.thread-more:not(.thread-more--junction)')]
      .filter((button) => button.getAttribute('aria-expanded') === 'false')
      .forEach((button) => button.click())`);
    await wait(700);
  }
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });

// A bushy thread: focal marker, rails, capped indent, named parent.
await openThread(bushyThread);
const bushy = await run(`(() => {
  const style = (node) => getComputedStyle(node);
  const focal = document.querySelector('.thread-node--focal');
  const replies = [...document.querySelectorAll('.thread-node--reply')];
  const depths = [...document.querySelectorAll('.thread-node')].map((node) =>
    node.style.getPropertyValue('--depth').trim()
  );
  const parentHints = [...document.querySelectorAll('.thread-node__parent')].map((node) => node.innerText.trim());
  return {
    focal: Boolean(focal),
    focalBorder: focal ? style(focal).borderLeftWidth : null,
    focalMargin: focal ? style(focal).marginLeft : null,
    focalMarked: focal ? style(focal.querySelector('.post')).boxShadow !== 'none' : null,
    replyCount: replies.length,
    replyBorder: replies[0] ? style(replies[0]).borderLeftWidth : null,
    replyMargin: replies[0] ? style(replies[0]).marginLeft : null,
    maxDepth: Math.max(...depths.map(Number)),
    depthValues: [...new Set(depths)].sort(),
    parentHints: parentHints.slice(0, 3),
    ancestors: Boolean(document.querySelector('.thread-ancestors')),
  };
})()`);

// A post deep in a chain: its ancestors must be grouped and labelled.
await openThread(deepPost);
const deep = await run(`(() => {
  const group = document.querySelector('.thread-ancestors');
  return {
    ancestors: Boolean(group),
    label: group?.querySelector('.thread-ancestors__label')?.innerText.trim() ?? null,
    ancestorCards: group ? group.querySelectorAll('.post').length : 0,
    focal: Boolean(document.querySelector('.thread-node--focal')),
  };
})()`);
await send('Emulation.clearDeviceMetricsOverride');

const result = {
  bushy,
  deep,
  pass:
    bushy.focal === true &&
    bushy.focalBorder === '0px' &&
    bushy.focalMargin === '0px' &&
    bushy.focalMarked === true &&
    bushy.replyCount > 0 &&
    bushy.replyBorder === '2px' &&
    bushy.replyMargin !== '0px' &&
    bushy.maxDepth <= 3 &&
    bushy.parentHints.length > 0 &&
    bushy.parentHints.every((hint) => /replying to @/.test(hint)) &&
    deep.ancestors === true &&
    // CSS text-transform is reflected in innerText, so compare case-insensitively.
    /earlier in this thread/i.test(deep.label ?? '') &&
    deep.ancestorCards > 0 &&
    deep.focal === true,
};
console.log('thread-hierarchy:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

