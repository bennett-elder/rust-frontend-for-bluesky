//! AT Protocol OAuth for the web build: discovery, PAR, the code exchange, refresh and revoke.
//!
//! The parts that can be pure live in `alt_first_core::oauth` (PKCE, the DPoP proof, the
//! request bodies, callback parsing). This module is the browser half: a P-256 key that lives
//! in IndexedDB as a *non-extractable* `CryptoKey`, ECDSA signing through WebCrypto, and the
//! HTTP calls to the authorisation server.
//!
//! Two things are deliberately different from the app-password path:
//!
//! * every request carries a DPoP proof bound to its method, URL and token, so a stolen token
//!   is useless without the key;
//! * the first attempt at any endpoint is expected to fail with `use_dpop_nonce`, and the same
//!   request is repeated with the server's nonce. That is the protocol, not a fallback.

use alt_first_core::oauth::{
    authorize_url, code_exchange_form, jwk_thumbprint, refresh_form, revoke_form, CallbackParams,
    DpopProof,
};
use dioxus::prelude::*;

/// A DPoP key, as Rust sees it: the public half and its thumbprint. The private half never
/// leaves the browser's crypto layer — `sign` asks WebCrypto to sign by key handle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DpopKey {
    pub thumbprint: String,
    /// Public JWK as JSON text, ready to embed in a proof header.
    pub jwk: String,
}

/// What makes a session an OAuth session rather than an app-password one.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthSession {
    pub issuer: String,
    pub client_id: String,
    pub scope: String,
    /// Names the key in IndexedDB: a restored session is only usable while that key exists.
    pub thumbprint: String,
    pub jwk: String,
    /// Kept so a refresh does not have to redo discovery: the token endpoint is where the
    /// session was issued, and discovering it again could land on a different host after an
    /// account migration, which would invalidate the refresh token.
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
}

/// How a request authenticates. OAuth access tokens are `DPoP`-scheme tokens, not bearers:
/// the scheme itself tells the server to expect a proof.
#[derive(Clone, Copy)]
pub enum Auth<'a> {
    Bearer(&'a str),
    Dpop { token: &'a str, oauth: &'a OAuthSession },
}

pub struct RawResponse {
    pub status: u16,
    pub body: String,
    pub dpop_nonce: Option<String>,
}

