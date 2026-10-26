// Check a deployment's OAuth client metadata, before or after it is published.
//
// This exists because of one specific failure: `client_id` must be the URL of the document
// itself, and `redirect_uris` must be exactly what the deployment serves. Nothing in the
// build can catch a wrong value — the file is valid JSON either way — and the authorisation
// server only rejects it later, at sign-in, with an error that names no field. So: assert
// the file agrees with the URL it is published at, and say which value to fix when it does not.
//
// Usage:
//   node scripts/check_client_metadata.mjs https://2.willingtocode.com
//   node scripts/check_client_metadata.mjs https://you.github.io/repo/ --dist target/dx/.../public
//   (--dist checks the local file instead of fetching, which is what a pre-push run wants)
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const raw = process.argv[2];
const distFlag = process.argv.indexOf('--dist');
const dist = distFlag === -1 ? null : process.argv[distFlag + 1];
if (!raw) {
  console.error('check_client_metadata: give the deployed site URL, e.g. https://you.github.io/<repo>/');
  process.exit(1);
}

const site = raw.endsWith('/') ? raw : `${raw}/`;
const expectedClientId = `${site}oauth-client-metadata.json`;
const problems = [];

let metadata;
if (dist) {
  const file = join(dist, 'oauth-client-metadata.json');
  try {
    metadata = JSON.parse(readFileSync(file, 'utf8'));
  } catch (error) {
    console.error(`check_client_metadata: cannot read ${file} (${error.code ?? error.message})`);
    console.error('  generate it: node scripts/gen_client_metadata.mjs ' + site + ' ' + dist);
    process.exit(1);
  }
} else {
  const response = await fetch(expectedClientId);
  if (!response.ok) {
    console.error(`check_client_metadata: ${expectedClientId} → HTTP ${response.status}`);
    console.error('  Pages serves nothing at a path that does not exist, so check SITE_URL and the deploy.');
    process.exit(1);
  }
  metadata = await response.json();
}

// The whole identity of the client is this equality. A fork that kept the upstream URL
// signs in against the wrong site (or not at all).
if (metadata.client_id !== expectedClientId) {
  problems.push(
    `client_id is ${JSON.stringify(metadata.client_id)} but this deployment serves the file at ${expectedClientId}`,
  );
}

const redirects = Array.isArray(metadata.redirect_uris) ? metadata.redirect_uris : [];
if (!redirects.length) {
  problems.push('redirect_uris is missing or empty');
}
for (const uri of redirects) {
  let parsed;
  try {
    parsed = new URL(uri);
  } catch {
    problems.push(`redirect_uris entry ${JSON.stringify(uri)} is not an absolute URL`);
    continue;
  }
  // Absolute, https, and under this deployment. A redirect to another origin would send
  // the authorisation code somewhere this site cannot read.
  if (parsed.protocol !== 'https:') {
    problems.push(`redirect_uris entry ${uri} is not https`);
  }
  if (!uri.startsWith(site)) {
    problems.push(`redirect_uris entry ${uri} is not under ${site}`);
  }
}

if (metadata.dpop_bound_access_tokens !== true) {
  problems.push('dpop_bound_access_tokens must be true — without it the tokens are bearer tokens');
}
if (!Array.isArray(metadata.grant_types) || !metadata.grant_types.includes('authorization_code')) {
  problems.push('grant_types must include authorization_code');
}
if (!Array.isArray(metadata.response_types) || metadata.response_types.join(' ') !== 'code') {
  problems.push('response_types must be exactly ["code"]');
}
const scope = typeof metadata.scope === 'string' ? metadata.scope.split(/\s+/).filter(Boolean) : [];
if (!scope.includes('atproto')) {
  problems.push('scope must include atproto (the client must reject a session without it)');
}
if (metadata.token_endpoint_auth_method !== 'none') {
  problems.push('token_endpoint_auth_method must be "none" — this is a public client with no secret');
}

// The callback path has to exist as a file for Pages to serve it; a 404 here is the other
// half of the same silent failure.
let callbackStatus = null;
if (!dist && redirects[0]?.startsWith(site)) {
  try {
    callbackStatus = (await fetch(redirects[0])).status;
    if (callbackStatus !== 200) problems.push(`redirect_uris entry ${redirects[0]} → HTTP ${callbackStatus}`);
  } catch (error) {
    problems.push(`redirect_uris entry ${redirects[0]} is unreachable (${error.message})`);
  }
}

const result = {
  site,
  source: dist ?? expectedClientId,
  client_id: metadata.client_id,
  redirect_uris: redirects,
  scope: metadata.scope,
  callback_status: callbackStatus,
  problems,
  pass: problems.length === 0,
};
console.log('client-metadata:', JSON.stringify(result, null, 2));
if (problems.length) {
  console.error('\nfix: node scripts/gen_client_metadata.mjs ' + site + (dist ? ' ' + dist : ''));
}
// exitCode, not exit(): exit() truncates buffered stdout when the output is a pipe, and
// the JSON above is what a forker reads to see which field is wrong.
process.exitCode = result.pass ? 0 : 1;
