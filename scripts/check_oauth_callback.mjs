// Exercise the callback half of OAuth without an account.
//
// A forged callback with a bogus code cannot succeed — but it *can* prove the machinery is
// wired: the pending attempt is read, `state` is checked, the issuer is checked, the token
// request goes out with a DPoP proof, and the server's refusal is shown to the user instead
// of being swallowed. Those are the failure modes that otherwise look like "sign-in does
// nothing", so they are worth a test that runs on every change.
//
// Usage: node scripts/check_oauth_callback.mjs <base-url>
const base = process.argv[2];
if (!base) {
  console.error('check_oauth_callback: give the base URL of an OAuth-enabled build');
  process.exit(1);
}
const baseUrl = new URL(base.endsWith('/') ? base : `${base}/`);
const callbackUrl = new URL('oauth/callback/', baseUrl).toString();

const targets = (await (await fetch('http://127.0.0.1:9222/json/list')).json()).filter((t) => t.type === 'page');
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
  (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.result?.value;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

await send('Runtime.enable');
await send('Page.enable');
await send('Page.bringToFront');

const boot = async () => {
  await send('Page.navigate', { url: 'about:blank' });
  await wait(300);
  await send('Page.navigate', { url: base });
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (await run("Boolean(document.querySelector('.app-header'))")) break;
    await wait(300);
  }
  // The DPoP helper is installed on mount; wait for it before asking for a key.
  for (let attempt = 0; attempt < 20; attempt += 1) {
    if (await run("typeof window.__rustyDpop === 'object'")) break;
    await wait(150);
  }
};

await boot();
const key = await run('(async () => JSON.stringify(await window.__rustyDpop.loadOrCreate()))()');
const { thumbprint, jwk } = JSON.parse(key);

/// Writes a pending attempt and a callback, then reloads so `complete()` runs on mount.
const stage = async (callback) => {
  const pendingAttempt = {
    state: 'probe-state',
    verifier: 'probe-verifier-that-is-long-enough-to-be-plausible-000',
    issuer: 'https://bsky.social',
    did: 'did:plc:z72i7hdynmk6r22z27h6tvur',
    pds: 'https://bsky.social',
    clientId: 'http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%2Foauth%2Fcallback',
    redirectUri: 'http://127.0.0.1/oauth/callback',
    tokenEndpoint: 'https://bsky.social/oauth/token',
    revocationEndpoint: 'https://bsky.social/oauth/revoke',
    thumbprint,
    jwk,
    remember: false,
  };
  // Only the pending attempt is written by hand: the callback itself goes through the real
  // page, so this checks the handoff (park the payload, forward into the app) as well as the
  // exchange. A callback page that prints instead of forwarding stranding a real sign-in is
  // exactly the bug this catches.
  await run(`(() => {
    sessionStorage.setItem('rusty.oauth.pending', ${JSON.stringify(JSON.stringify(pendingAttempt))});
  })()`);
  await send('Page.navigate', { url: 'about:blank' });
  await wait(200);
  const query = new URLSearchParams(callback).toString();
  await send('Page.navigate', { url: `${callbackUrl}?${query}` });
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const error = await run("document.querySelector('.login-error')?.textContent?.trim() ?? null");
    if (error) return error;
    await wait(300);
  }
  return null;
};

// 1. A code the server will refuse: the exchange happens and its refusal is reported.
const refused = await stage({ code: 'not-a-real-code', state: 'probe-state', iss: 'https://bsky.social' });
// 2. A callback from another attempt: refused locally, before any request.
const mismatched = await stage({ code: 'x', state: 'some-other-state', iss: 'https://bsky.social' });
// 3. A callback from another issuer: refused locally too.
const wrongIssuer = await stage({ code: 'x', state: 'probe-state', iss: 'https://evil.example' });

const signedIn = await run("Boolean(document.querySelector('[data-action=\"rail-sign-out\"]'))");
await run("sessionStorage.clear()");

const result = {
  base,
  exchange_refusal: refused,
  state_mismatch: mismatched,
  issuer_mismatch: wrongIssuer,
  signed_in: signedIn,
  pass:
    typeof refused === 'string' &&
    /refused|invalid/i.test(refused) &&
    typeof mismatched === 'string' &&
    /state/i.test(mismatched) &&
    typeof wrongIssuer === 'string' &&
    /evil\.example/.test(wrongIssuer) &&
    signedIn === false,
};
console.log('oauth-callback:', JSON.stringify(result, null, 2));
ws.close();
process.exitCode = result.pass ? 0 : 1;