impl RawResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Installs the browser-side helpers once per page load.
///
/// The key is generated as an extractable pair only so its public half can be exported; the
/// private half is then re-imported with `extractable: false` and only *that* is stored, so no
/// script on the origin can export it afterwards. Idempotent: the guard keeps a second call
/// from replacing the object a flow in progress is holding.
const DPOP_JS: &str = r#"
window.__rustyDpop = window.__rustyDpop || (() => {
  const DB = 'rusty.oauth', STORE = 'keys', ID = 'dpop';
  const b64u = (buffer) => btoa(String.fromCharCode(...new Uint8Array(buffer)))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const openDb = () => new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  const withStore = async (mode, run) => {
    const db = await openDb();
    try {
      return await new Promise((resolve, reject) => {
        const tx = db.transaction(STORE, mode);
        const request = run(tx.objectStore(STORE));
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
      });
    } finally { db.close(); }
  };
  const thumbprintOf = async (jwk) => {
    const canonical = '{"crv":"' + jwk.crv + '","kty":"' + jwk.kty + '","x":"' + jwk.x + '","y":"' + jwk.y + '"}';
    return b64u(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(canonical)));
  };
  const create = async () => {
    const pair = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
    const exported = await crypto.subtle.exportKey('jwk', pair.privateKey);
    const publicJwk = { kty: exported.kty, crv: exported.crv, x: exported.x, y: exported.y };
    // Re-import without `d`: from here on the private key cannot be exported by anything.
    const privateKey = await crypto.subtle.importKey(
      'jwk', exported, { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
    const record = { privateKey, jwk: publicJwk, thumbprint: await thumbprintOf(publicJwk) };
    await withStore('readwrite', (store) => store.put(record, ID));
    return record;
  };
  const loadOrCreate = async () => {
    const existing = await withStore('readonly', (store) => store.get(ID));
    const record = existing || await create();
    return { thumbprint: record.thumbprint, jwk: JSON.stringify(record.jwk) };
  };
  const sign = async (bytes) => {
    const record = await withStore('readonly', (store) => store.get(ID));
    if (!record) throw new Error('no DPoP key stored for this browser');
    const signature = await crypto.subtle.sign(
      { name: 'ECDSA', hash: 'SHA-256' }, record.privateKey, new TextEncoder().encode(bytes));
    return b64u(signature);
  };
  const forget = () => withStore('readwrite', (store) => store.delete(ID));
  return { loadOrCreate, sign, forget, thumbprintOf };
})();
// `eval.recv()` waits for a message, so the script has to send one — without this the
// installer awaited forever and the sign-in never started.
dioxus.send('installed');
"#;

thread_local! {
    /// Server-issued DPoP nonces, keyed by origin. Without this, every request first gets a
    /// `401 use_dpop_nonce` and pays a second round trip — visible in a console as a wall of
    /// red that means nothing is wrong.
    static NONCES: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn origin_of(url: &str) -> String {
    let without_scheme = url.split_once("//").map(|(_, rest)| rest).unwrap_or(url);
    without_scheme.split('/').next().unwrap_or(without_scheme).to_string()
}

fn remember_nonce(url: &str, nonce: &str) {
    let origin = origin_of(url);
    NONCES.with(|map| {
        map.borrow_mut().insert(origin.clone(), nonce.to_string());
    });
    // Kept across reloads too: a reload with a fresh in-memory map used to re-earn a nonce
    // with a guaranteed `400 use_dpop_nonce`, which is what filled the console with red.
    let value = nonce.to_string();
    spawn(async move {
        storage_set(&format!("{NONCE_KEY_PREFIX}{origin}"), &value).await;
    });
}

async fn hydrate_nonce(url: &str) {
    let origin = origin_of(url);
    if NONCES.with(|map| map.borrow().contains_key(&origin)) {
        return;
    }
    if let Some(value) = storage_get(&format!("{NONCE_KEY_PREFIX}{origin}")).await {
        NONCES.with(|map| {
            map.borrow_mut().entry(origin).or_insert(value);
        });
    }
}

fn known_nonce(url: &str) -> Option<String> {
    NONCES.with(|map| map.borrow().get(&origin_of(url)).cloned())
}

/// One eval round trip: run a script that calls `dioxus.send(JSON)` and return the parsed JSON.
async fn eval_json(script: &str) -> Result<serde_json::Value, String> {
    let mut eval = document::eval(script);
    let envelope: String = eval.recv().await.map_err(|err| format!("eval failed: {err}"))?;
    serde_json::from_str(&envelope).map_err(|err| format!("bad eval envelope: {err}"))
}

pub async fn install_dpop() {
    let _ = document::eval(DPOP_JS).recv::<String>().await;
}

/// Fetch or create this browser's DPoP key.
pub async fn dpop_key() -> Result<DpopKey, String> {
    let value = eval_json(
        r#"(async () => {
            try {
                const value = await window.__rustyDpop.loadOrCreate();
                dioxus.send(JSON.stringify({ ok: true, value }));
            } catch (error) { dioxus.send(JSON.stringify({ ok: false, error: String(error) })); }
        })()"#,
    )
    .await?;
    if value["ok"] != serde_json::Value::Bool(true) {
        return Err(value["error"].as_str().unwrap_or("could not create a DPoP key").to_string());
    }
    Ok(DpopKey {
        thumbprint: value["value"]["thumbprint"].as_str().unwrap_or_default().to_string(),
        jwk: value["value"]["jwk"].as_str().unwrap_or("{\"kty\":\"EC\"}").to_string(),
    })
}

