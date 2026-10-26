import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';

// End-to-end check of the alt-text-first behaviour against the built app.
// Usage: node scripts/verify_dom.mjs [url]
// CHROME=... overrides the browser; defaults to Playwright's Chrome for Testing.
const url = process.argv[2] ?? 'http://127.0.0.1:8099/';
const home = process.env.HOME ?? '';
const candidates = [
  `${home}/Library/Caches/ms-playwright/chromium-1217/chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`,
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
];
const chrome = process.env.CHROME ?? candidates.find((path) => existsSync(path));
if (!chrome) throw new Error('no Chrome binary found; set CHROME=...');

// This Chrome build validates the Host header, so talk to it as "localhost".
const cdp = 'http://localhost:9222';

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function json(path) {
  const res = await fetch(`${cdp}${path}`);
  return res.json();
}

async function connect() {
  for (let attempt = 0; attempt < 40; attempt++) {
    try {
      const targets = await json('/json/list');
      const page = targets.find((t) => t.type === 'page');
      if (page) return page.webSocketDebuggerUrl;
    } catch {
      // not up yet
    }
    await wait(250);
  }
  throw new Error('could not reach Chrome DevTools on :9222');
}

/** Launch a headless browser if one isn't already listening. */
async function ensureBrowser() {
  try {
    await json('/json/version');
    return;
  } catch {
    // fall through and start our own
  }
  const profile = `${process.cwd()}/.tmp/chrome-profile`;
  spawn(
    chrome,
    [
      '--headless=new',
      '--no-sandbox',
      '--disable-gpu',
      '--mute-audio',
      // Avoid the macOS "Chrome Safe Storage" keychain prompt, which blocks
      // headless startup (and can leave the profile unreadable).
      '--use-mock-keychain',
      '--password-store=basic',
      '--autoplay-policy=no-user-gesture-required',
      '--remote-debugging-port=9222',
      `--user-data-dir=${profile}`,
      'about:blank',
    ],
    { detached: true, stdio: 'ignore' },
  ).unref();
  await connect();
}

await ensureBrowser();
const wsUrl = await connect();
const ws = new WebSocket(wsUrl);
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

const evaluate = async (expression) => {
  const result = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.result?.exceptionDetails) {
    throw new Error(result.result.exceptionDetails.text);
  }
  return result.result.result.value;
};

const snapshot = `(() => {
  const gates = [...document.querySelectorAll('.alt-first__gate')];
  return {
    posts: document.querySelectorAll('.post').length,
    avatars: document.querySelectorAll('.avatar').length,
    images: document.querySelectorAll('.embed img').length,
    videos: document.querySelectorAll('.embed video').length,
    missingAltBoxes: document.querySelectorAll('.alt-first__text--missing').length,
    actionIcons: document.querySelectorAll('.post__actions svg.icon').length,
    repostLabels: document.querySelectorAll('.post__repost').length,
    quotes: document.querySelectorAll('.quote').length,
    clickablePosts: document.querySelectorAll('.post[data-post-link]').length,
    gates: gates.slice(0, 3).map((g) => g.textContent.trim().slice(0, 90)),
  };
})()`;

await send('Page.enable');
await send('Runtime.enable');
await send('Page.navigate', { url });
await wait(4000);

const before = await evaluate(snapshot);
console.log('collapsed:', JSON.stringify(before, null, 2));

// Pick a deterministic target: the first IMAGE embed, not a link thumbnail.
const targetIndex = await evaluate(`(() => {
  const gates = [...document.querySelectorAll('.alt-first__gate')];
  let index = gates.findIndex((g) => g.textContent.trim().startsWith('Image'));
  if (index < 0) index = gates.findIndex((g) => g.querySelector('.alt-first__kicker'));
  if (index < 0) return -1;
  const figure = gates[index].closest('.alt-first');
  if (!figure) return -1;
  figure.dataset.testTarget = '1';
  return index;
})()`);
if (targetIndex < 0) {
  throw new Error('no media embed in this feed; pass a URL with ?actor=<media-rich handle>');
}

// Tap the alt box: image should mount and actually decode.
await evaluate(`document.querySelector('[data-test-target] .alt-first__gate').click()`);
await wait(2500);

