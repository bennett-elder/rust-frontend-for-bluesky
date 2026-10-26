// The signed-out shapes of reply and mute/block.
//
// Writes need a session, so what is asserted here is that the affordances are present
// and honest without one: a thread explains that replying needs sign-in instead of
// showing a box that cannot work, and a profile offers no mute/block that would fail.
//
// Usage: node scripts/check_reply_block.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const post = 'at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.post/3myd52wqlwc2b';
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
const open = async (hash) => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(350);
  await send('Page.navigate', { url: base + hash });
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (await run("document.querySelectorAll('.post').length > 0")) break;
    await wait(400);
  }
  await wait(1200);
};

await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');

// A thread: one reply box, and signed out it is an explanation, not a textarea.
await open(`#/post/${encodeURIComponent(post)}`);
const thread = await run(`(() => ({
  boxes: document.querySelectorAll('.reply-box').length,
  note: document.querySelector('.reply-box__note')?.textContent.trim() ?? null,
  textarea: Boolean(document.querySelector('.reply-box__input')),
  send: Boolean(document.querySelector('.reply-box__send')),
  noteMentionsHandle: /@[a-z0-9.-]+/.test(document.querySelector('.reply-box__note')?.textContent ?? ''),
}))()`);

// A profile: no mute/block without a session, and nothing on a public read that
// pretends otherwise.
await open('#/profile/bsky.app');
const profile = await run(`(() => ({
  actions: document.querySelectorAll('.profile__action').length,
  posts: document.querySelectorAll('.post').length,
  blockedNote: Boolean([...document.querySelectorAll('.note')].find((n) => /blocked/i.test(n.textContent))),
}))()`);

const result = {
  thread,
  profile,
  pass:
    thread.boxes === 1 &&
    thread.textarea === false &&
    thread.send === false &&
    typeof thread.note === 'string' &&
    /sign in/i.test(thread.note) &&
    thread.noteMentionsHandle === true &&
    profile.actions === 0 &&
    profile.posts > 0 &&
    profile.blockedNote === false,
};
console.log('reply-block:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