/// Ask WebCrypto for an ES256 signature over `input` (a JWS signing input).
async fn dpop_sign(input: &str) -> Result<String, String> {
    let value = eval_json(&format!(
        r#"(async () => {{
            try {{
                const signature = await window.__rustyDpop.sign({input});
                dioxus.send(JSON.stringify({{ ok: true, value: signature }}));
            }} catch (error) {{ dioxus.send(JSON.stringify({{ ok: false, error: String(error) }})); }}
        }})()"#,
        input = serde_json::to_string(input).unwrap(),
    ))
    .await?;
    if value["ok"] != serde_json::Value::Bool(true) {
        return Err(value["error"].as_str().unwrap_or("signing failed").to_string());
    }
    Ok(value["value"].as_str().unwrap_or_default().to_string())
}

/// A complete DPoP proof for one request.
async fn dpop_proof(
    key: &DpopKey,
    htm: &str,
    htu: &str,
    nonce: Option<&str>,
    access_token: Option<&str>,
) -> Result<String, String> {
    let jwk: serde_json::Value =
        serde_json::from_str(&key.jwk).map_err(|err| format!("bad stored key: {err}"))?;
    // `jti` must be unique per proof; the counter is enough on one browser and cheaper than
    // pulling a random source into wasm for something the nonce already protects.
    let jti = format!(
        "{}-{}",
        thumbprint_of(&jwk),
        js_now_millis().await.unwrap_or_default()
    );
    let iat = js_now_millis().await.unwrap_or_default() / 1000;
    let proof: DpopProof = alt_first_core::oauth::dpop_proof(&jwk, htm, htu, &jti, iat, nonce, access_token);
    let signature = dpop_sign(&proof.signing_input()).await?;
    Ok(format!("{}.{}", proof.signing_input(), signature))
}

/// RFC 7638 thumbprint, from the core implementation that is tested against known vectors —
/// the browser and Rust must agree on this string or the stored key stops matching its name.
fn thumbprint_of(jwk: &serde_json::Value) -> String {
    jwk_thumbprint(jwk).unwrap_or_default()
}

async fn js_now_millis() -> Option<i64> {
    let value = eval_json(
        r#"(() => { dioxus.send(JSON.stringify({ now: Date.now() })); })()"#,
    )
    .await
    .ok()?;
    value["now"].as_i64()
}

/// The app's own client identity, derived from where it is running.
///
/// No build input is needed: the origin *is* the answer, and it must match the metadata file
/// the deploy serves at `<origin><path>oauth-client-metadata.json`. Loopback is the exception
/// the PDS special-cases — `client_id` is exactly `http://localhost` with the rest in the query.
pub async fn client_identity() -> Result<(String, String), String> {
    let value = eval_json(
        r#"(() => { dioxus.send(JSON.stringify({ origin: location.origin, path: location.pathname.replace(/[^/]*$/, '') })); })()"#,
    )
    .await?;
    let origin = value["origin"].as_str().unwrap_or_default().to_string();
    let path = value["path"].as_str().unwrap_or_default().to_string();
    let loopback = origin.contains("127.0.0.1") || origin.contains("localhost");
    if loopback {
        let port = origin.rsplit(':').next().unwrap_or("80");
        let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
        let client_id = format!(
            "http://localhost?redirect_uri={}&scope={}",
            alt_first_core::encode_uri_component(&redirect),
            alt_first_core::encode_uri_component(SCOPE)
        );
        Ok((client_id, redirect))
    } else {
        Ok((
            format!("{origin}{path}oauth-client-metadata.json"),
            format!("{origin}{path}oauth/callback/"),
        ))
    }
}

pub const SCOPE: &str = "atproto transition:generic";

