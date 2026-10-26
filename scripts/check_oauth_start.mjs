// Does "Sign in with Bluesky" really perform discovery and PAR from the browser?
//
// This is the half of OAuth that needs no account, and the half where every silent failure
// lives: the client_id must be this origin, the DPoP proof must verify, PKCE must be S256,
// and the first attempt must be retried with the server's nonce. All of that is observable
// from the navigation URL, because a successful PAR produces a `request_uri` in it.
//
// Requires a build with OAuth enabled: `OAUTH_MODE=both` (or `oauth`), at the given base URL.
// Usage: node scripts/check_oauth_start.mjs <base-url> [handle]
const base = process.argv[2];

/// A published client names itself by URL; a loopback client is exactly `http://localhost`
/// with the redirect (which carries the real host) in its query string.
function clientIdMatchesDeployment(clientId, host) {
  if (clientId.includes(host)) return true;
  if (!clientId.startsWith('http://localhost?')) return false;
  const redirect = new URL(clientId).searchParams.get('redirect_uri') ?? '';
  return redirect.includes(host) && redirect.startsWith('http://127.0.0.1');
}
const handle = process.argv[3] ?? 'bsky.app';
if (!base) {
  console.error('check_oauth_start: give the base URL of an OAuth-enabled build');
  console.error('  cd app && OAUTH_MODE=both ../.tools/dx build --release --platform web');
  process.exit(1);
}

const targets = (await (await fetch('http://127.0.0.1:9222/json/list')).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
let navigations = [];
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.method === 'Page.frameRequestedNavigation') {
    navigations.push(message.params.url);
  }
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
await send('Page.navigate', { url: 'about:blank' });
await wait(300);
await send('Page.navigate', { url: base });
for (let attempt = 0; attempt < 40; attempt += 1) {
  if (await run("Boolean(document.querySelector('.app-header'))")) break;
  await wait(300);
}

await run("document.querySelector('[data-action=\"rail-sign-in\"]')?.click()");
for (let attempt = 0; attempt < 30; attempt += 1) {
  if (await run("Boolean(document.querySelector('#oauth-handle'))")) break;
  await wait(200);
}

const button = await run(
  "Boolean(document.querySelector('[data-action=\"oauth-sign-in\"]'))",
);
const note = await run("document.querySelector('.note')?.textContent?.trim() ?? null");

// Empty handle must fail locally rather than starting a half-flow.
await run("document.querySelector('[data-action=\"oauth-sign-in\"]')?.click()");
await wait(600);
const emptyHandleError = await run("document.querySelector('.login-error')?.textContent?.trim() ?? null");

// Now a real one: type a handle and let it discover + PAR.
await run(`(() => {
  const input = document.querySelector('#oauth-handle');
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setter.call(input, ${JSON.stringify(handle)});
  input.dispatchEvent(new Event('input', { bubbles: true }));
})()`);
await wait(200);
// The failed empty-handle attempt re-renders the card; clicking again before the button is
// enabled does nothing, and the check would then report the stale error text.
for (let attempt = 0; attempt < 20; attempt += 1) {
  const enabled = await run(`document.querySelector('[data-action="oauth-sign-in"]')?.disabled === false`);
  if (enabled) break;
  await wait(150);
}
navigations = [];
await run("document.querySelector('[data-action=\"oauth-sign-in\"]')?.click()");
for (let attempt = 0; attempt < 40; attempt += 1) {
  if (navigations.length) break;
  await wait(400);
}

const target = navigations.find((url) => url.startsWith('https://bsky.social/oauth/authorize')) ?? navigations[0] ?? null;
const parsed = target ? new URL(target) : null;
const error = await run("document.querySelector('.login-error')?.textContent?.trim() ?? null");

const result = {
  base,
  handle,
  button_present: button,
  note,
  empty_handle_error: emptyHandleError,
  oauth_error: error,
  navigate_to: target,
  client_id: parsed?.searchParams.get('client_id') ?? null,
  request_uri: parsed?.searchParams.get('request_uri') ?? null,
  pass:
    button === true &&
    typeof emptyHandleError === 'string' &&
    emptyHandleError.length > 0 &&
    typeof target === 'string' &&
    target.startsWith('https://bsky.social/oauth/authorize') &&
    (parsed?.searchParams.get('request_uri') ?? '').startsWith('urn:ietf:params:oauth:request_uri:') &&
    // The client id must be this deployment's, not a hard-coded upstream one. Loopback is its
    // own shape: exactly `http://localhost` with the deployment's host inside the redirect.
    clientIdMatchesDeployment(
      parsed?.searchParams.get('client_id') ?? '',
      new URL(base).host,
    ),
};
console.log('oauth-start:', JSON.stringify(result, null, 2));
ws.close();
process.exitCode = result.pass ? 0 : 1;