const revealed = await evaluate(`(() => {
  const img = document.querySelector('[data-test-target] img');
  return {
    images: document.querySelectorAll('[data-test-target] img').length,
    src: img?.getAttribute('src')?.slice(0, 80) ?? null,
    naturalWidth: img?.naturalWidth ?? 0,
    caption: document.querySelector('[data-test-target] .alt-first__caption')?.textContent?.slice(0, 60) ?? null,
    closeButtons: document.querySelectorAll('[data-test-target] .alt-first__close').length,
  };
})()`);
console.log('revealed:', JSON.stringify(revealed, null, 2));

// ALT pill: redundant in alt-first mode — the description is already on screen.
const pillInAltFirst = await evaluate(
  `document.querySelectorAll('[data-test-target] .alt-first__alt-pill').length`,
);
console.log(
  'alt pill (alt-first mode):',
  JSON.stringify({ pills: pillInAltFirst, expected: 0 }),
);

// Media-first mode: uncheck the preference; embeds reveal themselves with the
// caption hidden, which is the only situation where the pill earns its place.
await evaluate(`document.querySelector('.toggle input').click()`);
await wait(1500);
const mediaFirst = await evaluate(`(() => ({
  pills: document.querySelectorAll('.alt-first__alt-pill').length,
  captions: document.querySelectorAll('.alt-first__caption').length,
  mediaImgs: document.querySelectorAll('.alt-first__media').length,
}))()`);
console.log('media-first mode:', JSON.stringify(mediaFirst, null, 2));
await evaluate(`document.querySelector('.toggle input').click()`);
await wait(1000);

// Close it: should return to the alt box and drop the <img> from the DOM.
await evaluate(`document.querySelector('[data-test-target] .alt-first__close').click()`);
await wait(800);

const collapsedAgain = await evaluate(snapshot);
console.log('collapsed again:', JSON.stringify(collapsedAgain, null, 2));

// Video path: alt box -> poster -> playing (HLS attached by the page shim).
// Skipped when the chosen feed has no video embed.
const hasVideo = await evaluate(
  `Boolean(document.querySelector('.alt-first--video .alt-first__gate'))`,
);
if (hasVideo) {
  await evaluate(`document.querySelector('.alt-first--video .alt-first__gate').click()`);
  await wait(1200);
  const poster = await evaluate(`(() => {
    const img = document.querySelector('.alt-first--video img');
    return { poster: img?.getAttribute('src')?.slice(0, 70) ?? null, naturalWidth: img?.naturalWidth ?? 0 };
  })()`);
  console.log('video poster:', JSON.stringify(poster, null, 2));

  await evaluate(`document.querySelector('.alt-first--video .alt-first__play').click()`);
  await wait(6000);

  const playing = await evaluate(`(() => {
    const video = document.querySelector('.alt-first--video video');
    return {
      videos: document.querySelectorAll('.alt-first--video video').length,
      hlsAttached: video?.dataset.hlsAttached ?? null,
      src: video?.currentSrc?.slice(0, 70) ?? null,
      readyState: video?.readyState ?? null,
      networkState: video?.networkState ?? null,
      paused: video?.paused ?? null,
      currentTime: video?.currentTime ?? null,
      error: video?.error?.code ?? null,
    };
  })()`);
  console.log('video playing:', JSON.stringify(playing, null, 2));
} else {
  console.log('video playing: skipped (no video embed in this feed)');
}

// Windowing check: off-screen posts must keep their DOM nodes (so find-in-page,
// selection, and screen readers still see them) while skipping layout/paint.
const firstPostText = await evaluate(
  `(document.querySelector('.post__text')?.textContent ?? '').trim().slice(0, 16)`,
);
await evaluate(`window.scrollTo(0, document.body.scrollHeight)`);
await wait(600);
const windowing = await evaluate(`(() => {
  const posts = [...document.querySelectorAll('.post')];
  const last = posts[posts.length - 1];
  const probe = ${JSON.stringify(firstPostText)};
  return {
    postNodes: posts.length,
    lastInDom: Boolean(last),
    lastRenderedHeight: last?.getBoundingClientRect().height ?? null,
    contentVisibilityApplied: posts.filter((p) => getComputedStyle(p).contentVisibility === 'auto').length,
    firstPostStillSelectable: Boolean(posts[0]?.textContent?.length),
    offscreenProbe: probe,
    textFindableInnerText: probe.length > 0 && document.body.innerText.includes(probe),
    textFindableTextContent: probe.length > 0 && document.body.textContent.includes(probe),
  };
})()`);
console.log('windowing:', JSON.stringify(windowing, null, 2));

