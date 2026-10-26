// Covers the Android surfaces that screenshots could not settle: profiles, the
// failure panel, back-navigation and the folded-replies disclosure. Navigates by
// setting location.hash over CDP, so no coordinate guessing.
// Usage: node scripts/android_surfaces.mjs
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
const go = async (hash, settle = 5000) => {
  await run(`location.hash = ${JSON.stringify(hash)}`);
  await wait(settle);
  return run(`({ hash: location.hash, text: document.body.innerText.slice(0, 70).replace(/\\n/g, ' | ') })`);
};

await send('Runtime.enable');
const report = {};

// 1. Profile: banner, handle and profile links.
report.profile = await run(`(async () => { location.hash = '#/profile/bsky.app'; return true; })()`);
await wait(6000);
report.profile = await run(`({
  hash: location.hash,
  hasProfile: Boolean(document.querySelector('.profile')),
  links: document.querySelectorAll('.profile__links a, .profile-link').length,
  posts: document.querySelectorAll('.post').length,
  snippet: document.body.innerText.slice(0, 60).replace(/\\n/g, ' | '),
})`);

// 2. Failure panel for a feed that does not exist.
await run(`location.hash = '#/feed/at%3A%2F%2Fdid%3Aplc%3Aaaaaaaaaaaaaaaaaaaaaaaaa%2Fapp.bsky.feed.generator%2Fnope'`);
await wait(6000);
report.missingFeed = await run(`({
  headline: document.querySelector('.feed-error__headline')?.textContent ?? null,
  hint: document.querySelector('.feed-error .note')?.textContent ?? null,
  retry: Boolean([...document.querySelectorAll('.feed-error button')].find((b) => b.textContent === 'Try again')),
})`);

// 3. Back from a profile returns to the feed.
report.backFromProfile = await (async () => {
  await go('#/profile/bsky.app', 5000);
  const before = await run('location.hash');
  await run('history.back()');
  await wait(4000);
  const after = await run(`({ hash: location.hash, posts: document.querySelectorAll('.post').length })`);
  return { before, after };
})();

// 4. Folded replies, when the thread has a bushy node.
await go('#/post/at%3A%2F%2Fdid%3Aplc%3A4sswczmqlthkhyaj2nzjridq%2Fapp.bsky.feed.post%2F3my6npvi4cw7y', 6000);
report.folded = await run(`(() => {
  const summary = document.querySelector('.thread-more > summary');
  if (!summary) return { fold: null, reason: 'no node with >20 direct replies in this thread' };
  const hidden = document.querySelector('.thread-more .post');
  const before = hidden ? hidden.checkVisibility?.() ?? false : null;
  summary.click();
  return { fold: summary.textContent, visibleBefore: before };
})()`);
await wait(1200);
report.folded.visibleAfter = await run(`(() => {
  const p = document.querySelector('.thread-more .post');
  return p ? (p.checkVisibility?.() ?? false) : null;
})()`);

console.log('android-surfaces:', JSON.stringify(report, null, 2));
ws.close();
process.exit(0);
