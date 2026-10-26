//! AT Protocol OAuth, as far as it can be pure.
//!
//! The pieces here are the ones that are deterministic and worth testing on the host: the
//! base64url dialect JWTs use, PKCE's S256 challenge, the JWK thumbprint that names a DPoP
//! key, and the signing input a DPoP proof is built from. Key generation and the actual
//! ECDSA signature live in the browser (WebCrypto), because the private key must not be
//! something this code can export.
//!
//! References: RFC 7636 (PKCE), RFC 7638 (JWK thumbprint), RFC 9449 (DPoP), and the
//! AT Protocol OAuth spec's client-metadata requirements.

use sha2::{Digest, Sha256};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url without padding — the only encoding JWT/JWK/PKCE use.
pub fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(triple >> 18) as usize & 0x3f] as char);
        out.push(B64[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(B64[(triple >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(B64[triple as usize & 0x3f] as char);
        }
    }
    out
}

/// PKCE `code_challenge` for the only method AT Protocol allows: `S256`.
pub fn pkce_challenge(verifier: &str) -> String {
    base64url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// RFC 7638 thumbprint over a JWK.
///
/// The canonical form is the required members only, in lexicographic order, with no
/// whitespace — get the order or the spacing wrong and the thumbprint is a different
/// string, which the server then rejects as a key mismatch.
pub fn jwk_thumbprint(jwk: &serde_json::Value) -> Option<String> {
    let kty = jwk["kty"].as_str()?;
    let canonical = match kty {
        // EC keys: crv, kty, x, y
        "EC" => format!(
            "{{\"crv\":\"{}\",\"kty\":\"EC\",\"x\":\"{}\",\"y\":\"{}\"}}",
            jwk["crv"].as_str()?,
            jwk["x"].as_str()?,
            jwk["y"].as_str()?,
        ),
        // OKP keys (Ed25519): crv, kty, x
        "OKP" => format!(
            "{{\"crv\":\"{}\",\"kty\":\"OKP\",\"x\":\"{}\"}}",
            jwk["crv"].as_str()?,
            jwk["x"].as_str()?,
        ),
        _ => return None,
    };
    Some(base64url_encode(&Sha256::digest(canonical.as_bytes())))
}

/// A DPoP proof, minus its signature.
///
/// `htm`/`htu` bind the proof to one request, `ath` binds it to an access token, `nonce`
/// carries the server's challenge, and `jti` must be unique per proof (replay detection).
pub struct DpopProof {
    pub header: String,
    pub claims: String,
}

impl DpopProof {
    /// The string the ECDSA signature is computed over.
    pub fn signing_input(&self) -> String {
        format!("{}.{}", self.header, self.claims)
    }
}

/// Build the unsigned proof for one request.
///
/// The caller supplies the JWK (public half), the HTTP method and URL it is for, a unique
/// `jti`, the current time, and — when the server has issued one — a nonce. `ath` must be
/// the base64url SHA-256 of the access token, which is how a proof is bound to a token.
pub fn dpop_proof(
    jwk: &serde_json::Value,
    htm: &str,
    htu: &str,
    jti: &str,
    iat: i64,
    nonce: Option<&str>,
    access_token: Option<&str>,
) -> DpopProof {
    let header = serde_json::json!({
        "typ": "dpop+jwt",
        "alg": "ES256",
        "jwk": {
            "kty": jwk["kty"].clone(),
            "crv": jwk["crv"].clone(),
            "x": jwk["x"].clone(),
            "y": jwk["y"].clone(),
        }
    });
    let mut claims = serde_json::json!({
        "jti": jti,
        "htm": htm,
        "htu": htu,
        "iat": iat,
    });
    if let Some(nonce) = nonce {
        claims["nonce"] = serde_json::Value::String(nonce.to_string());
    }
    if let Some(token) = access_token {
        claims["ath"] = serde_json::Value::String(base64url_encode(&Sha256::digest(token.as_bytes())));
    }
    DpopProof {
        header: base64url_encode(header.to_string().as_bytes()),
        claims: base64url_encode(claims.to_string().as_bytes()),
    }
}

/// Which sign-in paths a build offers.
///
/// A deployment-level choice, not a user preference: app passwords need no configuration and
/// OAuth needs a client-metadata document whose URL is this deployment's own, so a fork and an
/// OAuth-enabled deploy are legitimately different builds. Selected by `OAUTH_MODE` at build
/// time, alongside `APP_NAME` and `REPO_URL`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignInMode {
    /// App password only. The default: zero configuration, so a fork that sets nothing works.
    #[default]
    AppPasswordOnly,
    /// Both paths offered side by side, app passwords first.
    Both,
    /// OAuth is the primary path; app passwords stay as a fallback link.
    OAuthPrimary,
}

// The variants describe what the sign-in UI *does*, not how finished it is: maturity is a
// property of the code, not a setting, and a value called "beta" would have to be renamed on
// the day it stopped being one — renaming a variable a deployment has already set.

impl SignInMode {
    /// Parse `OAUTH_MODE`. Accepts a few spellings because this is typed into a web form by
    /// hand, and returns `None` for anything else so the caller can fall back deliberately
    /// rather than silently treating a typo as a mode.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "off" | "app-password" | "password" => Some(Self::AppPasswordOnly),
            "both" => Some(Self::Both),
            "on" | "oauth" => Some(Self::OAuthPrimary),
            _ => None,
        }
    }

    /// Whether the UI should offer an OAuth entry point at all.
    pub fn offers_oauth(self) -> bool {
        !matches!(self, Self::AppPasswordOnly)
    }
}

