// Run the first half of the AT Protocol OAuth flow against the real network, locally.
//
// Everything up to the user's consent can be exercised without an account: handle → DID →
// PDS → authorisation-server metadata, then a Pushed Authorization Request signed with a
// DPoP proof and protected by PKCE. A `request_uri` back from PAR means the server accepted
// our client_id, redirect URI, scope, key and challenge — which is exactly the set of things
// that fail later, at sign-in, with an unhelpful error. Nothing here needs a browser.
//
// Usage: node scripts/probe_oauth.mjs <handle> [--port 8099] [--scope "atproto transition:generic"]
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

const handle = process.argv[2];
if (!handle) {
  console.error('probe_oauth: give a handle, e.g. node scripts/probe_oauth.mjs bsky.app');
  process.exit(1);
}
const portFlag = process.argv.indexOf('--port');
const port = portFlag === -1 ? 8099 : process.argv[portFlag + 1];
const scopeFlag = process.argv.indexOf('--scope');
const scope = scopeFlag === -1 ? 'atproto transition:generic' : process.argv[scopeFlag + 1];

const b64 = (value) => Buffer.from(value).toString('base64url');
const json = async (url) => {
  const response = await fetch(url, { headers: { accept: 'application/json' } });
  const text = await response.text();
  let body = null;
  try {
    body = JSON.parse(text);
  } catch {
    body = text.slice(0, 200);
  }
  return { status: response.status, body };
};

const trace = {};

// 1. Handle → DID. The AppView resolves handles; the DID document then names the PDS.
trace.handle = (await json(`https://api.bsky.app/xrpc/com.atproto.identity.resolveHandle?handle=${encodeURIComponent(handle)}`)).body;
if (!trace.handle?.did) {
  console.error('probe_oauth: could not resolve the handle:', trace.handle);
  process.exit(1);
}
const did = trace.handle.did;

// 2. DID → PDS. `did:plc` lives at plc.directory; `did:web` is the host itself.
const didDocumentUrl = did.startsWith('did:plc:')
  ? `https://plc.directory/${did}`
  : `https://${did.slice('did:web:'.length).replace(/%3A/g, ':')}/.well-known/did.json`;
const doc = (await json(didDocumentUrl)).body;
const pds = doc?.service?.find((entry) => entry.id === '#atproto_pds')?.serviceEndpoint;
if (!pds) {
  console.error('probe_oauth: DID document has no #atproto_pds service:', doc);
  process.exit(1);
}
trace.did = did;
trace.pds = pds;

// 3. PDS → authorisation server. The spec route is `oauth-protected-resource`; bsky.social
// currently answers 404 there, so fall back to asking the PDS for its own AS metadata, which
// is what an entryway serves when it *is* the authorisation server.
const resource = await json(`${pds}/.well-known/oauth-protected-resource`);
trace.protected_resource_status = resource.status;
const authorizationServers = resource.body?.authorization_servers ?? [];
trace.authorization_servers = authorizationServers;
const asIssuer = authorizationServers[0] ?? null;
const asUrl = `${asIssuer ?? pds}/.well-known/oauth-authorization-server`;
const as = (await json(asUrl)).body;
if (!as?.pushed_authorization_request_endpoint) {
  console.error(`probe_oauth: no PAR endpoint in ${asUrl}:`, as);
  process.exit(1);
}
trace.authorization_server = as.issuer;

// 4. A throwaway ES256 key, the same shape the browser will hold. `ieee-p1363` is raw R||S,
// which is what JWS wants — node's default DER encoding would be silently invalid here.
const { publicKey, privateKey } = generateKeyPairSync('ec', { namedCurve: 'P-256' });
const jwk = publicKey.export({ format: 'jwk' });
const thumbprintInput = `{"crv":"${jwk.crv}","kty":"${jwk.kty}","x":"${jwk.x}","y":"${jwk.y}"}`;
const thumbprint = createHash('sha256').update(thumbprintInput).digest('base64url');
trace.dpop_key_thumbprint = thumbprint;

const dpopProof = (url, method, nonce) => {
  const header = { typ: 'dpop+jwt', alg: 'ES256', jwk: { kty: jwk.kty, crv: jwk.crv, x: jwk.x, y: jwk.y } };
  const claims = { jti: randomBytes(16).toString('base64url'), htm: method, htu: url, iat: Math.floor(Date.now() / 1000) };
  if (nonce) claims.nonce = nonce;
  const signingInput = `${b64(JSON.stringify(header))}.${b64(JSON.stringify(claims))}`;
  const signature = sign('sha256', Buffer.from(signingInput), { key: privateKey, dsaEncoding: 'ieee-p1363' });
  return `${signingInput}.${signature.toString('base64url')}`;
};

// 5. The loopback client: `client_id` is exactly http://localhost with metadata in the query
// string, and the redirect must be 127.0.0.1 (not localhost). No document is fetched.
const redirectUri = `http://127.0.0.1:${port}/oauth/callback`;
const clientId = `http://localhost?redirect_uri=${encodeURIComponent(redirectUri)}&scope=${encodeURIComponent(scope)}`;
const verifier = randomBytes(32).toString('base64url');
const challenge = createHash('sha256').update(verifier).digest('base64url');
const state = randomBytes(16).toString('base64url');

const parBody = new URLSearchParams({
  client_id: clientId,
  response_type: 'code',
  redirect_uri: redirectUri,
  scope,
  state,
  code_challenge: challenge,
  code_challenge_method: 'S256',
  login_hint: handle,
});

const par = async (nonce) => {
  const response = await fetch(as.pushed_authorization_request_endpoint, {
    method: 'POST',
    headers: {
      'content-type': 'application/x-www-form-urlencoded',
      DPoP: dpopProof(as.pushed_authorization_request_endpoint, 'POST', nonce),
    },
    body: parBody.toString(),
  });
  const text = await response.text();
  let body = text;
  try {
    body = JSON.parse(text);
  } catch {}
  return { status: response.status, nonce: response.headers.get('dpop-nonce'), body };
};

// 6. First attempt, then the nonce retry: the server hands out a nonce via `use_dpop_nonce`
// and the same request must be repeated with it. This dance is mandatory, not defensive.
let attempt = await par(null);
trace.par_first_attempt = { status: attempt.status, body: attempt.body };
if (attempt.status !== 200 && attempt.nonce) {
  attempt = await par(attempt.nonce);
  trace.par_after_nonce = { status: attempt.status, nonce_used: true, body: attempt.body };
}

const requestUri = attempt.body?.request_uri;
trace.request_uri = requestUri ?? null;
if (requestUri) {
  const authorize = new URL(as.authorization_endpoint);
  authorize.searchParams.set('client_id', clientId);
  authorize.searchParams.set('request_uri', requestUri);
  trace.authorize_url = authorize.toString();
}
trace.ok = Boolean(requestUri);
console.log('probe-oauth:', JSON.stringify(trace, null, 2));
process.exitCode = trace.ok ? 0 : 1;