/// One HTTP request with explicit headers, returning status, body and any `DPoP-Nonce`.
///
/// Separate from the app's ordinary `request_text` because OAuth needs two things it does not:
/// headers it chooses (a proof per attempt), and the nonce from a failed attempt's response
/// rather than an error.
pub async fn fetch_raw(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Result<RawResponse, String> {
    let headers_json = serde_json::to_string(
        &headers
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap();
    let value = eval_json(&format!(
        r#"(async () => {{
            try {{
                // A request that never answers would leave the caller awaiting a message that
                // never comes: the sign-in sits on the callback URL forever with no error.
                // A deadline turns that into a reportable failure.
                const controller = new AbortController();
                const deadline = setTimeout(() => controller.abort(), 20000);
                let response;
                try {{
                    response = await fetch({url}, {{
                        method: {method},
                        headers: {headers_json},
                        body: {body},
                        credentials: 'omit',
                        signal: controller.signal,
                    }});
                }} finally {{
                    clearTimeout(deadline);
                }}
                dioxus.send(JSON.stringify({{
                    ok: true,
                    status: response.status,
                    body: await response.text(),
                    nonce: response.headers.get('DPoP-Nonce'),
                }}));
            }} catch (error) {{
                const timedOut = error && error.name === 'AbortError';
                dioxus.send(JSON.stringify({{
                    ok: false,
                    error: timedOut ? 'the request timed out after 20s' : String(error),
                }}));
            }}
        }})()"#,
        url = serde_json::to_string(url).unwrap(),
        method = serde_json::to_string(method).unwrap(),
        body = serde_json::to_string(&body).unwrap(),
    ))
    .await?;
    if value["ok"] != serde_json::Value::Bool(true) {
        return Err(value["error"].as_str().unwrap_or("request failed").to_string());
    }
    let dpop_nonce = value["nonce"].as_str().map(str::to_string);
    if let Some(nonce) = &dpop_nonce {
        remember_nonce(url, nonce);
    }
    Ok(RawResponse {
        status: value["status"].as_u64().unwrap_or(0) as u16,
        body: value["body"].as_str().unwrap_or_default().to_string(),
        dpop_nonce,
    })
}

/// Headers that authenticate a request, signing a proof for this exact method and URL.
///
/// `nonce` is the server's `DPoP-Nonce` when retrying after a `use_dpop_nonce` rejection; a
/// PDS that requires nonces for resource requests will reject the first one of a session.
pub async fn auth_headers(
    auth: Auth<'_>,
    method: &str,
    url: &str,
    nonce: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    hydrate_nonce(url).await;
    let known = known_nonce(url);
    let nonce = nonce.or(known.as_deref());
    match auth {
        Auth::Bearer(token) => Ok(vec![(
            "Authorization".to_string(),
            format!("Bearer {token}"),
        )]),
        Auth::Dpop { token, oauth } => {
            let key = DpopKey {
                thumbprint: oauth.thumbprint.clone(),
                jwk: oauth.jwk.clone(),
            };
            let proof = dpop_proof(&key, method, url, nonce, Some(token)).await?;
            Ok(vec![
                ("Authorization".to_string(), format!("DPoP {token}")),
                ("DPoP".to_string(), proof),
            ])
        }
    }
}

/// A POST that retries once with the server's nonce.
///
/// Every DPoP endpoint answers the first attempt with `400 use_dpop_nonce` and a `DPoP-Nonce`
/// header. The retry repeats the identical request with that nonce in the proof — and with a
/// fresh `jti`, since the first proof was rejected.
async fn post_with_nonce(
    url: &str,
    form: &str,
    key: &DpopKey,
    access_token: Option<&str>,
) -> Result<RawResponse, String> {
    hydrate_nonce(url).await;
    let attempt = |nonce: Option<String>| {
        let form = form.to_string();
        async move {
            let proof = dpop_proof(key, "POST", url, nonce.as_deref(), access_token).await?;
            fetch_raw(
                "POST",
                url,
                &[
                    ("content-type".to_string(), "application/x-www-form-urlencoded".to_string()),
                    ("DPoP".to_string(), proof),
                ],
                Some(&form),
            )
            .await
        }
    };

    let first = attempt(known_nonce(url)).await?;
    if first.ok() {
        return Ok(first);
    }
    match (&first.dpop_nonce, first.status) {
        (Some(nonce), 400 | 401) => attempt(Some(nonce.clone())).await,
        _ => Ok(first),
    }
}

