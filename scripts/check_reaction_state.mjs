// A like or a repost that is yours must look like it *without* a pointer on it.
// Both controls had that bug in different forms: the heart stayed grey because
// `.post__action--liked` (0,1,0) lost to `button.post__action` (0,1,1) and nothing
// filled the stroke-only path; the repost had no state at all until `viewer.repost`
// was rendered. This asserts computed style with the class applied, because a
// class list assertion passed the whole time the heart was grey.
//
// Usage: node scripts/check_reaction_state.mjs [feed-url]
// The root is the Explore page (no posts until you search), so default to a feed.
const url = process.argv[2] ??
  'http://127.0.0.1:8099/#/feed/at%3A%2F%2Fdid%3Aplc%3Az72i7hdynmk6r22z27h6tvur%2Fapp.bsky.feed.generator%2Fwhats-hot';
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
  const result = message.result;
  if (!result) return { error: message.error?.message ?? 'no result' };
  if (result.exceptionDetails) {
    return { threw: result.exceptionDetails.exception?.description ?? 'error' };
  }
  return result.result.value;
};
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.bringToFront');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url });
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("document.querySelectorAll('.post').length > 0")) break;
  await wait(500);
}

const state = await run(`(() => {
  const read = (button) => ({
    fill: getComputedStyle(button.querySelector('path')).fill,
    color: getComputedStyle(button).color,
    pressed: button.getAttribute('aria-pressed'),
  });
  const like = document.querySelector('button.post__action--like');
  const repost = document.querySelector('button.post__action--repost');
  if (!like || !repost) return { found: false, like: Boolean(like), repost: Boolean(repost) };
  const out = { found: true };
  out.likeOff = read(like);
  like.classList.add('post__action--liked');
  out.likeOn = read(like);
  like.classList.remove('post__action--liked');
  out.repostOff = read(repost);
  repost.classList.add('post__action--reposted');
  out.repostOn = read(repost);
  repost.classList.remove('post__action--reposted');
  return out;
})()`);

const pink = 'rgb(236, 72, 153)';
const green = 'rgb(28, 107, 60)';
const result = {
  ...state,
  expected: { likeOnFill: pink, likeOnColor: pink, repostOnColor: green },
  pass:
    state.found === true &&
    state.likeOff.pressed === 'false' &&
    state.likeOff.fill === 'none' &&
    state.likeOn.fill === pink &&
    state.likeOn.color === pink &&
    state.repostOff.pressed === 'false' &&
    state.repostOff.color !== green &&
    state.repostOn.color === green &&
    // The repost glyph is arrows, not a shape: it tints, it never fills.
    state.repostOn.fill === 'none',
};
console.log('reaction-state:', JSON.stringify(result, null, 2));
ws.close();
process.exit(result.pass ? 0 : 1);

