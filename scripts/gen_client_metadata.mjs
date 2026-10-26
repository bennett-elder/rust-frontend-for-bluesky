// Publish this deployment's OAuth client metadata.
//
// Atproto has no app registry to sign up to: the `client_id` *is* a URL, and the
// authorisation server fetches the document at that URL the first time someone signs in.
// "Registering an app" therefore means "serve a JSON file at a stable https URL", and the
// file must name its own URL and the exact redirect URIs. Nothing is issued, nothing is
// approved, and there is no client secret — which is why this is the only per-deployment
// artefact a fork has to get right, and why it can be generated instead of hand-written.
//
// The URL matters because it is baked in twice: as `client_id`, and inside `redirect_uris`.
// GitHub Pages gives a project page `/<repo>/` and a custom domain `/`, so the same build
// cannot infer it — pass it in (CI derives it, a custom domain sets SITE_URL).
//
// Usage:
//   node scripts/gen_client_metadata.mjs <site-url> [dist]      # publish metadata + callback
//   node scripts/gen_client_metadata.mjs --loopback <port> [dist]
//
// `--loopback` publishes no metadata at all — the PDS reads a loopback client straight from
// the query string — and only drops the callback page into a dev build, so a local redirect
// has somewhere to land at http://127.0.0.1:<port>/oauth/callback.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const SCOPE = 'atproto transition:generic';
const defaultDist = 'target/dx/bluesky-alt-first-app/release/web/public';

/// The redirect lands on a real file, not the app root: a static host serves nothing at a
/// path that does not exist, and Pages needs a directory index to serve this one. Keeping it
/// out of index.html means the app's own URL never carries an authorisation code.
function writeCallback(dist) {
  const dir = join(dist, 'oauth', 'callback');
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, 'index.html'),
    `<!doctype html>
<meta charset="utf-8">
<title>Signing in…</title>
<body>
<p>Signing in…</p>
<script>
  // Only the query is read; the fragment is never trusted here.
  const params = new URLSearchParams(location.search);
  const payload = {};
  for (const key of ['code', 'state', 'iss', 'error', 'error_description']) {
    const value = params.get(key);
    if (value !== null) payload[key] = value;
  }
  try {
    sessionStorage.setItem('rusty.oauth.callback', JSON.stringify(payload));
  } catch {
    // Private mode: fall through with the params in the query instead.
  }
  // Hand off to the app, which finishes the exchange with the DPoP key it generated before
  // the redirect. This used to print the payload on loopback instead — a leftover from when
  // the exchange did not exist, and it stranded real sign-ins on a dead-end page.
  // replace() drops the code from the address bar and from history.
  location.replace('../../#/oauth-callback');
</script>
`,
  );
  return join(dir, 'index.html');
}

const loopback = process.argv[2] === '--loopback';

if (loopback) {
  const port = process.argv[3] ?? '8099';
  const dist = process.argv[4] ?? defaultDist;
  const redirectUri = `http://127.0.0.1:${port}/oauth/callback`;
  const file = writeCallback(dist);
  console.log(
    `gen_client_metadata: loopback, no metadata file\n` +
      `  client_id     http://localhost?redirect_uri=${encodeURIComponent(redirectUri)}&scope=${encodeURIComponent(SCOPE)}\n` +
      `  redirect_uris ${redirectUri}\n` +
      `  wrote         ${file}`,
  );
} else {
  const raw = process.argv[2] ?? process.env.SITE_URL;
  const dist = process.argv[3] ?? defaultDist;
  if (!raw) {
    console.error('gen_client_metadata: give the deployed site URL, e.g. https://user.github.io/repo/');
    console.error('  or use --loopback <port> for local development');
    process.exitCode = 1;
  } else {
    // Exactly one trailing slash, so `<site>oauth-client-metadata.json` concatenates cleanly
    // and `client_id` compares equal to the URL the server will really fetch.
    const site = raw.endsWith('/') ? raw : `${raw}/`;
    const url = new URL(site);
    if (url.search || url.hash) {
      console.error(`gen_client_metadata: ${raw} has a query or fragment; the site URL is a bare origin + path`);
      process.exitCode = 1;
    } else if (url.protocol !== 'https:') {
      // Only https is publishable. Local development uses the loopback client instead, whose
      // metadata is the query string — so a loopback URL here is an error, not a broken file.
      console.error(
        `gen_client_metadata: ${raw} is not https.\n` +
          `  for local development: node scripts/gen_client_metadata.mjs --loopback 8099`,
      );
      process.exitCode = 1;
    } else {
      const clientId = `${site}oauth-client-metadata.json`;
      const redirectUri = `${site}oauth/callback/`;
      const metadata = {
        client_id: clientId,
        client_name: process.env.APP_NAME?.trim() || 'Rusty',
        client_uri: site,
        // `atproto` is mandatory and means "authenticate me"; `transition:generic` keeps the
        // familiar full read/write scope until granular scopes are wired into the UI.
        scope: SCOPE,
        redirect_uris: [redirectUri],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        dpop_bound_access_tokens: true,
        application_type: 'web',
        token_endpoint_auth_method: 'none',
      };

      mkdirSync(dist, { recursive: true });
      const file = join(dist, 'oauth-client-metadata.json');
      writeFileSync(file, `${JSON.stringify(metadata, null, 2)}\n`);
      writeCallback(dist);

      console.log(
        `gen_client_metadata: ok\n  client_id     ${clientId}\n  redirect_uris ${redirectUri}\n  wrote         ${file}`,
      );
    }
  }
}