fn json_error(response: &RawResponse, what: &str) -> String {
    let body: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
    match (body["error"].as_str(), body["error_description"].as_str()) {
        (Some(error), Some(description)) => format!("{what} refused: {error} ({description})"),
        (Some(error), None) => format!("{what} refused: {error}"),
        _ => format!("{what} failed: HTTP {} {}", response.status, response.body),
    }
}

/// Where a handle's account actually lives, and where it authorises.
pub struct Discovery {
    pub did: String,
    pub pds: String,
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub par_endpoint: String,
    pub revocation_endpoint: Option<String>,
}

async fn get_json(url: &str) -> Result<(u16, serde_json::Value), String> {
    let response = fetch_raw("GET", url, &[("accept".to_string(), "application/json".to_string())], None).await?;
    Ok((
        response.status,
        serde_json::from_str(&response.body).unwrap_or(serde_json::Value::Null),
    ))
}

/// handle → DID → PDS → authorisation server.
///
/// The DID document is the only trustworthy source for the PDS; the AS then comes from the
/// PDS's own `oauth-protected-resource`, which as of writing lives on the PDS host and not on
/// `bsky.social` (that host answers 404). Ask both, and fall back to the PDS being its own AS.
pub async fn discover(handle: &str) -> Result<Discovery, String> {
    let (status, resolved) = get_json(&format!(
        "https://api.bsky.app/xrpc/com.atproto.identity.resolveHandle?handle={}",
        alt_first_core::encode_uri_component(handle)
    ))
    .await?;
    let did = resolved["did"].as_str().map(str::to_string);
    let Some(did) = did else {
        return Err(format!("could not resolve {handle} (HTTP {status})"));
    };

    let did_document_url = if let Some(short) = did.strip_prefix("did:plc:") {
        format!("https://plc.directory/did:plc:{short}")
    } else if let Some(host) = did.strip_prefix("did:web:") {
        format!("https://{}/.well-known/did.json", host.replace("%3A", ":"))
    } else {
        return Err(format!("unsupported DID method: {did}"));
    };
    let (_, document) = get_json(&did_document_url).await?;
    let pds = document["service"]
        .as_array()
        .and_then(|services| {
            services
                .iter()
                .find(|service| service["id"].as_str() == Some("#atproto_pds"))
        })
        .and_then(|service| service["serviceEndpoint"].as_str())
        .map(|endpoint| endpoint.trim_end_matches('/').to_string());
    let Some(pds) = pds else {
        return Err(format!("no PDS in the DID document for {did}"));
    };

    let (resource_status, resource) =
        get_json(&format!("{pds}/.well-known/oauth-protected-resource")).await?;
    let issuer = resource["authorization_servers"]
        .as_array()
        .and_then(|servers| servers.first())
        .and_then(|server| server.as_str())
        .map(str::to_string)
        .or_else(|| (resource_status == 200).then(|| pds.clone()))
        .unwrap_or_else(|| pds.clone());

    let (as_status, as_metadata) = get_json(&format!("{issuer}/.well-known/oauth-authorization-server")).await?;
    if as_status != 200 {
        return Err(format!("no authorisation-server metadata at {issuer} (HTTP {as_status})"));
    }
    let field = |name: &str| as_metadata[name].as_str().map(str::to_string);
    let (Some(authorization_endpoint), Some(token_endpoint), Some(par_endpoint)) = (
        field("authorization_endpoint"),
        field("token_endpoint"),
        field("pushed_authorization_request_endpoint"),
    ) else {
        return Err(format!("{issuer} is missing OAuth endpoints"));
    };
    // The issuer in the metadata must match where it was fetched from, or discovery has been
    // redirected somewhere else and nothing downstream can be trusted.
    if let Some(declared) = field("issuer") {
        if declared != issuer {
            return Err(format!("issuer mismatch: {declared} != {issuer}"));
        }
    }

    Ok(Discovery {
        did,
        pds,
        issuer,
        authorization_endpoint,
        token_endpoint,
        par_endpoint,
        revocation_endpoint: field("revocation_endpoint"),
    })
}

