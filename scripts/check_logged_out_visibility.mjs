// Accounts can opt out of logged-out visibility; the AppView marks them with the
// `!no-unauthenticated` label — on the *author*, not the post. Signed out, that content
// must not render, and ordinary accounts must still render.
//
// Usage: node scripts/check_logged_out_visibility.mjs [base-url] [labelled-actor]
//
// `labelled-actor` is required: an account that has opted out of logged-out visibility.
// There is deliberately no default — baking a particular account in here would make it a
// permanent test fixture and tie this repository to it.
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const labelled = process.argv[3];
if (!labelled) {
  console.error('usage: node scripts/check_logged_out_visibility.mjs <base-url> <labelled-actor>');
  process.exit(2);
}
const cdp = 'http://localhost:9222';

// A post by that account, found rather than hard-coded.
const feed = await (
  await fetch(
    `https://api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor=${encodeURIComponent(labelled)}&limit=5`,
  )
).json();
const candidate = (feed.feed ?? []).find((item) => item.post?.author?.handle === labelled);
if (!candidate) throw new Error(`no posts from ${labelled} to test with`);
const postUri = candidate.post.uri;

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
const run = async (expression) =>
  (await send('Runtime.evaluate', { expression, returnByValue: true })).result?.result?.value;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const open = async (hash) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(400);
  await send('Page.navigate', { url: base + hash });
  await wait(8000);
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');

await open(`#/profile/${labelled}`);
const gatedProfile = await run(`({
  signedIn: Boolean(document.querySelector('[data-action="account-menu"]')),
  note: document.querySelector('.profile .note')?.textContent.trim() ?? null,
  name: Boolean(document.querySelector('.profile__name')),
  posts: document.querySelectorAll('.post').length,
  back: Boolean(document.querySelector('.profile--gated .thread__back')),
})`);

await open(`#/post/${encodeURIComponent(postUri)}`);
const gatedPost = await run(`({
  gatedCards: document.querySelectorAll('.post--gated').length,
  texts: document.querySelectorAll('.post__text').length,
  note: document.querySelector('.post__gated-note')?.textContent.trim() ?? null,
})`);

// And an ordinary account must still be visible: a gate that hides everything is a bug.
await open('#/profile/bsky.app');
const ordinary = await run(`({
  name: document.querySelector('.profile__name')?.textContent.trim() ?? null,
  posts: document.querySelectorAll('.post').length,
  gated: document.querySelectorAll('.post--gated').length,
  texts: document.querySelectorAll('.post__text').length,
})`);

const result = {
  labelled,
  postUri,
  gatedProfile,
  gatedPost,
  ordinary,
  pass:
    gatedProfile.signedIn === false &&
    /signed-in readers/.test(gatedProfile.note ?? '') &&
    gatedProfile.name === false &&
    gatedProfile.posts === 0 &&
    gatedProfile.back === false &&
    gatedPost.gatedCards >= 1 &&
    gatedPost.texts === 0 &&
    /signed-in readers/.test(gatedPost.note ?? '') &&
    typeof ordinary.name === 'string' &&
    ordinary.posts > 0 &&
    // Gated cards are allowed here: an ordinary account can repost a labelled one.
    ordinary.texts > 0,
};
console.log('logged-out-visibility:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);
