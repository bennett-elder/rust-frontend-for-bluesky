// Does the built artifact work both ways GitHub Pages serves it?
//
//   project page  https://<user>.github.io/<repo>/   (a fork, subdirectory)
//   custom domain https://example.com/               (root)
//
// A leading slash in an asset URL satisfies only one of those, and the failure is silent: a
// blank page with 404s in a console nobody is watching. This serves the real build in both
// positions and asserts that the app boots with no failed requests.
//
// The two cases get **separate server roots** on purpose. An earlier version copied the
// artifact into both `<repo>/` and the server root, so a root-absolute `/assets/<wasm>` was
// answered by the copy at the root — the check passed while the deployed site came up blank.
// The bundle matters more than the HTML here: dx fetches the wasm from inside the JS, where a
// leading slash never appears in `index.html`.
//
// Usage: node scripts/check_pages_subpath.mjs [dist] [repo-name]
import { cpSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { extname, join } from 'node:path';

const dist = process.argv[2] ?? 'target/dx/bluesky-alt-first-app/release/web/public';
const repo = process.argv[3] ?? 'a-fork-with-another-name';
const port = 8098;
const subRoot = '.tmp/pages-sub';
const rootRoot = '.tmp/pages-root';

rmSync(subRoot, { recursive: true, force: true });
rmSync(rootRoot, { recursive: true, force: true });
cpSync(dist, `${subRoot}/${repo}`, { recursive: true });
cpSync(dist, rootRoot, { recursive: true });

// Switched per visit, so neither case can be rescued by files from the other.
let serveRoot = subRoot;

const html = readFileSync(`${dist}/index.html`, 'utf8');
const absolute = [...html.matchAll(/(?:href|src)="\/[^"]*"/g)].map((match) => match[0]);
const absoluteInBundle = readdirSync(`${dist}/assets`)
  .filter((entry) => entry.endsWith('.js'))
  .flatMap((entry) =>
    [
      ...readFileSync(`${dist}/assets/${entry}`, 'utf8').matchAll(/["'`]\/\.?\/?assets\/[^"'`]*/g),
    ].map(
      (match) => `${entry}: ${match[0].slice(0, 48)}`,
    ),
  );

const mime = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.wasm': 'application/wasm',
  '.css': 'text/css',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
};
const server = createServer(async (request, response) => {
  const path = decodeURIComponent(request.url.split('?')[0]);
  const file = join(serveRoot, path.endsWith('/') ? `${path}index.html` : path);
  try {
    const body = await readFile(file);
    response.writeHead(200, { 'content-type': mime[extname(file)] ?? 'application/octet-stream' });
    response.end(body);
  } catch {
    response.writeHead(404, { 'content-type': 'text/plain' });
    response.end('not found');
  }
});
await new Promise((resolve) => server.listen(port, resolve));

const targets = (await (await fetch('http://localhost:9222/json/list')).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(targets[0].webSocketDebuggerUrl);
const pending = new Map();
let id = 0;
let failures = [];
ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  if (message.method === 'Network.responseReceived' && message.params.response.status >= 400) {
    failures.push(`${message.params.response.status} ${message.params.response.url}`);
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
await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
await send('Page.enable');

const visit = async (url) => {
  failures = [];
  await send('Page.navigate', { url: 'about:blank' });
  await wait(350);
  await send('Page.navigate', { url });
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (await run("Boolean(document.querySelector('.app-header'))")) break;
    await wait(400);
  }
  await wait(1500);
  return {
    url,
    title: await run('document.title'),
    booted: await run("Boolean(document.querySelector('.app-header'))"),
    failed: failures.slice(0, 6),
  };
};

serveRoot = subRoot;
const subpath = await visit(`http://127.0.0.1:${port}/${repo}/`);
// A deep link under the project path: Pages 404s unknown paths, but hash routing means there
// is no unknown path — the fragment never reaches the server.
const deepLink = await visit(`http://127.0.0.1:${port}/${repo}/#/profile/bsky.app`);
const deepRendered = await run("document.querySelector('.profile__name')?.textContent?.trim() ?? null");
// The icon is linked relatively like everything else; a missing one is a silent 404 in a
// console, so fetch it explicitly from the subpath position.
const iconStatus = await run(`fetch('./favicon.ico').then((r) => r.status).catch(() => 0)`);
serveRoot = rootRoot;
const atRoot = await visit(`http://127.0.0.1:${port}/`);
server.close();

const result = {
  absolute_asset_urls: absolute,
  absolute_in_bundle: absoluteInBundle,
  subpath,
  root: atRoot,
  deepLink,
  deepRendered,
  icon_status: iconStatus,
  pass:
    absolute.length === 0 &&
    absoluteInBundle.length === 0 &&
    subpath.booted &&
    subpath.failed.length === 0 &&
    atRoot.booted &&
    atRoot.failed.length === 0 &&
    deepLink.booted &&
    deepLink.failed.length === 0 &&
    typeof deepRendered === 'string' &&
    iconStatus === 200,
};
console.log('pages-subpath:', JSON.stringify(result, null, 2));
ws.close();
process.exitCode = result.pass ? 0 : 1;