/// `application/x-www-form-urlencoded` body from ordered pairs.
///
/// OAuth endpoints take form bodies, not JSON, and the encoding differs from a query only in
/// that a space may be `+` (this leaves it percent-encoded, which is also valid and avoids
/// having to treat `+` specially on the way back in).
pub fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                crate::encode_uri_component(name),
                crate::encode_uri_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The authorisation URL to open in a browser, after PAR has registered the request.
///
/// With PAR in use the URL carries only the `request_uri` — the scope, redirect and challenge
/// live server-side, which is the point: a tampered URL cannot change what is being asked for.
pub fn authorize_url(authorization_endpoint: &str, client_id: &str, request_uri: &str) -> String {
    let separator = if authorization_endpoint.contains('?') { '&' } else { '?' };
    format!(
        "{authorization_endpoint}{separator}client_id={}&request_uri={}",
        crate::encode_uri_component(client_id),
        crate::encode_uri_component(request_uri)
    )
}

/// Body for the `authorization_code` grant.
///
/// `code_verifier` proves this client started the flow (PKCE, so the code is useless to
/// anyone who intercepts it), and `client_id` names the metadata document again because the
/// server does not remember which client a `request_uri` belonged to.
pub fn code_exchange_form(
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> String {
    form_encode(&[
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("code", code),
        ("code_verifier", code_verifier),
    ])
}

/// Body for the `refresh_token` grant. Refresh tokens rotate: the response carries a new one,
/// and the old one must be discarded.
pub fn refresh_form(client_id: &str, refresh_token: &str) -> String {
    form_encode(&[
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ])
}

/// Body for revocation at sign-out. Revoking the refresh token is enough: access tokens are
/// short-lived and revoking the refresh stops the session being extended.
pub fn revoke_form(client_id: &str, refresh_token: &str) -> String {
    form_encode(&[
        ("client_id", client_id),
        ("token", refresh_token),
        ("token_type_hint", "refresh_token"),
    ])
}

/// What the callback hands back: the query string the authorisation server redirected with.
///
/// Parsed here rather than in the UI because getting it wrong is a security bug: a mismatched
/// `state` means the code belongs to a different flow and must be discarded, and an `error`
/// must be surfaced rather than swallowed as "no code".
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub iss: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

impl CallbackParams {
    pub fn parse(query: &str) -> Self {
        let mut params = Self::default();
        for pair in query.trim_start_matches('?').split('&') {
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let value = crate::decode_uri_component(&value.replace('+', " "));
            match name {
                "code" => params.code = Some(value),
                "state" => params.state = Some(value),
                "iss" => params.iss = Some(value),
                "error" => params.error = Some(value),
                "error_description" => params.error_description = Some(value),
                _ => {}
            }
        }
        params
    }

    /// The code, but only for the flow this client started.
    ///
    /// `expected_state` comes from storage written before the redirect; anything else — a
    /// replayed callback, another tab's flow, a forged one — is refused. An `error` from the
    /// server is reported as itself so the user sees "access denied" rather than "no code".
    pub fn take_code(&self, expected_state: &str) -> Result<&str, String> {
        if let Some(error) = &self.error {
            let description = self.error_description.as_deref().unwrap_or("no description");
            return Err(format!("authorisation refused: {error} ({description})"));
        }
        let code = self.code.as_deref().ok_or("callback carried no code")?;
        match self.state.as_deref() {
            Some(state) if state == expected_state => Ok(code),
            Some(_) => Err("callback state did not match this sign-in attempt".to_string()),
            None => Err("callback carried no state".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64url_decode(value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buffer: u32 = 0;
        let mut bits = 0;
        for byte in value.bytes() {
            let sextet = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'-' => 62,
                b'_' => 63,
                _ => continue,
            } as u32;
            buffer = (buffer << 6) | sextet;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        out
    }

    #[test]
    fn base64url_matches_the_rfc_4648_vectors() {
        // Same alphabet as base64, no padding: the encoding JWT, JWK and PKCE all use.
        assert_eq!(base64url_encode(b""), "");
        assert_eq!(base64url_encode(b"f"), "Zg");
        assert_eq!(base64url_encode(b"fo"), "Zm8");
        assert_eq!(base64url_encode(b"foo"), "Zm9v");
        assert_eq!(base64url_encode(b"foob"), "Zm9vYg");
        assert_eq!(base64url_encode(b"fooba"), "Zm9vYmE");
        assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
        // The two substitutions that make it url-safe rather than base64.
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_appendix_b() {
        // The spec's own worked example. If this drifts, every authorisation request is
        // rejected as a challenge mismatch, with no useful error at the server.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn jwk_thumbprint_uses_only_the_required_members_in_order() {
        // Cross-checked with an independent SHA-256 of the canonical JSON. Extra members
        // (`kid`, `use`, `alg`) and key order in the input must not change the result:
        // the thumbprint is what names the DPoP key for the whole session.
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            "kid": "ignored",
            "use": "sig",
        });
        assert_eq!(
            jwk_thumbprint(&jwk).as_deref(),
            Some("oKIywvGUpTVTyxMQ3bwIIeQUudfr_CkLMjCE19ECD-U")
        );

        let reordered = serde_json::json!({
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "crv": "P-256",
            "kty": "EC",
        });
        assert_eq!(jwk_thumbprint(&jwk), jwk_thumbprint(&reordered));

        // A different key is a different name.
        let mut other = jwk.clone();
        other["x"] = serde_json::Value::String("AAAA".to_string());
        assert_ne!(jwk_thumbprint(&other), jwk_thumbprint(&jwk));

        assert_eq!(jwk_thumbprint(&serde_json::json!({"kty": "RSA"})), None);
    }

    #[test]
    fn dpop_proof_carries_the_binding_claims() {
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            "d": "private-material-must-not-appear",
        });
        let proof = dpop_proof(
            &jwk,
            "POST",
            "https://bsky.social/oauth/par",
            "unique-jti",
            1_700_000_000,
            None,
            None,
        );

        let header: serde_json::Value =
            serde_json::from_slice(&base64url_decode(&proof.header)).expect("header json");
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["alg"], "ES256");
        // The private exponent must never be sent, even if the caller passes it in.
        assert!(header["jwk"].get("d").is_none());

        let claims: serde_json::Value =
            serde_json::from_slice(&base64url_decode(&proof.claims)).expect("claims json");
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://bsky.social/oauth/par");
        assert_eq!(claims["jti"], "unique-jti");
        assert_eq!(claims["iat"], 1_700_000_000);
        assert!(claims.get("nonce").is_none());
        assert!(claims.get("ath").is_none());

        assert_eq!(proof.signing_input(), format!("{}.{}", proof.header, proof.claims));
    }

    #[test]
    fn form_bodies_match_what_the_token_endpoint_expects() {
        // Field names are load-bearing: `code_verifier` misspelled is an `invalid_grant` with
        // no hint, and a missing `client_id` is `invalid_client`.
        assert_eq!(
            code_exchange_form(
                "https://app.example/oauth-client-metadata.json",
                "https://app.example/oauth/callback/",
                "the-code",
                "the-verifier",
            ),
            concat!(
                "grant_type=authorization_code",
                "&client_id=https%3A%2F%2Fapp.example%2Foauth-client-metadata.json",
                "&redirect_uri=https%3A%2F%2Fapp.example%2Foauth%2Fcallback%2F",
                "&code=the-code",
                "&code_verifier=the-verifier",
            )
        );
        assert_eq!(
            refresh_form("cid", "refresh-1"),
            "grant_type=refresh_token&client_id=cid&refresh_token=refresh-1"
        );
        assert_eq!(
            revoke_form("cid", "refresh-1"),
            "client_id=cid&token=refresh-1&token_type_hint=refresh_token"
        );
        // A space or `+` in a value must not turn into a field separator.
        assert_eq!(form_encode(&[("scope", "atproto transition:generic")]),
                   "scope=atproto%20transition%3Ageneric");
    }

    #[test]
    fn authorize_url_carries_only_the_request_uri() {
        // The scope, redirect and challenge are registered server-side by PAR; a URL that also
        // repeated them could be edited to change the request.
        let url = authorize_url(
            "https://bsky.social/oauth/authorize",
            "https://app.example/oauth-client-metadata.json",
            "urn:ietf:params:oauth:request_uri:req-1",
        );
        assert_eq!(
            url,
            concat!(
                "https://bsky.social/oauth/authorize",
                "?client_id=https%3A%2F%2Fapp.example%2Foauth-client-metadata.json",
                "&request_uri=urn%3Aietf%3Aparams%3Aoauth%3Arequest_uri%3Areq-1",
            )
        );
        assert!(!url.contains("scope="));
    }

    #[test]
    fn callback_accepts_only_the_flow_it_started() {
        let params = CallbackParams::parse("?code=abc&state=xyz&iss=https%3A%2F%2Fbsky.social");
        assert_eq!(params.code.as_deref(), Some("abc"));
        assert_eq!(params.state.as_deref(), Some("xyz"));
        assert_eq!(params.iss.as_deref(), Some("https://bsky.social"));
        assert_eq!(params.take_code("xyz"), Ok("abc"));

        // A replayed or forged callback for another attempt is refused, not used.
        assert!(params.take_code("other").is_err());
        // No state at all is not "fine, then".
        assert!(CallbackParams::parse("?code=abc").take_code("xyz").is_err());
        assert!(CallbackParams::parse("").take_code("xyz").is_err());

        // A refusal is reported as itself: "access denied" is what the user did, and telling
        // them "no code" would hide it.
        let refused = CallbackParams::parse("?error=access_denied&error_description=Denied+by+the+user");
        let message = refused.take_code("xyz").unwrap_err();
        assert!(message.contains("access_denied"), "{message}");
        assert!(message.contains("Denied by the user"), "{message}");
        assert!(refused.take_code("xyz").is_err());
    }

    #[test]
    fn sign_in_mode_parses_the_documented_spellings() {
        assert_eq!(SignInMode::parse("off"), Some(SignInMode::AppPasswordOnly));
        assert_eq!(SignInMode::parse("app-password"), Some(SignInMode::AppPasswordOnly));
        assert_eq!(SignInMode::parse("both"), Some(SignInMode::Both));
                assert_eq!(SignInMode::parse("on"), Some(SignInMode::OAuthPrimary));
        assert_eq!(SignInMode::parse("oauth"), Some(SignInMode::OAuthPrimary));

        // A repository variable is typed by hand into a web form, so case and stray spaces
        // must not turn a working choice into the silent default.
        assert_eq!(SignInMode::parse("  OAuth "), Some(SignInMode::OAuthPrimary));
        assert_eq!(SignInMode::parse(""), Some(SignInMode::AppPasswordOnly));
    }

    #[test]
    fn an_unrecognised_mode_is_rejected_rather_than_guessed() {
        // `OAUTH_MODE: oauth2` must not quietly mean something; the caller picks the default
        // (app passwords) so an unset or typo'd variable keeps the configuration-free path.
        assert_eq!(SignInMode::parse("oauth2"), None);
        // "beta" was a lifecycle label, renamed to `both` before anything shipped; it must not
        // linger as an alias that quietly keeps working.
        assert_eq!(SignInMode::parse("beta"), None);
        assert_eq!(SignInMode::parse("yes please"), None);
        assert_eq!(SignInMode::default(), SignInMode::AppPasswordOnly);
        assert!(!SignInMode::default().offers_oauth());
        assert!(SignInMode::Both.offers_oauth());
        assert!(SignInMode::OAuthPrimary.offers_oauth());
    }

    #[test]
    fn dpop_nonce_and_ath_are_added_when_supplied() {
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
        });
        let proof = dpop_proof(
            &jwk,
            "POST",
            "https://bsky.social/oauth/token",
            "jti-2",
            1_700_000_100,
            Some("server-nonce"),
            Some("example-access-token"),
        );
        let claims: serde_json::Value =
            serde_json::from_slice(&base64url_decode(&proof.claims)).expect("claims json");
        assert_eq!(claims["nonce"], "server-nonce");
        // `ath` is base64url(SHA-256(token)); cross-checked independently.
        assert_eq!(claims["ath"], "Z1P3Ll-e0JrOBqzfbrTXjd9Z_l-iiW1obnZMWdV1w1s");
    }
}