// Facets: the current feed should render mentions/links/tags as anchors. Run
// against an account with rich text (?actor=jay.bsky.team) for full coverage.
const facets = await evaluate(`(() => {
  const links = [...document.querySelectorAll('.post__text a.facet')];
  const byKind = {};
  for (const link of links) {
    const kind = [...link.classList].find((c) => c.startsWith('facet--')) ?? 'facet--?';
    byKind[kind] = (byKind[kind] ?? 0) + 1;
  }
  return {
    posts: document.querySelectorAll('.post').length,
    facets: links.length,
    byKind,
    samples: links.slice(0, 4).map((a) => ({
      text: a.textContent.slice(0, 32),
      href: (a.getAttribute('href') ?? '').slice(0, 58),
    })),
    textIntact: document.querySelector('.post__text')?.textContent?.length > 0 ?? false,
  };
})()`);
console.log('facets:', JSON.stringify(facets, null, 2));

// Thread view: the permalink (date) opens getPostThread; clicking the body does
// the same through the delegated listener.
await evaluate(`document.querySelector('.post__permalink').click()`);
await wait(4500);
const thread = await evaluate(`(() => ({
  back: Boolean(document.querySelector('.thread__back')),
  threadNodes: document.querySelectorAll('.thread-node').length,
  postsInThread: document.querySelectorAll('.thread .post').length,
  missingPlaceholders: document.querySelectorAll('.thread-missing').length,
  facetsInThread: document.querySelectorAll('.thread .facet').length,
  mediaLoaded: document.querySelectorAll('.thread .alt-first__media').length,
  selfLinks: document.querySelectorAll('.thread .post[data-post-link]').length,
}))()`);
console.log('thread:', JSON.stringify(thread, null, 2));

await evaluate(`document.querySelector('.thread__back').click()`);
await wait(1500);
const backToFeed = await evaluate(`({
  feedPosts: document.querySelectorAll('.post').length,
  threadOpen: Boolean(document.querySelector('.thread')),
})`);
console.log('back to feed:', JSON.stringify(backToFeed, null, 2));

// Feed tabs: ALT4Me is public; Following must fail honestly when logged out.
await evaluate(
  `[...document.querySelectorAll('.feed-tab')].find((b) => b.textContent.includes('ALT4Me')).click()`,
);
await wait(4000);
const alt4me = await evaluate(`({
  posts: document.querySelectorAll('.post').length,
  firstGate: document.querySelector('.alt-first__gate')?.textContent?.trim().slice(0, 60) ?? null,
  error: document.querySelector('.error')?.textContent ?? null,
})`);
console.log('alt4me tab:', JSON.stringify(alt4me, null, 2));

await evaluate(
  `[...document.querySelectorAll('.feed-tab')].find((b) => b.textContent.includes('Following')).click()`,
);
await wait(2500);
const following = await evaluate(`({
  error: document.querySelector('.error')?.textContent ?? null,
  loginFormOpen: Boolean(document.querySelector('.login-form')),
  rememberToggle: Boolean(document.querySelector('.login-remember')),
  posts: document.querySelectorAll('.post').length,
})`);
console.log('following tab (logged out):', JSON.stringify(following, null, 2));

// Pagination: Load more appends the next page via the cursor.
const beforeMore = await evaluate(`document.querySelectorAll('.post').length`);
const hadLoadMore = await evaluate(`Boolean(document.querySelector('.load-more'))`);
if (hadLoadMore) {
  await evaluate(`document.querySelector('.load-more').click()`);
  await wait(4500);
}
const afterMore = await evaluate(`({
  posts: document.querySelectorAll('.post').length,
  hasMore: Boolean(document.querySelector('.load-more')),
  end: document.querySelector('.feed-end')?.textContent ?? null,
})`);
console.log(
  'pagination:',
  JSON.stringify({ hadLoadMore, before: beforeMore, after: afterMore }, null, 2),
);

// Profile route: banner, counts, and the new profile-links feature.
await evaluate(`location.hash = '#/profile/bsky.app'`);
await wait(5000);
const profile = await evaluate(`(() => ({
  name: document.querySelector('.profile__name')?.textContent?.trim() ?? null,
  links: [...document.querySelectorAll('.profile__link')].map((a) => a.textContent.trim()),
  posts: document.querySelectorAll('.profile .post').length,
  banner: Boolean(document.querySelector('.profile__banner img, img.profile__banner')),
  stats: document.querySelector('.profile__stats')?.textContent ?? null,
  hash: location.hash,
}))()`);
console.log('profile route:', JSON.stringify(profile, null, 2));

ws.close();
process.exit(0);