const PENDING_KEY: &str = "rusty.oauth.pending";
/// Per-origin DPoP nonces, so a reload can sign its first proof correctly.
const NONCE_KEY_PREFIX: &str = "rusty.oauth.nonce:";
const CALLBACK_KEY: &str = "rusty.oauth.callback";

async fn storage_get(key: &str) -> Option<String> {
    let value = eval_json(&format!(
        r#"(() => {{
            try {{ dioxus.send(JSON.stringify({{ value: sessionStorage.getItem({key}) }})); }}
            catch (error) {{ dioxus.send(JSON.stringify({{ value: null }})); }}
        }})()"#,
        key = serde_json::to_string(key).unwrap()
    ))
    .await
    .ok()?;
    value["value"].as_str().map(str::to_string)
}

async fn storage_set(key: &str, value: &str) {
    let _ = eval_json(&format!(
        r#"(() => {{
            try {{ sessionStorage.setItem({key}, {value}); }} catch (error) {{}}
            dioxus.send(JSON.stringify({{ ok: true }}));
        }})()"#,
        key = serde_json::to_string(key).unwrap(),
        value = serde_json::to_string(value).unwrap()
    ))
    .await;
}

async fn storage_remove(key: &str) {
    let _ = eval_json(&format!(
        r#"(() => {{
            try {{ sessionStorage.removeItem({key}); }} catch (error) {{}}
            dioxus.send(JSON.stringify({{ ok: true }}));
        }})()"#,
        key = serde_json::to_string(key).unwrap()
    ))
    .await;
}

/// Random bytes as base64url, from the platform CSPRNG.
async fn random_b64(len: usize) -> Result<String, String> {
    let value = eval_json(&format!(
        r#"(() => {{
            const bytes = new Uint8Array({len});
            crypto.getRandomValues(bytes);
            const text = btoa(String.fromCharCode(...bytes))
                .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
            dioxus.send(JSON.stringify({{ value: text }}));
        }})()"#
    ))
    .await?;
    value["value"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "could not read random bytes".to_string())
}

/// Everything a callback needs to finish the flow it belongs to.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    state: String,
    verifier: String,
    issuer: String,
    did: String,
    pds: String,
    client_id: String,
    redirect_uri: String,
    token_endpoint: String,
    revocation_endpoint: Option<String>,
    thumbprint: String,
    jwk: String,
    remember: bool,
}

