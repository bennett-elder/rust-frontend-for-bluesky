// The per-post menu: two share links, and no mute/block without a session.
//
// The two links are the point. Ours carries the at-uri and a `#`; the bsky.app one is
// handle + record key, which is the only form the official app and link unfurlers read.
//
// Usage: node scripts/check_post_menu.mjs [base-url]
const base = process.argv[2] ?? 'http://127.0.0.1:8099/';
const targets = (await (await fetch('http://localhost:9222/json/list')).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } };
await new Promise((r) => (ws.onopen = r));
const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
const run = async (expr) => { const m = await send('Runtime.evaluate', { expression: expr, returnByValue: true }); if (m.result?.exceptionDetails) return { threw: m.result.exceptionDetails.exception?.description }; return m.result?.result?.value; };
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
await send('Runtime.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');
await send('Page.navigate', { url: 'about:blank' });
await wait(400);
await send('Page.navigate', { url: base + '#/feed/at%3A%2F%2Fdid%3Aplc%3Az72i7hdynmk6r22z27h6tvur%2Fapp.bsky.feed.generator%2Fwhats-hot' });
await wait(8000);
// Share is always offered; the context menu only exists with a session, because
// everything in it (mute, block) needs one.
const closed = await run(`({
  shareToggles: document.querySelectorAll('[data-action="share-menu"]').length,
  menuToggles: document.querySelectorAll('[data-action="post-menu"]').length,
  open: document.querySelectorAll('.post__menu').length,
})`);
await run(`document.querySelector('[data-action="share-menu"]').click()`);
await wait(600);
const open = await run(`(() => {
  const internal = document.querySelector('[data-action="menu-copy-internal"]');
  const external = document.querySelector('[data-action="menu-copy-bsky"]');
  return {
    items: [...document.querySelectorAll('.post__menu-item')].map((b) => b.textContent.trim()),
    internal: internal ? internal.getAttribute('title') : null,
    external: external ? external.getAttribute('title') : null,
    mute: Boolean(document.querySelector('[data-action="menu-mute"]')),
    block: Boolean(document.querySelector('[data-action="menu-block"]')),
    role: document.querySelector('.post__menu')?.getAttribute('role'),
  };
})()`);
await run(`document.querySelector('[data-action="menu-copy-bsky"]').click()`);
await wait(500);
const copied = await run(`document.querySelector('.post__menu-note')?.textContent.trim() ?? null`);

// The context menu signed out: "Copy post text" and nothing that needs a session.
await run(`document.querySelector('[data-action="share-menu"]').click()`);
await wait(200);
await run(`document.querySelector('[data-action="post-menu"]').click()`);
await wait(500);
const context = await run(`(() => ({
  items: [...document.querySelectorAll('.post__menu-item')].map((b) => b.textContent.trim()),
  mute: Boolean(document.querySelector('[data-action="menu-mute"]')),
  block: Boolean(document.querySelector('[data-action="menu-block"]')),
}))()`);

// Copying the text must say that it happened, like the share items do.
await run(`document.querySelector('[data-action="menu-copy-text"]').click()`);
await wait(500);
const contextNote = await run(`document.querySelector('.post__menu-note')?.textContent.trim() ?? null`);

// Dismissal: a click outside, Escape, and opening another menu must all close it.
const openCount = () => run("document.querySelectorAll('.post__menu').length");
const dismiss = {};
// Click on the page background, not on a menu.
await run(`document.querySelector('.app-header').click()`);
await wait(500);
dismiss.afterOutsideClick = await openCount();
// Reopen, then Escape.
await run(`document.querySelector('[data-action="post-menu"]').click()`);
await wait(400);
dismiss.reopened = await openCount();
await run(`document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`);
await wait(500);
dismiss.afterEscape = await openCount();
// One menu at a time: open the context menu, then the share menu beside it.
await run(`document.querySelector('[data-action="post-menu"]').click()`);
await wait(300);
await run(`document.querySelector('[data-action="share-menu"]').click()`);
await wait(500);
dismiss.afterSwitching = await openCount();
// Deleting is only ever offered on your own posts. Signed out the whole signed-in group is
// absent, but assert the specific action: "Delete post" on a stranger's post is the kind of
// mistake a refactor makes quietly.
const delete_on_foreign_post = await run(
  `Boolean(document.querySelector('[data-action="menu-delete-post"]'))`,
);

const pass =
  closed.shareToggles > 0 &&
  closed.menuToggles > 0 &&
  closed.open === 0 &&
  open.items.length === 2 &&
  open.items[0] === 'Copy link to this app' &&
  open.items[1] === 'Copy bsky.app link' &&
  String(open.internal).startsWith('#/post/') &&
  String(open.external).startsWith('https://bsky.app/profile/') &&
  open.external.includes('/post/') &&
  open.mute === false &&
  open.block === false &&
  open.role === 'menu' &&
  /Copied/.test(copied ?? '') &&
  context.items.length === 1 &&
  context.items[0] === 'Copy post text' &&
  /Copied post text/.test(contextNote ?? '') &&
  context.mute === false &&
  context.block === false &&
  dismiss.afterOutsideClick === 0 &&
  dismiss.reopened === 1 &&
  dismiss.afterEscape === 0 &&
  dismiss.afterSwitching === 1 &&
  delete_on_foreign_post === false;
console.log('post-menu:', JSON.stringify({ closed, open, copied, context, contextNote, dismiss, delete_on_foreign_post, pass }, null, 2));
process.exit(pass ? 0 : 1);
ws.close();