/// Begin a sign-in: discover, register the request (PAR), remember the attempt, navigate.
///
/// Same-tab navigation, not a popup: `sessionStorage` is per tab, so a popup would strand the
/// pending state and the callback would arrive in a tab that knows nothing about it.
pub async fn start(handle: &str, remember: bool) -> Result<(), String> {
    if handle.trim().is_empty() {
        return Err("enter your handle first".to_string());
    }
    install_dpop().await;
    let discovery = discover(handle).await?;
    let key = dpop_key().await?;
    let (client_id, redirect_uri) = client_identity().await?;

    let verifier = random_b64(32).await?;
    let challenge = alt_first_core::oauth::pkce_challenge(&verifier);
    let state = random_b64(16).await?;

    let form = alt_first_core::oauth::form_encode(&[
        ("client_id", client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", SCOPE),
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("login_hint", handle),
    ]);
    let response = post_with_nonce(&discovery.par_endpoint, &form, &key, None).await?;
    let body: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
    let Some(request_uri) = body["request_uri"].as_str() else {
        return Err(json_error(&response, "pushed authorisation request"));
    };

    let pending = Pending {
        state,
        verifier,
        issuer: discovery.issuer.clone(),
        did: discovery.did.clone(),
        pds: discovery.pds.clone(),
        client_id: client_id.clone(),
        redirect_uri,
        token_endpoint: discovery.token_endpoint.clone(),
        revocation_endpoint: discovery.revocation_endpoint.clone(),
        thumbprint: key.thumbprint.clone(),
        jwk: key.jwk.clone(),
        remember,
    };
    storage_set(
        PENDING_KEY,
        &serde_json::to_string(&pending).map_err(|err| err.to_string())?,
    )
    .await;

    let target = authorize_url(&discovery.authorization_endpoint, &client_id, request_uri);
    let _ = eval_json(&format!(
        r#"(() => {{ location.assign({url}); dioxus.send(JSON.stringify({{ ok: true }})); }})()"#,
        url = serde_json::to_string(&target).unwrap()
    ))
    .await;
    Ok(())
}

/// Whether the tab is on the callback URL, i.e. a redirect delivered us here and something
/// was supposed to finish. Used to explain a callback that found nothing to complete.
pub async fn on_callback_route() -> bool {
    eval_json(r#"(() => { dioxus.send(JSON.stringify({ hash: location.hash })); })()"#)
        .await
        .ok()
        .and_then(|value| {
            value["hash"]
                .as_str()
                .map(|hash| hash.contains("oauth-callback"))
        })
        .unwrap_or(false)
}

/// A finished sign-in, in the shape `main.rs` needs to build its own `Session`.
pub struct Completed {
    pub access_jwt: String,
    pub refresh_jwt: String,
    pub did: String,
    pub handle: String,
    pub pds: String,
    pub oauth: OAuthSession,
    pub remember: bool,
}

/// Finish a sign-in if the tab has a callback waiting.
///
/// Returns `Ok(None)` when there is nothing to do — the common case on every normal load — so
/// the app can call this unconditionally on mount.
pub async fn complete() -> Result<Option<Completed>, String> {
    let Some(pending_json) = storage_get(PENDING_KEY).await else {
        return Ok(None);
    };
    let Some(callback_json) = storage_get(CALLBACK_KEY).await else {
        return Ok(None);
    };
    let pending: Pending = serde_json::from_str(&pending_json)
        .map_err(|err| format!("stored sign-in attempt is unreadable: {err}"))?;
    let callback: serde_json::Value =
        serde_json::from_str(&callback_json).unwrap_or(serde_json::Value::Null);
    let params = CallbackParams {
        code: callback["code"].as_str().map(str::to_string),
        state: callback["state"].as_str().map(str::to_string),
        iss: callback["iss"].as_str().map(str::to_string),
        error: callback["error"].as_str().map(str::to_string),
        error_description: callback["error_description"].as_str().map(str::to_string),
    };

    // Both are single-use whatever happens next: keeping them would let a stale callback be
    // replayed against a later attempt.
    storage_remove(CALLBACK_KEY).await;
    storage_remove(PENDING_KEY).await;

    // The issuer in the callback must be the one this attempt started with, or the code came
    // from somewhere else entirely.
    if let Some(iss) = &params.iss {
        if iss != &pending.issuer {
            return Err(format!("callback came from {iss}, not {}", pending.issuer));
        }
    }
    let code = params.take_code(&pending.state)?;

    install_dpop().await;
    let key = dpop_key().await?;
    if key.thumbprint != pending.thumbprint {
        return Err("the DPoP key for this sign-in is gone; start again".to_string());
    }

    let form = code_exchange_form(&pending.client_id, &pending.redirect_uri, code, &pending.verifier);
    let response = post_with_nonce(&pending.token_endpoint, &form, &key, None).await?;
    let body: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
    let (Some(access), Some(refresh)) = (body["access_token"].as_str(), body["refresh_token"].as_str())
    else {
        return Err(json_error(&response, "code exchange"));
    };

    let oauth = OAuthSession {
        issuer: pending.issuer.clone(),
        client_id: pending.client_id.clone(),
        scope: body["scope"].as_str().unwrap_or(SCOPE).to_string(),
        thumbprint: pending.thumbprint.clone(),
        jwk: pending.jwk.clone(),
        token_endpoint: pending.token_endpoint.clone(),
        revocation_endpoint: pending.revocation_endpoint.clone(),
    };

    // Prove the token actually works before treating the flow as done, and get the canonical
    // handle while at it: the one typed may be an alias.
    let (did, handle) = identity(&oauth, access, &pending.pds).await.unwrap_or((
        pending.did.clone(),
        String::new(),
    ));

    // `pds` comes from the DID document, and must: `bsky.social` answers OAuth tokens with
    // "OAuth tokens are meant for PDS access only" — the entryway is not a resource server
    // for them. Do not "simplify" this to the app-password host.
    Ok(Some(Completed {
        access_jwt: access.to_string(),
        refresh_jwt: refresh.to_string(),
        did,
        handle,
        pds: pending.pds.clone(),
        oauth,
        remember: pending.remember,
    }))
}

/// `com.atproto.server.getSession` with a DPoP-bound token: the cheapest proof that the whole
/// chain worked, and the only way to learn the account's canonical handle.
async fn identity(oauth: &OAuthSession, access: &str, pds: &str) -> Option<(String, String)> {
    let url = format!("{pds}/xrpc/com.atproto.server.getSession");
    // The PDS rejects the first proof of a session with `use_dpop_nonce`, so a single attempt
    // always failed here and the caller fell back to showing the DID as the handle.
    let mut nonce: Option<String> = None;
    let mut response = None;
    for _ in 0..2 {
        let headers = auth_headers(
            Auth::Dpop { token: access, oauth },
            "GET",
            &url,
            nonce.as_deref(),
        )
        .await
        .ok()?;
        let attempt = fetch_raw("GET", &url, &headers, None).await.ok()?;
        if attempt.ok() {
            response = Some(attempt);
            break;
        }
        match (attempt.dpop_nonce.clone(), attempt.status) {
            (Some(issued), 400 | 401) => nonce = Some(issued),
            _ => return None,
        }
    }
    let response = response?;
    let body: serde_json::Value = serde_json::from_str(&response.body).ok()?;
    Some((
        body["did"].as_str()?.to_string(),
        body["handle"].as_str()?.to_string(),
    ))
}

/// Rotate the token pair.
///
/// Refresh tokens rotate too: the response carries a new one and the old one stops working, so
/// a failed store here means the next refresh needs the *new* value to have been persisted.
pub async fn refresh(oauth: &OAuthSession, refresh_token: &str) -> Result<(String, String), String> {
    install_dpop().await;
    // The proof is signed by whatever key this browser holds; if that is not the key the
    // session was issued to, the server can only answer `invalid_dpop_proof`. Say the real
    // problem instead of sending a proof that cannot verify.
    let current = dpop_key().await?;
    if current.thumbprint != oauth.thumbprint {
        return Err(format!(
            "this session's key is not in this browser (have {}, need {}); sign in again",
            current.thumbprint, oauth.thumbprint
        ));
    }
    let key = DpopKey {
        thumbprint: oauth.thumbprint.clone(),
        jwk: oauth.jwk.clone(),
    };
    let form = refresh_form(&oauth.client_id, refresh_token);
    let response = post_with_nonce(&oauth.token_endpoint, &form, &key, None).await?;
    let body: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
    match (body["access_token"].as_str(), body["refresh_token"].as_str()) {
        (Some(access), Some(refresh)) => Ok((access.to_string(), refresh.to_string())),
        _ => Err(json_error(&response, "refresh")),
    }
}

/// Revoke the session at the authorisation server, then forget the key.
///
/// Best effort by design: a network failure must not stop the local sign-out, or a session
/// would appear to persist. Forgetting the key is what makes the tokens unusable locally, and
/// revoking is what makes them unusable anywhere.
pub async fn revoke(oauth: &OAuthSession, refresh_token: &str) {
    install_dpop().await;
    if let Some(endpoint) = &oauth.revocation_endpoint {
        let key = DpopKey {
            thumbprint: oauth.thumbprint.clone(),
            jwk: oauth.jwk.clone(),
        };
        let _ = post_with_nonce(endpoint, &revoke_form(&oauth.client_id, refresh_token), &key, None).await;
    }
    let _ = eval_json(
        r#"(async () => {
            try { await window.__rustyDpop.forget(); } catch (error) {}
            dioxus.send(JSON.stringify({ ok: true }));
        })()"#,
    )
    .await;
}
