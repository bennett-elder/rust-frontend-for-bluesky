mod oauth_flow;

use alt_first_core::{
    alt_policy, continue_thread_label, decode_uri_component as decode_hash_value,
    encode_uri_component as encode_uri, feed_end_note, new_post_count, placeholder_ratio,
    text_segments, unfetched_replies, video_alt_policy, ByteRange, EmbedRecordView, EmbedView,
    ExternalView, Facet, FacetFeature,
    LikesResponse, NotificationsResponse, PostsResponse, ProfileViewerState, QuotesResponse,
    ReplyRef, RepostedByResponse, SignInMode,
    SearchPostsResponse,
    TrendingTopic, TrendingTopicsResponse, notification_verb,
    FeedFailure, FeedFailureKind, FeedGeneratorsResponse, FeedResponse,
    FeedViewReason, ImageView, PostThreadResponse, PostView, PreferencesResponse, ProfileLink,
    discover_label, tab_title, ProfileViewDetailed, Subject, TextSegment, ThreadViewNode,
    TitleSubject, VideoView,
};
use dioxus::prelude::*;

const CSS: &str = include_str!("../assets/style.css");

pub const PUBLIC_API: &str = "https://api.bsky.app";

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaState {
    Collapsed,
    Thumbnail,
    Playing,
}

/// Global "alt text first" preference, shared through context.
#[derive(Clone, Copy)]
struct AltTextFirst(Signal<bool>);

/// Which palette to paint with. `System` leaves `<html>` without a `data-theme`
/// attribute so the CSS media query decides — that way the correct colours are there
/// before the app boots, and a host that flips appearance at sunset just follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn from_str(value: &str) -> Theme {
        match value.trim() {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

#[derive(Clone, Copy)]
struct ThemePref(Signal<Theme>);

/// App-password session, if signed in. Memory only.
#[derive(Clone, Copy)]
struct SessionState(Signal<Option<Session>>);

/// A like or a repost. Both are one record with a `subject`, both are deleted by
/// rkey, and both need the same optimistic treatment — so they share one path.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Reaction {
    Like,
    Repost,
}

impl Reaction {
    fn collection(self) -> &'static str {
        match self {
            Self::Like => "app.bsky.feed.like",
            Self::Repost => "app.bsky.feed.repost",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Repost => "repost",
        }
    }
}

/// Optimistic state for one reaction on one post. Kept in context so a card in a
/// feed, a thread and a profile all show the same thing without prop threading.
#[derive(Clone, PartialEq)]
struct ReactionView {
    on: bool,
    /// The viewer's own record; turning it off deletes this.
    record: Option<String>,
    count: i64,
}

/// Parent uri -> author handle, so a feed card can say "Replying to @who" without a
/// request of its own. Filled in one batched `getPosts` per loaded page.
#[derive(Clone, Copy)]
struct ParentHandles(Signal<std::collections::HashMap<String, Option<String>>>);

#[derive(Clone, Copy)]
struct ReactionCtx {
    /// Keyed by (reaction, post uri).
    state: Signal<std::collections::HashMap<(Reaction, String), ReactionView>>,
    /// Writes in flight. Those buttons are disabled, because a second tap used to
    /// race the first one's record and leave a like on the server that the UI had
    /// already shown as off.
    pending: Signal<std::collections::HashSet<(Reaction, String)>>,
    /// Last failure per (reaction, post), rendered under the card. A reaction that
    /// quietly flips back is the silent-failure class this project keeps a table for.
    errors: Signal<std::collections::HashMap<(Reaction, String), String>>,
}

/// The sign-in page's error channel, shared so a failure that happens *outside* the page —
/// an OAuth callback that could not be completed on load — still lands somewhere visible.
#[derive(Clone, Copy)]
struct LoginState(Signal<Option<String>>);

/// The signed-in account's feeds, shared with the sign-in page so a fresh session fills the
/// tab row without a reload.
#[derive(Clone, Copy)]
struct FeedsState(Signal<Vec<(String, String)>>);

/// Posts deleted this session, by at-uri. Deletion is local-first: the record is gone from the
/// repository, but cached feed pages still contain it, and re-showing a post you just deleted is
/// the sort of lie this app is supposed to avoid.
#[derive(Clone, Copy)]
struct DeletedPosts(Signal<std::collections::HashSet<String>>);

/// Handles and DIDs only; anything else never reaches an eval'd URL.
fn sanitize_actor(input: &str) -> Option<String> {
    let handle = input.trim().trim_start_matches('@');
    if handle.is_empty() || handle.len() > 253 {
        return None;
    }
    let allowed = handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'));
    allowed.then(|| handle.to_string())
}

/// A signed-in session, from either an app password or OAuth. In memory unless the reader
/// asked to be remembered — which is the default for OAuth and opt-in for app passwords.
#[derive(Clone, PartialEq, Eq)]
struct Session {
    handle: String,
    did: String,
    access_jwt: String,
    /// The PDS rotates this on every refresh; the old one must be discarded.
    refresh_jwt: String,
    /// Authenticated calls go to the account's PDS, not the public AppView.
    /// For `*.bsky.social` handles that is bsky.social; other hosts need DID
    /// resolution before this is correct.
    pds: String,
    /// Present for OAuth sessions: the key and endpoints that replace `refreshSession`.
    /// `None` means an app-password session, which is why every call site asks the session
    /// for its auth rather than reaching for `access_jwt` directly.
    oauth: Option<oauth_flow::OAuthSession>,
    /// Whether this session belongs on disk. Carried on the session itself because rotation
    /// happens in three places, and a rotation that forgot to re-save left the stored refresh
    /// token dead: refresh tokens are single-use, so the next load traded a stale one for a
    /// 400 and never recovered.
    remembered: bool,
}

impl Session {
    /// How to authenticate a request with this session.
    ///
    /// The distinction is not cosmetic: OAuth tokens go in an `Authorization: DPoP` header
    /// with a per-request proof, app-password tokens in `Authorization: Bearer`. Doing this
    /// once here is what keeps 20 call sites from having to know the difference.
    fn auth(&self) -> Option<oauth_flow::Auth<'_>> {
        match &self.oauth {
            None => Some(oauth_flow::Auth::Bearer(&self.access_jwt)),
            Some(oauth) => Some(oauth_flow::Auth::Dpop {
                token: &self.access_jwt,
                oauth,
            }),
        }
    }
}

/// One HTTP call through the webview's own `fetch`, so web and Android share it.
///
/// Credentials are attached only to calls we make; nothing logs them. What this client
/// writes is a fixed list — replies (on an existing post), likes, reposts, blocks and mutes,
/// and the deletes that undo them — so "does not post, follow or delete" holds by the absence
/// of such calls rather than by trust.
async fn request_json<T: serde::de::DeserializeOwned>(
    method: &str,
    url: &str,
    auth: Option<oauth_flow::Auth<'_>>,
    body: Option<&str>,
) -> Result<T, String> {
    let text = request_text(method, url, auth, body).await?;
    serde_json::from_str::<T>(&text).map_err(|err| format!("could not parse response: {err}"))
}

/// A call whose response body is not needed.
///
/// `app.bsky.graph.muteActor` answers with no body at all, so parsing it as JSON
/// failed and a successful mute looked like an error.
async fn request_ok(
    method: &str,
    url: &str,
    auth: Option<oauth_flow::Auth<'_>>,
    body: Option<&str>,
) -> Result<(), String> {
    request_text(method, url, auth, body).await.map(|_| ())
}

async fn request_text(
    method: &str,
    url: &str,
    auth: Option<oauth_flow::Auth<'_>>,
    body: Option<&str>,
) -> Result<String, String> {
    let mut headers = match auth {
        Some(auth) => oauth_flow::auth_headers(auth, method, url, None).await?,
        None => Vec::new(),
    };
    if body.is_some() {
        headers.push(("content-type".to_string(), "application/json".to_string()));
    }
    let mut response = oauth_flow::fetch_raw(method, url, &headers, body).await?;

    // A DPoP-bound request can be rejected once with `use_dpop_nonce`; the same request has
    // to be repeated with the nonce the server just handed out, and with a fresh proof.
    if matches!(response.status, 400 | 401) {
        if let (Some(nonce), Some(auth)) = (response.dpop_nonce.clone(), auth) {
            let mut retry_headers = oauth_flow::auth_headers(auth, method, url, Some(&nonce)).await?;
            if body.is_some() {
                retry_headers.push(("content-type".to_string(), "application/json".to_string()));
            }
            response = oauth_flow::fetch_raw(method, url, &retry_headers, body).await?;
        }
    }

    if !response.ok() {
        return Err(format!(
            "HTTP {} from {url}: {}",
            response.status, response.body
        ));
    }
    Ok(response.body)
}

async fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    request_json("GET", url, None, None).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedSession {
    access_jwt: String,
    refresh_jwt: String,
    did: String,
    handle: String,
}

/// App-password sign-in. Legacy auth path (bearer session), used while OAuth is
/// being built; the password is never stored, only exchanged for a session token.
async fn login(identifier: &str, password: &str, remembered: bool) -> Result<Session, String> {
    let body = serde_json::json!({ "identifier": identifier, "password": password }).to_string();
    let created: CreatedSession = request_json(
        "POST",
        "https://bsky.social/xrpc/com.atproto.server.createSession",
        None,
        Some(&body),
    )
    .await?;

    Ok(Session {
        handle: created.handle,
        did: created.did,
        access_jwt: created.access_jwt,
        refresh_jwt: created.refresh_jwt,
        pds: "https://bsky.social".to_string(),
        oauth: None,
        remembered,
    })
}

/// Rotates the token pair. Access tokens are short-lived (~2 h), so without this
/// a session dies mid-use with a bare 401.
async fn refresh(session: &Session) -> Result<Session, String> {
    // Another tab may already have rotated the pair. Refresh tokens are single-use, so using
    // ours here would spend a token the server has retired and get `400 invalid_grant` — the
    // two-tab failure. Adopt theirs instead: it is newer, and it is what storage holds.
    if session.remembered {
        if let Some(stored) = load_stored_session().await {
            if stored.did == session.did && stored.refresh_jwt != session.refresh_jwt {
                log_to_console("adopted a session another tab had already rotated");
                return Ok(stored);
            }
        }
    }

    // OAuth sessions do not talk to `refreshSession`: the token endpoint rotates the pair and
    // the new refresh token must replace the old one, or the next refresh fails.
    if let Some(oauth) = &session.oauth {
        let (access_jwt, refresh_jwt) = oauth_flow::refresh(oauth, &session.refresh_jwt).await?;
        return Ok(Session {
            handle: session.handle.clone(),
            did: session.did.clone(),
            access_jwt,
            refresh_jwt,
            pds: session.pds.clone(),
            oauth: Some(oauth.clone()),
            remembered: session.remembered,
        });
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Refreshed {
        access_jwt: String,
        refresh_jwt: String,
        did: String,
        handle: String,
    }

    let refreshed: Refreshed = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.server.refreshSession", session.pds),
        Some(oauth_flow::Auth::Bearer(&session.refresh_jwt)),
        None,
    )
    .await?;

    Ok(Session {
        handle: refreshed.handle,
        did: refreshed.did,
        access_jwt: refreshed.access_jwt,
        refresh_jwt: refreshed.refresh_jwt,
        pds: session.pds.clone(),
        oauth: None,
        remembered: session.remembered,
    })
}

/// The session in the signal, when it is no longer the one that just 401'd — another read, or
/// another tab through `refresh`'s storage check, may already have replaced it.
fn newer_than(state: SessionState, stale: &Session) -> Option<Session> {
    (state.0)().filter(|current| current.refresh_jwt != stale.refresh_jwt)
}

/// Cross-tab notice that a rotation happened.
///
/// Refresh tokens are single-use, so two tabs that rotate at the same moment spend one token
/// twice and the loser is refused. Its session is not dead — the winner's pair is already in
/// storage — so the loser only needs to know that, and it must not sign the reader out.
///
/// Not a lock: a lock needs an election, a lease with an expiry, and a decision about what to do
/// when the holder's tab dies mid-rotation. This only narrows the wait for a storage read that
/// works with or without the API.
const AUTH_CHANNEL_JS: &str = r#"
window.__rustyAuth = window.__rustyAuth || (() => {
  let channel = null;
  try { channel = new BroadcastChannel('altfirst-auth'); } catch (error) {}
  const waiters = [];
  if (channel) {
    channel.onmessage = (event) => {
      const message = (event && event.data) || {};
      // Only a finished rotation matters: it means storage holds a newer pair than this tab's.
      if (message.type === 'rotated') {
        for (const resolve of waiters.splice(0)) resolve(message);
      }
    };
  }
  return {
    announce: (message) => { try { if (channel) channel.postMessage(message); } catch (error) {} },
    next: (timeoutMs) => new Promise((resolve) => {
      if (!channel) { resolve({ type: 'unsupported' }); return; }
      const done = (message) => { clearTimeout(timer); resolve(message); };
      const timer = setTimeout(() => {
        const index = waiters.indexOf(done);
        if (index >= 0) waiters.splice(index, 1);
        resolve({ type: 'timeout' });
      }, timeoutMs);
      waiters.push(done);
    }),
  };
})();
"#;

/// Install the cross-tab channel once, before any rotation can run.
async fn install_auth_channel() {
    let _ = document::eval(AUTH_CHANNEL_JS).recv::<String>().await;
}

/// Tell other tabs that this one is rotating, or that it has finished.
///
/// Best effort: without `BroadcastChannel` nothing is announced and every caller behaves as it
/// did before, because the storage read is the actual mechanism.
fn announce_auth(kind: &str) {
    document::eval(&format!(
        "window.__rustyAuth && window.__rustyAuth.announce({{ type: {} }})",
        serde_json::to_string(kind).unwrap()
    ));
}

/// Wait for another tab's rotation, up to `timeout_ms`. `None` on timeout or without the API.
async fn next_auth_message(timeout_ms: u64) -> Option<serde_json::Value> {
    let mut eval = document::eval(&format!(
        r#"(() => {{
            const send = (message) => dioxus.send(JSON.stringify(message));
            if (!window.__rustyAuth) {{ send({{ type: 'unsupported' }}); return; }}
            window.__rustyAuth.next({timeout_ms}).then(send);
        }})()"#
    ));
    let raw: String = eval.recv().await.ok()?;
    serde_json::from_str(&raw).ok()
}

/// Adopt the pair another tab rotated to, if storage holds one this tab has not seen.
///
/// `None` means nobody else produced one: the grant really is dead, and the caller may sign out.
async fn adopt_rotation(mut state: SessionState, stale: &Session) -> Option<Session> {
    let stored = load_stored_session().await?;
    if stored.refresh_jwt == stale.refresh_jwt {
        return None;
    }
    // Already on disk — the tab that rotated persisted it, which is why this can be adopted
    // without another token-endpoint round trip.
    state.0.set(Some(stored.clone()));
    Some(stored)
}

/// Rotate the pair once, publish it, and persist it.
///
/// Persisting is part of rotating rather than an afterthought: refresh tokens are single-use,
/// so a rotation that is not saved leaves the stored token dead and the next reload answers
/// `400 invalid_grant` forever.
///
/// `refresh` already adopts a pair another tab stored (that is its first step, for a remembered
/// session). This handles the harder case — both tabs rotate at the same instant — by waiting
/// for the winner's announcement and reading what it saved. Returns the refresh error when
/// nobody else rotated, so callers can tell a dead grant from a race.
async fn rotate_session(mut state: SessionState, stale: &Session) -> Result<Session, String> {
    if let Some(fresh) = newer_than(state, stale) {
        return Ok(fresh);
    }
    announce_auth("rotating");
    match refresh(stale).await {
        Ok(fresh) => {
            state.0.set(Some(fresh.clone()));
            persist_session(Some(&fresh), fresh.remembered);
            // Announced after persisting, so a tab that adopts on this message cannot read the
            // pair it was already holding.
            announce_auth("rotated");
            Ok(fresh)
        }
        Err(error) => {
            let _ = next_auth_message(1500).await;
            match adopt_rotation(state, stale).await {
                Some(fresh) => Ok(fresh),
                None => Err(error),
            }
        }
    }
}

/// True when a 401 means the access token is no longer good — as opposed to a DPoP nonce or
/// proof problem.
///
/// The distinction matters because rotating costs a single-use refresh token. A PDS answers a
/// stale cached nonce with `401 use_dpop_nonce` and a fresh `DPoP-Nonce`, which `request_text`
/// retries by itself; treating that as expiry would spend a token on a clock that drifted, and
/// on a page with two parallel reads it would do it twice.
fn is_expired_token(error: &str) -> bool {
    if error.contains("use_dpop_nonce") || error.contains("invalid_dpop_proof") {
        return false;
    }
    ["ExpiredToken", "invalid_token", "InvalidToken", "AuthMissing"]
        .iter()
        .any(|marker| error.contains(marker))
}

/// One authenticated GET against the account's PDS, rotating an expired access token once.
///
/// Reads used to skip this. The restore path deliberately does not rotate on load — it cost a
/// round trip per reload, invalidated the DPoP nonce cached for the old token, and raced other
/// tabs for a single-use refresh token — on the promise that "a request that needs a new token
/// still refreshes on its own 401". Feed loads and likes kept that promise; reads did not. So a
/// session older than the ~2 h access-token lifetime answered 401 on the first PDS read after a
/// reload, `viewer_read` fell back to the public AppView, and that answers `viewer: null`:
/// hearts read as unliked, and tapping one created a *second* like record.
///
/// A failed rotation is reported, never acted on: signing the reader out is a decision for a
/// write or a feed load, not for a background read.
async fn authed_get<T: serde::de::DeserializeOwned>(
    path: &str,
    state: SessionState,
) -> Result<T, String> {
    let Some(existing) = (state.0)() else {
        return Err("Sign in to read this".into());
    };
    match authed_get_with::<T>(path, &existing).await {
        Ok(value) => Ok(value),
        Err(original) => {
            if !is_expired_token(&original) {
                return Err(original);
            }
            match rotate_session(state, &existing).await {
                Ok(fresh) => authed_get_with::<T>(path, &fresh).await,
                Err(_) => Err(original),
            }
        }
    }
}

/// The bare authenticated GET, shared by the first attempt and the retry so the two cannot
/// drift in URL shape or in which header carries the token.
async fn authed_get_with<T: serde::de::DeserializeOwned>(
    path: &str,
    session: &Session,
) -> Result<T, String> {
    request_json(
        "GET",
        &format!("{}{path}", session.pds),
        session.auth(),
        None,
    )
    .await
}

/// Discover is public; Following requires a session. Every other tab comes from
/// the account's saved feeds — no one's personal feed is baked in here.
const DISCOVER_FEED: &str =
    "at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot";

/// App location. Hash routes keep GitHub Pages happy (no rewrite rules) and give
/// the browser real history entries, so back/forward and Android's back gesture
/// work instead of exiting the app.
#[derive(Clone, PartialEq, Eq)]
enum Route {
    Home,
    Following,
    /// The signed-in account's feeds, listed on their own page.
    Feeds,
    /// Reading preferences, on their own page.
    Settings,
    /// Writing a new top-level post, on its own page.
    Compose,
    /// Everything posted with one hashtag: `#/tag/<tag>`.
    Hashtag { tag: String },
    /// The way in: app password or OAuth, on their own page rather than in the header.
    SignIn,
    Feed { uri: String },
    Profile { actor: String },
    Post { uri: String },
    /// The signed-in account's notifications.
    Notifications,
    /// Who liked a post: `#/post/<uri>/likes`.
    PostLikes { uri: String },
    /// Who reposted it: `#/post/<uri>/reposts`.
    PostReposts { uri: String },
    /// The posts quoting it: `#/post/<uri>/quotes`.
    PostQuotes { uri: String },
}

fn parse_hash(hash: &str) -> Route {
    let path = hash.trim_start_matches('#').trim_start_matches('/');
    let (head, rest) = path.split_once('/').unwrap_or((path, ""));

    match head {
        "following" => Route::Following,
        "feeds" => Route::Feeds,
        "notifications" => Route::Notifications,
        "settings" => Route::Settings,
        "compose" => Route::Compose,
        "tag" if !rest.is_empty() => Route::Hashtag {
            tag: decode_hash_value(rest),
        },
        "sign-in" => Route::SignIn,
        "feed" if !rest.is_empty() => Route::Feed {
            uri: decode_hash_value(rest),
        },
        "profile" if !rest.is_empty() => Route::Profile {
            actor: decode_hash_value(rest),
        },
        "post" if !rest.is_empty() => {
            // `?from=` records the feed the post was opened from; a tab opened
            // cold has no history for Back to use, so the URL carries it instead.
            let (path, _) = rest.split_once('?').unwrap_or((rest, ""));
            let mut parts = path.splitn(2, '/');
            let uri = parts.next().unwrap_or_default().to_string();
            match parts.next() {
                Some("likes") => Route::PostLikes {
                    uri: decode_hash_value(&uri),
                },
                Some("reposts") => Route::PostReposts {
                    uri: decode_hash_value(&uri),
                },
                Some("quotes") => Route::PostQuotes {
                    uri: decode_hash_value(&uri),
                },
                // Anything else is not a view we have: show the thread.
                _ => Route::Post {
                    uri: decode_hash_value(&uri),
                },
            }
        }
        _ => Route::Home,
    }
}

/// Canonical hash for a route, used when recording where a card came from.
fn route_hash(route: &Route) -> String {
    match route {
        Route::Home => "#/".to_string(),
        Route::Following => "#/following".to_string(),
        Route::Feeds => "#/feeds".to_string(),
        Route::Notifications => "#/notifications".to_string(),
        Route::Settings => "#/settings".to_string(),
        Route::Compose => "#/compose".to_string(),
        Route::Hashtag { tag } => format!("#/tag/{}", encode_uri(tag)),
        Route::SignIn => "#/sign-in".to_string(),
        Route::Feed { uri } => format!("#/feed/{}", encode_uri(uri)),
        Route::Profile { actor } => format!("#/profile/{}", encode_uri(actor)),
        Route::Post { uri } => format!("#/post/{}", encode_uri(uri)),
        Route::PostLikes { uri } => format!("#/post/{}/likes", encode_uri(uri)),
        Route::PostReposts { uri } => format!("#/post/{}/reposts", encode_uri(uri)),
        Route::PostQuotes { uri } => format!("#/post/{}/quotes", encode_uri(uri)),
    }
}

/// Post link, optionally remembering the feed it is being opened from.
fn post_href(uri: &str, origin: &str) -> String {
    let base = format!("#/post/{}", encode_uri(uri));
    if origin.is_empty() {
        base
    } else {
        format!("{base}?from={}", encode_uri(origin))
    }
}

/// Push a new history entry. The hashchange listener then updates `Route`, so
/// there is exactly one source of truth for the current location.
fn navigate(hash: &str) {
    document::eval(&format!(
        "if (location.hash !== {hash}) location.hash = {hash};",
        hash = serde_json::to_string(hash).unwrap()
    ));
}

fn go_back() {
    document::eval(
        r#"(() => {
            if (history.length > 1) {
                history.back();
                return;
            }
            // Opened cold (new tab, pasted link): history cannot help, so fall
            // back to the feed recorded in the URL, then to the last feed read.
            const carried = new URLSearchParams(location.hash.split('?')[1] || '').get('from');
            location.hash =
                carried || localStorage.getItem('altfirst.lastFeed') || '#/';
        })()"#,
    );
}

/// Relative times on cards, exact local times where they should be exact.
///
/// Doing this in JS rather than in Rust buys the reader's locale and timezone for free,
/// and `Intl.RelativeTimeFormat` handles pluralisation and direction. Rust renders the
/// static date as a fallback so a card is never blank before this runs.
fn install_when() {
    document::eval(
        r#"(() => {
            if (window.__altWhen) return;
            window.__altWhen = true;

            const relative = (date) => {
                const seconds = Math.round((date.getTime() - Date.now()) / 1000);
                const ahead = seconds > 0;
                const magnitude = Math.abs(seconds);
                const format = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto', style: 'narrow' });
                if (magnitude < 60) return ahead ? format.format(1, 'minute') : 'now';
                if (magnitude < 3600) return format.format(Math.round(seconds / 60), 'minute');
                if (magnitude < 86400) return format.format(Math.round(seconds / 3600), 'hour');
                if (magnitude < 7 * 86400) return format.format(Math.round(seconds / 86400), 'day');
                // Older than a week: the date is more use than "38d". A different year
                // gets the year, or two Decembers look identical.
                const sameYear = date.getFullYear() === new Date().getFullYear();
                return date.toLocaleDateString(undefined, {
                    month: 'short',
                    day: 'numeric',
                    ...(sameYear ? {} : { year: 'numeric' }),
                });
            };

            const absolute = (date) =>
                date.toLocaleString(undefined, {
                    month: 'short',
                    day: 'numeric',
                    year: 'numeric',
                    hour: 'numeric',
                    minute: '2-digit',
                });

            const paint = () => {
                for (const node of document.querySelectorAll('[data-when]')) {
                    const raw = node.getAttribute('data-when');
                    if (!raw) continue;
                    const date = new Date(raw);
                    if (Number.isNaN(date.getTime())) continue;
                    // The exact time is always one hover away, whatever the visible form.
                    node.title = absolute(date);
                    node.textContent = node.getAttribute('data-format') === 'absolute'
                        ? absolute(date)
                        : '· ' + relative(date);
                }
            };

            paint();
            // Relative text goes stale, and cards arrive after fetch, so repaint on DOM
            // changes and on a timer.
            let scheduled = 0;
            const schedule = () => {
                if (scheduled) return;
                scheduled = setTimeout(() => {
                    scheduled = 0;
                    paint();
                }, 150);
            };
            new MutationObserver(schedule).observe(document.body, { childList: true, subtree: true });
            setInterval(paint, 30000);
        })()"#,
    );
}


///
/// The saved offset is the point of the scroll memory, and it is also what makes a feed
/// feel stuck: it reopens 900 px down with no shortcut back up. Two modes:
///
/// * `data-scroll-top="true"` — this element *is* the current route (the active tab), so
///   the second click is taken in the capture phase, before the bubble-phase navigation
///   listener can treat it as a fresh navigation, and the page scrolls to the top.
/// * `data-scroll-top="home"` — the Home item, which may point somewhere else. The click
///   navigates as usual, but the feed is asked not to restore its saved offset, so the
///   reader lands at the top of Home instead of 900 px into it.
fn install_double_click_top() {
    document::eval(
        r#"(() => {
            if (window.__altDoubleTop) return;
            window.__altDoubleTop = true;
            const WINDOW_MS = 420;
            const KEY = 'altfirst.scroll';
            // Same key normalisation as the scroll memory, or the jump writes an offset
            // the restore will never read.
            const routeKey = () =>
                (window.__altRouteKey ? window.__altRouteKey() : location.hash.split('?')[0]);
            const store = () => {
                try { return JSON.parse(localStorage.getItem(KEY) || '{}') || {}; }
                catch { return {}; }
            };
            const forget = (route) => {
                try {
                    const next = store();
                    delete next[route];
                    localStorage.setItem(KEY, JSON.stringify(next));
                } catch {}
            };
            const rememberTop = (route) => {
                try {
                    const next = store();
                    next[route] = { y: 0, t: Date.now() };
                    localStorage.setItem(KEY, JSON.stringify(next));
                } catch {}
            };
            const jump = () => {
                const smooth = !matchMedia('(prefers-reduced-motion: reduce)').matches;
                window.scrollTo({ top: 0, behavior: smooth ? 'smooth' : 'auto' });
            };
            document.addEventListener('click', (event) => {
                const target = event.target;
                if (!(target instanceof Element)) return;
                const current = target.closest('[data-scroll-top]');
                const mode = current && current.getAttribute('data-scroll-top');
                if (mode !== 'true' && mode !== 'home') return;
                const now = Date.now();
                const previous = Number(current.dataset.scrollTopAt || 0);
                current.dataset.scrollTopAt = String(now);
                if (!previous || now - previous > WINDOW_MS) return;
                current.dataset.scrollTopAt = '0';

                if (mode === 'true') {
                    // Already here: take the click instead of navigating to this route.
                    event.preventDefault();
                    event.stopPropagation();
                    jump();
                    // Write the offset now: the debounced scroll listener would otherwise
                    // keep the old value if the tab closes mid-animation.
                    rememberTop(routeKey());
                    return;
                }

                // Home. Let the click navigate; the point is only that the destination
                // does not restore its old offset. `restore` consumes this flag, and a
                // timeout clears it in case no restore ever runs (signed out, Home opens
                // the sign-in panel instead of a feed).
                window.__altSkipRestore = true;
                forget('#/following');
                setTimeout(jump, 60);
                setTimeout(jump, 600);
                setTimeout(() => { window.__altSkipRestore = false; }, 4000);
            }, true);
        })()"#,
    );
}

///
/// One document-wide "close the menus" signal, plus a hook for menus to receive it.
///
/// The open/closed state lives in Rust, so a JS listener cannot flip it directly: this
/// dispatches one custom event, and every *open* menu has registered an eval that turns
/// that event into a message back into Rust. Menus that are closed register nothing, so
/// a feed of fifty cards carries a single global listener rather than a hundred.
fn install_menu_dismiss() {
    document::eval(
        r#"(() => {
            if (window.__altMenuDismiss) return;
            window.__altMenuDismiss = true;
            const dismiss = () => document.dispatchEvent(new CustomEvent('alt-first:dismiss-menus'));
            window.__altDismissMenus = dismiss;
            // Capture phase, so every click closes whatever is open *before* the click
            // does its own work. Clicks inside an open menu are exempt: closing it there
            // would unmount the very button being pressed, mid-event. A click on a
            // toggle therefore dismisses the other menus synchronously — the toggle's
            // own handler then opens its menu.
            document.addEventListener('click', (event) => {
                const target = event.target;
                if (target instanceof Element && target.closest('.post__menu')) return;
                dismiss();
            }, true);
            document.addEventListener('keydown', (event) => {
                if (event.key === 'Escape') dismiss();
            }, true);
        })()"#,
    );
}

/// Drag the feed strip with a mouse, the way the official app lets you.
///
/// The strip is a horizontal scroller with its scrollbar hidden, which on a touchscreen is fine
/// — you swipe it. A mouse has no swipe, so the tabs past the right edge were reachable only by
/// tabbing to them. This adds pointer drag (mouse only; touch keeps its native momentum
/// scrolling), swallows the click that ends a drag so a drag does not open the tab under the
/// cursor, and lets the arrow keys walk the row when one of its items has focus.
fn install_feed_strip_drag() {
    document::eval(
        r#"(() => {
            if (window.__altStripDrag) return;
            window.__altStripDrag = true;
            let drag = null;
            let suppressClickUntil = 0;

            document.addEventListener('pointerdown', (event) => {
                if (event.pointerType !== 'mouse' || event.button !== 0) return;
                const target = event.target;
                if (!(target instanceof Element)) return;
                const strip = target.closest('.feed-tabs');
                if (!strip || strip.scrollWidth <= strip.clientWidth + 1) return;
                drag = { strip, startX: event.clientX, startScroll: strip.scrollLeft, moved: false };
            });

            document.addEventListener('pointermove', (event) => {
                if (!drag) return;
                const dx = event.clientX - drag.startX;
                if (!drag.moved && Math.abs(dx) < 4) return;
                drag.moved = true;
                drag.strip.classList.add('feed-tabs--dragging');
                drag.strip.scrollLeft = drag.startScroll - dx;
            });

            const finish = () => {
                if (!drag) return;
                if (drag.moved) {
                    drag.strip.classList.remove('feed-tabs--dragging');
                    // The click that follows pointerup belongs to the drag, not to the tab
                    // that happens to be under the cursor when it ends.
                    suppressClickUntil = Date.now() + 350;
                }
                drag = null;
            };
            document.addEventListener('pointerup', finish);
            document.addEventListener('pointercancel', finish);

            document.addEventListener('click', (event) => {
                if (Date.now() >= suppressClickUntil) return;
                const target = event.target;
                if (!(target instanceof Element) || !target.closest('.feed-tabs')) return;
                event.preventDefault();
                event.stopPropagation();
            }, true);

            document.addEventListener('keydown', (event) => {
                if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
                const target = event.target;
                if (!(target instanceof Element)) return;
                const item = target.closest('.feed-tabs > *');
                if (!item) return;
                const items = [...item.parentElement.children];
                const next = items[items.indexOf(item) + (event.key === 'ArrowRight' ? 1 : -1)];
                if (!next) return;
                event.preventDefault();
                next.focus();
                next.scrollIntoView({ block: 'nearest', inline: 'nearest' });
            }, true);
        })()"#,
    );
}

/// Close this menu when the document says so (outside click, Escape, another menu).
fn use_menu_dismiss(mut open: Signal<bool>) {
    use_effect(move || {
        if !open() {
            return;
        }
        spawn(async move {
            let mut eval = document::eval(
                r#"(() => {
                    const handler = () => {
                        document.removeEventListener('alt-first:dismiss-menus', handler);
                        dioxus.send(true);
                    };
                    document.addEventListener('alt-first:dismiss-menus', handler);
                })()"#,
            );
            if eval.recv::<bool>().await.is_ok() {
                open.set(false);
            }
        });
    });
}

/// Wrapping a post in `<a>` makes a drag-select start a link drag, which is why
/// selection died. Instead one delegated listener navigates only for a plain
/// click on non-interactive content with no live selection.
fn install_click_navigation() {
    document::eval(
        r#"(() => {
            if (window.__altFirstClickNav) return;
            window.__altFirstClickNav = true;
            document.addEventListener('click', (event) => {
                const target = event.target;
                if (!(target instanceof Element)) return;
                if (event.defaultPrevented || event.button !== 0) return;
                if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

                // In-app anchors navigate through JS rather than the browser default.
                // Android's WebView does not follow fragment-only `href`s here, so the
                // same link works on desktop and silently does nothing on device.
                const anchor = target.closest("a[href^='#/']");
                if (anchor) {
                    const href = anchor.getAttribute('href');
                    if (href && location.hash !== href) {
                        event.preventDefault();
                        location.hash = href;
                    }
                    return;
                }

                const post = target.closest('[data-post-link]');
                if (!post) return;
                if (target.closest('a, button, video, img, input, label, .facet')) return;
                const link = post.getAttribute('data-post-link');
                if (!link) return;
                // The card is a div so post text stays selectable, which also means
                // the browser has no link to fall back on. A modified click would
                // otherwise do nothing at all, so open the tab ourselves.
                if (event.metaKey || event.ctrlKey || event.shiftKey) {
                    window.open(link, '_blank', 'noopener');
                    return;
                }
                if (event.altKey) return;
                const selection = window.getSelection();
                if (selection && selection.toString().trim().length > 0) return;
                if (location.hash !== link) location.hash = link;
            }, { capture: true }); // capture: Dioxus preventDefaults on bubble, which
            // made `event.defaultPrevented` true before this handler ever ran.
            // Middle-click: the other way everyone expects a link to open.
            document.addEventListener('auxclick', (event) => {
                if (event.button !== 1) return;
                const target = event.target;
                if (!(target instanceof Element)) return;
                if (target.closest('a, button, video, img, input, label, .facet')) return;
                const link = target.closest('[data-post-link]')?.getAttribute('data-post-link');
                if (link) window.open(link, '_blank', 'noopener');
            });
        })()"#,
    );
}

/// Remembers `scrollY` per route and puts it back after the feed re-renders.
///
/// Needed because opening a post unmounts the list: without this, returning to a
/// feed you had scrolled three pages into drops you at the top of page one. The
/// offset is per hash, so each feed keeps its own place.
///
/// Offsets live in `localStorage`, not just memory, so a tab opened cold (cmd-click,
/// or Back in that new tab) can return to the same spot — which means it has to
/// refetch the pages needed to make that spot reachable.
fn install_scroll_memory() {
    document::eval(
        r#"(() => {
            if (window.__altScrollMemory) return;
            window.__altScrollMemory = true;
            const KEY = 'altfirst.scroll';
            const DAY = 864e5;
            let store = {};
            try { store = JSON.parse(localStorage.getItem(KEY) || '{}') || {}; } catch { store = {}; }
            const prune = () => {
                const entries = Object.entries(store)
                    .filter(([, v]) => v && Date.now() - (v.t || 0) < 2 * DAY)
                    .sort((a, b) => (b[1].t || 0) - (a[1].t || 0))
                    .slice(0, 8);
                store = Object.fromEntries(entries);
            };
            const flush = () => {
                prune();
                try { localStorage.setItem(KEY, JSON.stringify(store)); } catch {}
            };
            // One key per route. `location.hash` keeps whatever encoding the URL happened
            // to carry, and the same feed arrived sometimes encoded
            // (`#/feed/at%3A%2F%2F…`) and sometimes raw (`#/feed/at://…`), so the offset
            // was saved under one key and looked up under the other — the feed then
            // forgot where it was, or restored a different feed's offset.
            const routeKey = () => {
                const raw = location.hash.split('?')[0];
                try { return decodeURIComponent(raw); } catch { return raw; }
            };
            window.__altRouteKey = routeKey;

            let timer = 0;
            let restoring = false;
            window.addEventListener('scroll', () => {
                // Timers, not rAF: a backgrounded tab throttles animation frames, so
                // offsets would silently stop being recorded. Gating on a "restoring"
                // flag was worse still: one stuck flag muted saving for the session.
                // The restore only scrolls *to* the saved offset, so letting it write
                // that same value back is harmless.
                if (timer) return;
                timer = setTimeout(() => {
                    timer = 0;
                    store[routeKey()] = { y: Math.round(window.scrollY), t: Date.now() };
                    flush();
                }, 120);
            }, { passive: true });

            window.__altScrollRestore = () => {
                // A double-click on Home asked for the top: land there once, then hand
                // normal behaviour back so later reloads still restore.
                if (window.__altSkipRestore) {
                    window.__altSkipRestore = false;
                    window.scrollTo(0, 0);
                    return;
                }
                const entry = store[routeKey()];
                if (!entry || !entry.y) return;
                const target = entry.y;
                const deadline = performance.now() + 15000;
                const reachable = target - 40;
                const finish = () => {
                    restoring = false;
                    window.__altScrollQuietUntil = Date.now() + 800;
                };
                const maxScroll = () =>
                    document.documentElement.scrollHeight - window.innerHeight;
                if (maxScroll() >= reachable) {
                    window.scrollTo(0, target);
                    return;
                }
                restoring = true;
                // Keep auto-load quiet: a restored deep offset is near the bottom
                // of a shorter document, which would otherwise shift the page.
                window.__altScrollQuietUntil = Date.now() + 1200;
                let lastHeight = 0;
                let clicks = 0;
                let lastClick = performance.now();
                const tick = () => {
                    try {
                        if (maxScroll() >= reachable) {
                            window.scrollTo(0, target);
                            finish();
                            return;
                        }
                        const now = performance.now();
                        if (now > deadline) return finish();
                        // Too short to reach the offset: a cold tab only holds page
                        // one, so pull pages until the document is tall enough.
                        const button = document.querySelector('.load-more--auto');
                        if (!button) return finish();
                        if (!button.disabled && clicks < 12 && now - lastClick > 700) {
                            lastClick = now;
                            clicks += 1;
                            button.click();
                        }
                        // Nothing is arriving and nothing will: stop asking.
                        if (document.documentElement.scrollHeight === lastHeight && now - lastClick > 4000) {
                            return finish();
                        }
                        lastHeight = document.documentElement.scrollHeight;
                        setTimeout(tick, 120);
                    } catch {
                        // Never leave `restoring` wedged: that would mute saving.
                        finish();
                    }
                };
                setTimeout(tick, 60);
                // Watchdog: however this loop dies, saving must resume.
                setTimeout(() => {
                    restoring = false;
                }, 16000);
            };
            window.__altScrollDebug = () => ({
                restoring,
                keys: Object.keys(store),
                y: Math.round(window.scrollY),
            });
        })()"#,
    );
}

/// Put the reader back where they were after a cached feed re-renders.
fn restore_scroll() {
    document::eval("window.__altScrollRestore?.()");
}

/// Jump to the top after swapping in a fresher first page.
fn scroll_to_top() {
    document::eval("window.scrollTo(0, 0);");
}

/// Endless scroll: when the feed nears the bottom, trigger the hidden load-more
/// button. Programmatic clicks need no pointer events, so the control can stay
/// invisible while remaining keyboard-accessible via its own focus order.
fn install_auto_load() {
    document::eval(
        r#"(() => {
            if (window.__altFirstAutoLoad) return;
            window.__altFirstAutoLoad = true;
            let lastClick = 0;
            const nearBottom = () =>
                window.innerHeight + window.scrollY >=
                document.documentElement.scrollHeight - 700;
            const maybeLoad = () => {
                if (Date.now() < (window.__altScrollQuietUntil || 0)) return;
                if (!nearBottom()) return;
                const button = document.querySelector('.load-more--auto');
                if (!button || button.disabled) return;
                const now = Date.now();
                if (now - lastClick < 900) return;
                lastClick = now;
                button.click();
            };
            window.addEventListener('scroll', maybeLoad, { passive: true });
            new MutationObserver(maybeLoad).observe(document.documentElement, {
                childList: true,
                subtree: true,
            });
            // Catches the case where the first page is shorter than the viewport,
            // so no scroll event ever fires.
            setInterval(maybeLoad, 1200);

            // Re-check the feed for new posts: on refocus, and every minute while
            // the tab is visible. Without this, a feed you stay on never learns
            // that anything was posted and the "new posts" affordance never appears.
            const tickRefresh = () => {
                if (document.hidden) return;
                document.querySelector('.refresh-tick')?.click();
            };
            window.addEventListener('focus', tickRefresh);
            document.addEventListener('visibilitychange', tickRefresh);
            setInterval(tickRefresh, 60000);
            maybeLoad();
        })()"#,
    );
}

/// Remember the chosen feed and go there.
fn navigate_feed(hash: &str) {
    remember_feed(hash);
    navigate(hash);
}

/// Tab id for the current route, used to mark the active tab.
fn route_key(route: &Route) -> String {
    match route {
        // Not the generator key: the root route is the Explore page, and sharing a
        // key with the trending feed lit that tab up while the reader was on Explore.
        Route::Home => "explore".to_string(),
        Route::Following => "following".to_string(),
        Route::Feeds => "feeds".to_string(),
        Route::Notifications => "notifications".to_string(),
        Route::Settings => "settings".to_string(),
        Route::Compose => "compose".to_string(),
        Route::Hashtag { tag } => format!("tag:{tag}"),
        Route::SignIn => "sign-in".to_string(),
        Route::Feed { uri } => format!("custom:{uri}"),
        Route::Profile { actor } => format!("profile:{actor}"),
        Route::Post { uri } => format!("post:{uri}"),
        Route::PostLikes { uri } => format!("post:{uri}:likes"),
        Route::PostReposts { uri } => format!("post:{uri}:reposts"),
        Route::PostQuotes { uri } => format!("post:{uri}:quotes"),
    }
}

fn label_for_uri(uri: &str, saved: &[(String, String)]) -> String {
    saved
        .iter()
        .find(|(saved_uri, _)| saved_uri == uri)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| {
            uri.rsplit('/')
                .next()
                .unwrap_or("feed")
                .chars()
                .take(14)
                .collect()
        })
}

/// The name this deployment goes by.
///
/// `APP_NAME` at build time, else the default. Baked in rather than fetched, so the
/// header, the rail brand and every tab title are right in the first frame — and so a
/// second deployment can call itself something else without editing source. The
/// workflow feeds this from a repository variable; see README, "Naming a deploy".
const APP_NAME: &str = match option_env!("APP_NAME") {
    Some(name) => name,
    None => "Rusty",
};

/// Where "fork me" points. Overridable, because a fork of this app may want the link to
/// point at its own repository rather than upstream.
const REPO_URL: &str = match option_env!("REPO_URL") {
    Some(url) => url,
    None => "https://github.com/bennett-elder/rust-frontend-for-bluesky",
};

/// Which sign-in paths this build offers.
///
/// A deployment choice, like `APP_NAME`: app passwords need no configuration, while OAuth
/// needs a client-metadata document naming this exact URL, so "fork with no variables" and
/// "OAuth enabled" are honestly different builds. `OAUTH_MODE` selects between them
/// (`off` | `app-password`, `both`, `on` | `oauth`); anything unrecognised falls
/// back to app passwords, which is the path that cannot be misconfigured.
fn sign_in_mode() -> SignInMode {
    option_env!("OAUTH_MODE")
        .and_then(SignInMode::parse)
        .unwrap_or_default()
}

/// Whether the OAuth round trip exists in this build.
///
/// A fact about the code, not a setting: the flag is wired before the flow is, so a deployment
/// can be told apart from a fork without offering a button that cannot work. Flipping this to
/// `true` is the commit that makes `OAUTH_MODE` mean what it says; until then every mode
/// renders the app-password form.
const OAUTH_IMPLEMENTED: bool = true;

/// What the form admits about itself, which differs per build — and is how a deployer can see
/// that their `OAUTH_MODE` variable actually reached the wasm.
fn sign_in_note() -> &'static str {
    match (sign_in_mode(), OAUTH_IMPLEMENTED) {
        (SignInMode::AppPasswordOnly, _) => {
            "App password only, kept in memory. Revoke it any time in Bluesky settings."
        }
        (_, false) => {
            "App passwords only in this build. OAuth is selected for this deployment but the flow is not finished yet."
        }
        (SignInMode::Both, true) => {
            "App-password sign-ins are cleared when you close the tab unless you ask to be remembered. OAuth sign-ins are revoked when you sign out."
        }
        (SignInMode::OAuthPrimary, true) => {
            "Sign in with Bluesky. You can revoke this app in Bluesky settings at any time."
        }
    }
}


/// The name line carries a link mid-sentence — "a Dioxus Bluesky Client you can *fork for
/// yourself* to customize" — so the sentence lives in the markup rather than in a constant
/// whose halves would have to be kept in step.

/// Keep the tab title in step with the route so tabs are tellable apart.
///
/// The generated `index.html` only carries a static title (and dx's default is
/// "dioxus | ⛺"), so this is what actually distinguishes one tab from another.
fn update_title(route: &Route, label: &str) {
    // Every route goes through the same builder: Home must use the resolved label
    // (Discover when signed in, What's Hot when anonymous), never a literal.
    let subject = match route {
        Route::Home => TitleSubject::Explore,
        Route::Feed { .. } => TitleSubject::Feed { label },
        Route::Following => TitleSubject::Following,
        Route::Feeds => TitleSubject::Feeds,
        Route::Notifications => TitleSubject::Notifications,
        Route::Settings => TitleSubject::Settings,
        Route::Compose => TitleSubject::Compose,
        Route::Hashtag { tag } => TitleSubject::Hashtag { tag: tag.clone() }, 
        Route::SignIn => TitleSubject::SignIn,
        Route::Profile { actor } => TitleSubject::Profile { handle: actor },
        Route::Post { .. } => TitleSubject::Post,
        Route::PostLikes { .. } => TitleSubject::Engagement { label: "Likes" },
        Route::PostReposts { .. } => TitleSubject::Engagement { label: "Reposts" },
        Route::PostQuotes { .. } => TitleSubject::Engagement { label: "Quotes" },
    };
    let title = tab_title(subject, APP_NAME);
    document::eval(&format!(
        "document.title = {};",
        serde_json::to_string(&title).unwrap()
    ));
}

/// Label for a feed, resolved in exactly one place.
///
/// The whats-hot generator is the awkward case: its `displayName` is "Discover",
/// and Bluesky ranks it against your network when signed in — but anonymous traffic
/// gets global trending, which is honestly "What's Hot". So: the account's saved
/// name wins, then "Discover" for a signed-in viewer, then the preset label.
fn label_for(source: &FeedSource, saved: &[(String, String)], signed_in: bool) -> String {
    if let FeedSource::Custom { uri, label } = source {
        let saved_label = saved
            .iter()
            .find(|(saved_uri, _)| saved_uri == uri)
            .map(|(_, saved_label)| saved_label.clone())
            .filter(|saved_label| !saved_label.trim().is_empty());
        if uri == DISCOVER_FEED {
            return discover_label(saved_label.as_deref(), signed_in);
        }
        return saved_label.unwrap_or_else(|| label.clone());
    }
    source.label()
}

/// The feed a route points at, if any.
fn source_for(route: &Route, saved: &[(String, String)]) -> Option<FeedSource> {
    match route {
        // `#/` is the Explore page (search + trending), not a feed: the Discover
        // generator keeps its own `#/feed/<uri>` route, reachable from Feeds.
        Route::Home => None,
        Route::Following => Some(FeedSource::Following),
        Route::Feed { uri } => Some(FeedSource::Custom {
            uri: uri.clone(),
            label: label_for_uri(uri, saved),
        }),
        Route::Profile { .. }
        | Route::Post { .. }
        | Route::PostLikes { .. }
        | Route::PostReposts { .. }
        | Route::PostQuotes { .. }
        | Route::Feeds
        | Route::Settings
        | Route::SignIn
        | Route::Notifications
        | Route::Compose
        | Route::Hashtag { .. } => None,
    }
}

#[derive(Clone, PartialEq, Eq)]
enum FeedSource {
    /// The built-in timeline; needs a session and is not a generator.
    Following,
    /// Deep-linked author feed (`?actor=`), until profiles exist.
    Author(String),
    /// Any generator: public presets and the account's saved feeds alike.
    Custom { uri: String, label: String },
}

/// Pages already fetched, so revisiting a feed does not throw away the reader's
/// progress. Bounded: a long session should not pin every feed it ever opened.
#[derive(Clone, Default)]
struct FeedCache {
    entries: std::collections::HashMap<String, (Vec<(PostView, Option<String>)>, Option<String>)>,
    order: std::collections::VecDeque<String>,
}

/// Background check for newer posts.
///
/// Deliberately does **not** run through the feed effect: re-running that effect
/// bumps the generation counter, and a first-page fetch still in flight would then
/// be discarded as stale — which is exactly how the feed once came up empty on boot,
/// showing "Loading public feed…" forever.
async fn revalidate_feed(
    source: FeedSource,
    session: Option<Session>,
    state: SessionState,
    known: std::collections::HashSet<String>,
    generation: u64,
    feed_generation: Signal<u64>,
    mut feed_pending: Signal<Option<PendingRefresh>>,
) {
    let Ok(response) = fetch_feed(source, session, None, state).await else {
        // A failed background check is not worth interrupting reading.
        return;
    };
    if *feed_generation.peek() != generation {
        return;
    }
    let fresh = feed_items_of(&response);
    let fresh_uris: Vec<String> = fresh.iter().map(|(post, _)| post.uri.clone()).collect();
    if new_post_count(&fresh_uris, &known) == 0 {
        return;
    }
    feed_pending.set(Some(PendingRefresh {
        items: fresh,
        cursor: response.cursor,
        count: new_post_count(&fresh_uris, &known),
    }));
}

/// A fresher first page held back until the reader asks for it.
///
/// Swapping a feed under someone mid-scroll loses their place, so newer posts are
/// announced rather than injected.
#[derive(Clone)]
struct PendingRefresh {
    items: Vec<(PostView, Option<String>)>,
    cursor: Option<String>,
    count: usize,
}

impl FeedCache {
    const KEEP: usize = 6;

    fn get(&self, key: &str) -> Option<(Vec<(PostView, Option<String>)>, Option<String>)> {
        self.entries.get(key).cloned()
    }

    fn insert(
        &mut self,
        key: String,
        items: Vec<(PostView, Option<String>)>,
        cursor: Option<String>,
    ) {
        if !self.entries.contains_key(&key) {
            self.order.push_back(key.clone());
            while self.order.len() > Self::KEEP {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
        }
        self.entries.insert(key, (items, cursor));
    }
}
impl FeedSource {
    /// Label shown on the tab and the "reading" note.
    fn label(&self) -> String {
        match self {
            Self::Following => "Following".to_string(),
            Self::Author(handle) => format!("@{handle}"),
            Self::Custom { label, .. } => label.clone(),
        }
    }

    /// Stable identity for "is this tab active" without comparing labels.
    fn key(&self) -> String {
        match self {
            Self::Following => "following".to_string(),
            Self::Author(handle) => format!("author:{handle}"),
            Self::Custom { uri, .. } => format!("custom:{uri}"),
        }
    }

    fn needs_session(&self) -> bool {
        matches!(self, Self::Following)
    }
}

async fn fetch_feed(
    source: FeedSource,
    session: Option<Session>,
    cursor: Option<&str>,
    state: SessionState,
) -> Result<FeedResponse, String> {
    let cursor_param = cursor
        .map(|cursor| format!("&cursor={}", encode_uri(cursor)))
        .unwrap_or_default();

    match source {
        // Author feeds stay single-page for now; the profile view is the only
        // caller and it already shows a full first page.
        FeedSource::Author(handle) => fetch_author_feed(&handle, state).await,
        FeedSource::Custom { uri, .. } => {
            // Personalized generators (Discover above all) need the viewer context
            // that only the PDS can attach, so prefer it when signed in.
            let (base, auth_session) = match &session {
                Some(session) => (session.pds.clone(), Some(session.clone())),
                None => (PUBLIC_API.to_string(), None),
            };
            request_json(
                "GET",
                &format!(
                    "{base}/xrpc/app.bsky.feed.getFeed?feed={}&limit=30{cursor_param}",
                    encode_uri(&uri)
                ),
                auth_session.as_ref().and_then(Session::auth),
                None,
            )
            .await
        }
        FeedSource::Following => {
            let Some(session) = session else {
                return Err("Sign in to load Following".into());
            };
            request_json(
                "GET",
                &format!(
                    "{}/xrpc/app.bsky.feed.getTimeline?limit=30{cursor_param}",
                    session.pds
                ),
                session.auth(),
                None,
            )
            .await
        }
    }
}

/// Feed items with the optional "Reposted by …" attribution label.
fn feed_items_of(response: &FeedResponse) -> Vec<(PostView, Option<String>)> {
    response
        .feed
        .iter()
        .map(|item| {
            let label = match &item.reason {
                Some(FeedViewReason::Repost { by, .. }) => {
                    let name = by
                        .display_name
                        .clone()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| format!("@{}", by.handle));
                    Some(format!("Reposted by {name}"))
                }
                Some(FeedViewReason::Pin) => Some("Pinned post".to_string()),
                _ => None,
            };
            (item.post.clone(), label)
        })
        .collect()
}

/// Tabs to show. Presets are skipped when the account already saved the same
/// generator, and Discover is labelled for what it actually is when signed out.
fn feed_tabs(saved: &[(String, String)], signed_in: bool) -> Vec<FeedSource> {
    let mut tabs = Vec::new();
    let already_saved = |uri: &str| saved.iter().any(|(saved_uri, _)| saved_uri == uri);

    // Same resolver as the title: a tab that says "What's Hot" while the title says
    // "Discover" is exactly the disagreement this app kept having.
    if !already_saved(DISCOVER_FEED) {
        tabs.push(FeedSource::Custom {
            uri: DISCOVER_FEED.to_string(),
            label: discover_label(None, signed_in),
        });
    }
    tabs.push(FeedSource::Following);
    tabs.extend(
        saved
            .iter()
            .map(|(uri, label)| FeedSource::Custom {
                uri: uri.clone(),
                label: label.clone(),
            }),
    );
    tabs
}

/// Serialized form for "remember me". This is the one place tokens touch disk,
/// and only when the reader explicitly opts in — localStorage is readable by any
/// script on the origin, so the default is memory-only.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    handle: String,
    did: String,
    access_jwt: String,
    refresh_jwt: String,
    pds: String,
    /// Absent for app-password sessions, and absent from JSON written by older versions —
    /// which is exactly why it is optional rather than a required field.
    #[serde(default)]
    oauth: Option<oauth_flow::OAuthSession>,
}

impl From<&Session> for StoredSession {
    fn from(session: &Session) -> Self {
        Self {
            handle: session.handle.clone(),
            did: session.did.clone(),
            access_jwt: session.access_jwt.clone(),
            refresh_jwt: session.refresh_jwt.clone(),
            pds: session.pds.clone(),
            oauth: session.oauth.clone(),
        }
    }
}

impl From<StoredSession> for Session {
    fn from(stored: StoredSession) -> Self {
        Self {
            handle: stored.handle,
            did: stored.did,
            access_jwt: stored.access_jwt,
            refresh_jwt: stored.refresh_jwt,
            pds: stored.pds,
            oauth: stored.oauth,
            // It came out of storage, so it belongs there until something says otherwise.
            remembered: true,
        }
    }
}

const SESSION_KEY: &str = "altfirst.session";
const LAST_FEED_KEY: &str = "altfirst.lastFeed";
const THEME_KEY: &str = "altfirst.theme";

/// Applies a palette to `<html>` and remembers the choice.
///
/// The attribute is what the CSS keys off; `system` removes it so the media query takes
/// over again. Storage and the DOM are written in the same eval, so a change cannot
/// leave the two disagreeing and survive a reload in the wrong theme.
fn apply_theme(theme: Theme) {
    let attribute = match theme {
        Theme::System => "",
        other => other.as_str(),
    };
    document::eval(&format!(
        r#"(() => {{
            try {{ localStorage.setItem({key}, {stored}); }} catch (error) {{}}
            const value = {attribute};
            if (value) document.documentElement.dataset.theme = value;
            else delete document.documentElement.dataset.theme;
        }})()"#,
        key = serde_json::to_string(THEME_KEY).unwrap(),
        stored = serde_json::to_string(theme.as_str()).unwrap(),
        attribute = serde_json::to_string(attribute).unwrap(),
    ));
}

async fn stored_theme() -> Theme {
    let mut eval = document::eval(&format!(
        "dioxus.send(localStorage.getItem({}) ?? 'system')",
        serde_json::to_string(THEME_KEY).unwrap()
    ));
    eval.recv::<String>()
        .await
        .map(|raw| Theme::from_str(&raw))
        .unwrap_or(Theme::System)
}

/// Remembered tab choice. The hash already survives a refresh on its own; this
/// makes the *home* route reopen the last feed you were reading instead of
/// snapping back to What's Hot.
fn remember_feed(hash: &str) {
    document::eval(&format!(
        "localStorage.setItem({key}, {value});",
        key = serde_json::to_string(LAST_FEED_KEY).unwrap(),
        value = serde_json::to_string(hash).unwrap()
    ));
}

/// Which route to open when the app is loaded cold, if any.
///
/// Deliberately ignores an explicit hash: if the URL says where to go, that wins.
/// Remembering a feed is only for a bare URL — and a session-only feed is never a
/// valid target when nothing is signed in, which is how a stale `#/following` used
/// to strand the whole app on "Sign in to load Following".
async fn restore_target() -> Option<String> {
    let mut eval = document::eval(&format!(
        r#"(() => {{
            const hash = location.hash;
            if (hash && hash !== '#') return dioxus.send('');
            const last = localStorage.getItem({last_key}) ?? '';
            if (!last || last === '#/') return dioxus.send('');
            if (last === '#/following' && !localStorage.getItem({session_key})) {{
                localStorage.removeItem({last_key});
                return dioxus.send('');
            }}
            dioxus.send(last);
        }})()"#,
        last_key = serde_json::to_string(LAST_FEED_KEY).unwrap(),
        session_key = serde_json::to_string(SESSION_KEY).unwrap(),
    ));
    eval.recv::<String>().await.ok().filter(|value| !value.is_empty())
}

fn persist_session(session: Option<&Session>, remember: bool) {
    let script = match (session, remember) {
        (Some(session), true) => {
            let json = serde_json::to_string(&StoredSession::from(session)).unwrap_or_default();
            format!(
                "localStorage.setItem({key}, {value});",
                key = serde_json::to_string(SESSION_KEY).unwrap(),
                value = serde_json::to_string(&json).unwrap()
            )
        }
        _ => format!(
            "localStorage.removeItem({});",
            serde_json::to_string(SESSION_KEY).unwrap()
        ),
    };
    document::eval(&script);
}

async fn load_stored_session() -> Option<Session> {
    let mut eval = document::eval(&format!(
        "dioxus.send(localStorage.getItem({}) ?? '')",
        serde_json::to_string(SESSION_KEY).unwrap()
    ));
    let raw = eval.recv::<String>().await.ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<StoredSession>(&raw)
        .ok()
        .map(Session::from)
}

/// ISO-8601 timestamp for a like record; the browser already has a clock.
async fn now_iso() -> String {
    let mut eval = document::eval("dioxus.send(new Date().toISOString())");
    eval.recv::<String>().await.unwrap_or_default()
}

/// Create the viewer's like or repost record and return its at-uri.
///
/// The uri is the whole point: undoing the reaction is deleting exactly that
/// record, so the toggle never has to look it up again.
async fn create_reaction(
    session: &Session,
    reaction: Reaction,
    post_uri: &str,
    cid: &str,
) -> Result<String, String> {
    let body = serde_json::json!({
        "repo": session.did,
        "collection": reaction.collection(),
        "record": {
            "$type": reaction.collection(),
            "subject": { "uri": post_uri, "cid": cid },
            "createdAt": now_iso().await,
        }
    });
    let response: serde_json::Value = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.createRecord", session.pds),
        session.auth(),
        Some(&body.to_string()),
    )
    .await?;
    response["uri"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{} created but no uri returned", reaction.noun()))
}

async fn delete_reaction(
    session: &Session,
    reaction: Reaction,
    record_uri: &str,
) -> Result<(), String> {
    let rkey = record_uri.rsplit('/').next().unwrap_or_default().to_string();
    let body = serde_json::json!({
        "repo": session.did,
        "collection": reaction.collection(),
        "rkey": rkey,
    });
    let _: serde_json::Value = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.deleteRecord", session.pds),
        session.auth(),
        Some(&body.to_string()),
    )
    .await?;
    Ok(())
}

/// Delete one of your own posts.
///
/// `com.atproto.repo.deleteRecord` identifies the record by collection and key, so the at-uri
/// has to be taken apart — and a post that is not yours has no rkey in your repository, which
/// is why the caller only offers this on your own posts.
async fn delete_post(session: &Session, uri: &str) -> Result<(), String> {
    let Some(rkey) = alt_first_core::record_key(uri) else {
        return Err("that post's address is not a record uri".to_string());
    };
    let body = serde_json::json!({
        "repo": session.did,
        "collection": "app.bsky.feed.post",
        "rkey": rkey,
    })
    .to_string();
    let _: serde_json::Value = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.deleteRecord", session.pds),
        session.auth(),
        Some(&body),
    )
    .await?;
    Ok(())
}

/// Post a reply and return its at-uri.
///
/// `reply` carries strong refs to the thread root and to the post being answered —
/// both, because the AppView places the record in the conversation from those refs
/// alone.
async fn create_reply(
    session: &Session,
    text: &str,
    refs: &ReplyRef,
) -> Result<String, String> {
    let mut record = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": text,
        "createdAt": now_iso().await,
        "reply": {
            "root": { "uri": refs.root.uri, "cid": refs.root.cid },
            "parent": { "uri": refs.parent.uri, "cid": refs.parent.cid },
        },
    });
    // Replies carry facets too: a link in a reply was as inert as a link in a post.
    attach_facets(&mut record, text).await;
    let body = serde_json::json!({
        "repo": session.did,
        "collection": "app.bsky.feed.post",
        "record": record,
    });
    let response: serde_json::Value = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.createRecord", session.pds),
        session.auth(),
        Some(&body.to_string()),
    )
    .await?;
    response["uri"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "reply created but no uri returned".to_string())
}

/// Handle → DID, which is what a mention facet has to store.
async fn resolve_handle(handle: &str) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Resolved {
        did: String,
    }
    let resolved: Resolved = request_json(
        "GET",
        &format!(
            "{PUBLIC_API}/xrpc/com.atproto.identity.resolveHandle?handle={}",
            encode_uri(handle)
        ),
        None,
        None,
    )
    .await?;
    Ok(resolved.did)
}

/// Annotate a record's text with the links and mentions it contains.
///
/// Without this a URL is stored as plain text and nothing can click it — which is how a client
/// ends up unable to share its own address — and an `@handle` is just characters.
///
/// A mention only becomes a facet when the handle **resolves**: the record has to name a DID,
/// and a facet pointing at an invented one is worse than plain text. A failed lookup therefore
/// leaves that mention as typed, which is the honest fallback.
async fn attach_facets(record: &mut serde_json::Value, text: &str) {
    let mut facets = alt_first_core::link_facets(text);
    for candidate in alt_first_core::mention_candidates(text) {
        if let Ok(did) = resolve_handle(&candidate.handle).await {
            facets.push(Facet {
                index: ByteRange {
                    byte_start: candidate.byte_start,
                    byte_end: candidate.byte_end,
                },
                features: vec![FacetFeature::Mention { did }],
            });
        }
    }
    for candidate in alt_first_core::tag_candidates(text) {
        facets.push(Facet {
            index: ByteRange {
                byte_start: candidate.byte_start,
                byte_end: candidate.byte_end,
            },
            features: vec![FacetFeature::Tag {
                tag: candidate.tag,
            }],
        });
    }
    // Mentions, links and tags are found separately; a post can contain all three in any order.
    facets.sort_by_key(|facet| facet.index.byte_start);
    if !facets.is_empty() {
        if let Ok(value) = serde_json::to_value(&facets) {
            record["facets"] = value;
        }
    }
}

/// A top-level post: the reply record without the `reply` refs.
///
/// Same call, same record type, same error surface — the only thing that made writing
/// impossible was that the one existing writer always attached a parent.
async fn create_post(session: &Session, text: &str) -> Result<String, String> {
    let mut record = serde_json::json!({
        "$type": "app.bsky.feed.post",
        "text": text,
        "createdAt": now_iso().await,
    });
    attach_facets(&mut record, text).await;
    let body = serde_json::json!({
        "repo": session.did,
        "collection": "app.bsky.feed.post",
        "record": record,
    });
    let response: serde_json::Value = request_json(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.createRecord", session.pds),
        session.auth(),
        Some(&body.to_string()),
    )
    .await?;
    response["uri"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "post created but no uri returned".to_string())
}

/// Mute or unmute an account.
///
/// Muting is server-side state, not a public record: there is nothing to create and
/// nothing to delete, and nobody can see that it happened.
async fn set_muted(session: &Session, did: &str, muted: bool) -> Result<(), String> {
    let method = if muted {
        "app.bsky.graph.muteActor"
    } else {
        "app.bsky.graph.unmuteActor"
    };
    request_ok(
        "POST",
        &format!("{}/xrpc/{}", session.pds, method),
        session.auth(),
        Some(&serde_json::json!({ "actor": did }).to_string()),
    )
    .await
}

/// Block or unblock an account, returning the block record's uri (None when unblocked).
///
/// A block *is* a public record, so undoing it means deleting that exact record —
/// which is why `existing` (from `viewer.blocking`) is required to unblock. Blocks are
/// bidirectional: the account can no longer see or interact with this one either.
async fn set_blocked(
    session: &Session,
    did: &str,
    existing: Option<&str>,
    block: bool,
) -> Result<Option<String>, String> {
    if block {
        let body = serde_json::json!({
            "repo": session.did,
            "collection": "app.bsky.graph.block",
            "record": {
                "$type": "app.bsky.graph.block",
                "subject": did,
                "createdAt": now_iso().await,
            }
        });
        let response: serde_json::Value = request_json(
            "POST",
            &format!("{}/xrpc/com.atproto.repo.createRecord", session.pds),
            session.auth(),
            Some(&body.to_string()),
        )
        .await?;
        return response["uri"]
            .as_str()
            .map(|uri| Some(uri.to_string()))
            .ok_or_else(|| "block created but no uri returned".to_string());
    }

    let Some(record_uri) = existing else {
        // Nothing to undo: the AppView already considers this account unblocked.
        return Ok(None);
    };
    let rkey = record_uri.rsplit('/').next().unwrap_or_default().to_string();
    request_ok(
        "POST",
        &format!("{}/xrpc/com.atproto.repo.deleteRecord", session.pds),
        session.auth(),
        Some(
            &serde_json::json!({
                "repo": session.did,
                "collection": "app.bsky.graph.block",
                "rkey": rkey,
            })
            .to_string(),
        ),
    )
    .await?;
    Ok(None)
}
/// Run a block or unblock and fold the result into the profile's optimistic state.
///
/// Shared by the action row and the confirmation panel so the two cannot drift: same
/// call, same error line, same busy flag.
fn apply_block(
    session: Session,
    did: String,
    existing: Option<String>,
    next: bool,
    mut blocked: Signal<Option<bool>>,
    mut block_uri: Signal<Option<String>>,
    mut busy: Signal<bool>,
    mut error: Signal<Option<String>>,
) {
    busy.set(true);
    error.set(None);
    spawn(async move {
        match set_blocked(&session, &did, existing.as_deref(), next).await {
            Ok(uri) => {
                blocked.set(Some(next));
                block_uri.set(uri);
            }
            Err(err) => error.set(Some(err)),
        }
        busy.set(false);
    });
}




async fn fetch_saved_feeds(state: SessionState) -> Result<Vec<(String, String)>, String> {
    // Rotating here matters as much as on a profile: this is the first authenticated call the
    // restore path makes, so a stale access token used to quietly empty the Feeds page.
    let prefs: PreferencesResponse =
        authed_get("/xrpc/app.bsky.actor.getPreferences", state).await?;

    let uris = prefs.saved_feed_uris();
    if uris.is_empty() {
        return Ok(Vec::new());
    }

    let query = uris
        .iter()
        .map(|uri| format!("feeds={}", encode_uri(uri)))
        .collect::<Vec<_>>()
        .join("&");
    let generators: FeedGeneratorsResponse = fetch_json(&format!(
        "{PUBLIC_API}/xrpc/app.bsky.feed.getFeedGenerators?{query}"
    ))
    .await?;

    Ok(uris
        .into_iter()
        .map(|uri| {
            let label = generators
                .feeds
                .iter()
                .find(|feed| feed.uri == uri)
                .and_then(|feed| feed.display_name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| {
                    uri.rsplit('/')
                        .next()
                        .unwrap_or("feed")
                        .chars()
                        .take(14)
                        .collect()
                });
            (uri, label)
        })
        .collect())
}

/// GET that carries the viewer, with a safe degradation path.
///
/// Signed in, the request goes to the PDS with the access token, because that is
/// the service that fills `viewer.like` (the public AppView answers `null` for it,
/// which makes an already-liked post look unliked — and tapping the heart there
/// creates a second like record). An expired token is rotated *first*, so the fallback
/// below is for a read that genuinely cannot be authenticated rather than one that merely
/// aged out. If it still fails, the read falls back to the public AppView: losing the
/// hearts is a small price, turning a thread into an error panel because of viewer plumbing
/// is not.
async fn viewer_read<T: serde::de::DeserializeOwned>(
    path: &str,
    state: SessionState,
) -> Result<T, String> {
    if (state.0)().is_none() {
        return request_json("GET", &format!("{PUBLIC_API}{path}"), None, None).await;
    }
    match authed_get::<T>(path, state).await {
        Ok(value) => Ok(value),
        Err(original) => request_json("GET", &format!("{PUBLIC_API}{path}"), None, None)
            .await
            .map_err(|_| original),
    }
}

async fn send_reaction(
    session: &Session,
    reaction: Reaction,
    target: &str,
    cid: &str,
    previous_uri: Option<String>,
    next: bool,
) -> Result<Option<String>, String> {
    if next {
        create_reaction(session, reaction, target, cid).await.map(Some)
    } else {
        match previous_uri {
            Some(record) => delete_reaction(session, reaction, &record).await.map(|_| None),
            // The reaction never got a record back, so there is nothing to delete.
            None => Ok(None),
        }
    }
}

/// Flip a like or a repost: optimistic write, one token rotation on 401, and a
/// visible revert with a reason on failure. Taps are ignored while a write for the
/// same (reaction, post) is in flight, so two fast taps cannot race one record.
fn toggle_reaction(
    mut ctx: ReactionCtx,
    mut session_state: SessionState,
    reaction: Reaction,
    target: String,
    cid: String,
    view: ReactionView,
) {
    let key = (reaction, target.clone());
    if ctx.pending.read().contains(&key) {
        return;
    }
    let Some(mut signed) = (session_state.0)() else {
        navigate("#/sign-in");
        return;
    };
    let next = !view.on;
    let previous = view.record.clone();
    let optimistic = ReactionView {
        on: next,
        // Placeholder until the PDS answers with the real record uri.
        record: if next { previous.clone() } else { None },
        count: (view.count + if next { 1 } else { -1 }).max(0),
    };
    ctx.state.write().insert(key.clone(), optimistic.clone());
    ctx.pending.write().insert(key.clone());
    ctx.errors.write().remove(&key);

    spawn(async move {
        let mut result =
            send_reaction(&signed, reaction, &target, &cid, previous.clone(), next).await;
        // Access tokens expire in ~2 h; rotate and retry once, like feed loads do. The
        // rotation must be saved — see the feed path for why.
        if let Err(error) = &result {
            if is_expired_token(error) {
                // Same one rotation path as feeds and reads — including its cross-tab handling,
                // so a like in two tabs cannot sign the loser out.
                match rotate_session(session_state, &signed).await {
                    Ok(fresh) => {
                        signed = fresh;
                        result = send_reaction(&signed, reaction, &target, &cid, previous.clone(), next)
                            .await;
                    }
                    Err(refresh_error) => {
                        log_to_console(&format!("refresh failed: {refresh_error}"));
                        if is_dead_grant(&refresh_error) {
                            forget_session(session_state, &refresh_error);
                            ctx.pending.write().remove(&key);
                            return;
                        }
                    }
                }
            }
        }
        ctx.pending.write().remove(&key);
        match result {
            Ok(record) => {
                ctx.state.write().insert(
                    key.clone(),
                    ReactionView {
                        on: next,
                        record,
                        count: optimistic.count,
                    },
                );
                ctx.errors.write().remove(&key);
            }
            Err(error) => {
                // Put the card back exactly as it was, and say why.
                ctx.state.write().insert(key.clone(), view);
                ctx.errors
                    .write()
                    .insert(key, reaction_note(&error, reaction, next));
            }
        }
    });
}

/// A short reason for a failed reaction, sized for the space under a card.
///
/// `request_json` errors read `HTTP 400 from https://…: {"error":…}`, which is
/// noise under a post; classify it and keep the server's own words only when they
/// are already short.
fn reaction_note(error: &str, reaction: Reaction, next: bool) -> String {
    let verb = if next { "save" } else { "remove" };
    let noun = reaction.noun();
    let failure = FeedFailure::parse(error);
    match failure.kind {
        FeedFailureKind::Unreachable => "Couldn't reach Bluesky.".to_string(),
        FeedFailureKind::Refused => "Not authorized — sign in again.".to_string(),
        FeedFailureKind::RateLimited => format!("Bluesky is throttling {noun}s right now."),
        FeedFailureKind::ServerError => "Bluesky's side failed — try again.".to_string(),
        _ => {
            let detail: String = failure.detail.chars().take(120).collect();
            if detail.trim().is_empty() {
                format!("Couldn't {verb} that {noun}.")
            } else {
                format!("Couldn't {verb} that {noun}: {}", detail.trim())
            }
        }
    }
}

// `public.api.bsky.app` is not usable from a browser: it first answered 403 for
// `searchPosts`, and as of this writing it answers 403 (`Request forbidden by administrative
// rules`) for every endpoint, with or without an Origin header. `api.bsky.app` answers all of
// them with `access-control-allow-origin: *`, so that is the only AppView host here.

async fn search_posts(query: &str) -> Result<SearchPostsResponse, String> {
    request_json(
        "GET",
        &format!(
            "{PUBLIC_API}/xrpc/app.bsky.feed.searchPosts?q={}&limit=25",
            encode_uri(query)
        ),
        None,
        None,
    )
    .await
}

async fn fetch_trending() -> Result<TrendingTopicsResponse, String> {
    request_json(
        "GET",
        &format!("{PUBLIC_API}/xrpc/app.bsky.unspecced.getTrendingTopics?limit=8"),
        None,
        None,
    )
    .await
}

async fn fetch_profile(
    actor: &str,
    state: SessionState,
) -> Result<ProfileViewDetailed, String> {
    viewer_read(
        &format!(
            "/xrpc/app.bsky.actor.getProfile?actor={}",
            encode_uri(actor)
        ),
        state,
    )
    .await
}

/// The same profile, read publicly on purpose.
///
/// The account menu wants one field (the avatar URL) and needs no viewer state, so this skips
/// the token, the DPoP proof and the 401 that an authenticated read on Explore used to put in
/// the console for a URL the public AppView hands out anyway.
async fn fetch_public_profile(actor: &str) -> Result<ProfileViewDetailed, String> {
    request_json(
        "GET",
        &format!(
            "{PUBLIC_API}/xrpc/app.bsky.actor.getProfile?actor={}",
            encode_uri(actor)
        ),
        None,
        None,
    )
    .await
}

/// Read any public account's timeline, with the viewer's own like/repost state
/// when a session is present (see `fetch_thread` for why that matters).
///
/// Deliberately no `filter` param: the AppView offers `posts_with_media`, but it
/// hides text-only posts — and on Bluesky those are where mentions and links
/// (facets) concentrate. Media-richness is a property of the data, not a query
/// the client should impose.
async fn fetch_author_feed(
    actor: &str,
    state: SessionState,
) -> Result<FeedResponse, String> {
    viewer_read(
        &format!("/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit=30", encode_uri(actor)),
        state,
    )
    .await
}

/// Thread read: `parentHeight=2` gives the ancestors, `depth=4` the replies.
///
/// The session is not optional decoration: without it the AppView fills
/// `viewer.like` with null, so a post you already liked renders unliked here and
/// tapping the heart creates a *second* like record instead of removing the first.
async fn fetch_thread(uri: &str, state: SessionState) -> Result<PostThreadResponse, String> {
    let encoded = uri.replace(':', "%3A").replace('/', "%2F").replace('#', "%23");
    viewer_read(
        &format!("/xrpc/app.bsky.feed.getPostThread?uri={encoded}&depth=4&parentHeight=2"),
        state,
    )
    .await
}

/// Inject the stylesheet once (no asset pipeline dependency, works everywhere).
fn inject_css() {
    let css = serde_json::to_string(CSS).unwrap();
    document::eval(&format!(
        r#"if (!document.getElementById('alt-first-style')) {{
            const style = document.createElement('style');
            style.id = 'alt-first-style';
            style.textContent = {css};
            document.head.appendChild(style);
        }}"#
    ));
}

/// HLS: `playlist` is `.m3u8`. Android WebView plays it natively; desktop Chrome
/// needs hls.js. Load hls.js lazily and attach to any video[data-hls] added later.
fn install_hls_shim() {
    document::eval(
        r#"(async () => {
            if (window.__altFirstHls) return;
            window.__altFirstHls = true;

            if (!window.Hls) {
                await new Promise((resolve) => {
                    const script = document.createElement('script');
                    script.src = 'https://cdn.jsdelivr.net/npm/hls.js@1';
                    script.onload = resolve;
                    script.onerror = resolve;
                    document.head.appendChild(script);
                });
            }

            const attach = (video) => {
                const src = video.dataset.hls;
                if (!src || video.dataset.hlsAttached) return;
                video.dataset.hlsAttached = '1';
                if (video.canPlayType('application/vnd.apple.mpegurl')) {
                    video.src = src;
                    video.play().catch(() => {});
                    return;
                }
                if (window.Hls && window.Hls.isSupported()) {
                    const hls = new window.Hls();
                    hls.loadSource(src);
                    hls.attachMedia(video);
                    hls.on(window.Hls.Events.MANIFEST_PARSED, () => video.play().catch(() => {}));
                }
            };

            window.__altFirstAttachHls = attach;
            new MutationObserver((mutations) => {
                for (const mutation of mutations) {
                    for (const node of mutation.addedNodes) {
                        if (node.nodeType !== 1) continue;
                        if (node.matches && node.matches('video[data-hls]')) attach(node);
                        if (node.querySelectorAll) {
                            node.querySelectorAll('video[data-hls]').forEach(attach);
                        }
                    }
                }
            }).observe(document.documentElement, { childList: true, subtree: true });
        })()"#,
    );
}

/// Image embed: alt box first, image on demand, close returns to the box.
#[component]
fn AltFirstImage(image: ImageView) -> Element {
    let preference = use_context::<AltTextFirst>();
    let mut state = use_signal(|| {
        if (preference.0)() {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        }
    });
    let mut touched = use_signal(|| false);
    let mut show_alt = use_signal(|| (preference.0)());
    use_effect(move || {
        let enabled = (preference.0)();
        // The preference owns caption visibility: alt-first shows the description,
        // media-first hides it behind the ALT pill.
        show_alt.set(enabled);
        if touched() {
            return;
        }
        state.set(if enabled {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        });
    });

    let ratio = placeholder_ratio(image.aspect_ratio.as_ref());
    let described = alt_policy(&image.alt).is_described();
    // Alt-first mode puts the description on screen already, so no pill is needed;
    // in media-first mode the caption starts hidden and the pill reveals it.
    let text_class = if described {
        "alt-first__text"
    } else {
        "alt-first__text alt-first__text--missing"
    };
    let alt_gate = image.alt.clone();
    let alt_caption = image.alt.clone();
    let fullsize = image.fullsize.clone();

    rsx! {
        figure { class: "alt-first",
            {match state() {
                MediaState::Collapsed => rsx! {
                    button {
                        r#type: "button",
                        class: "alt-first__gate",
                        "aria-expanded": "false",
                        onclick: move |_| {
                            touched.set(true);
                            state.set(MediaState::Thumbnail);
                        },
                        span { class: "alt-first__kicker", "Image" }
                        span { class: text_class,
                            {if described { alt_gate.clone() } else { "No Alt Text".to_string() }}
                        }
                        span { class: "alt-first__action", "Show image" }
                    }
                },
                _ => rsx! {
                    div { class: "alt-first__media-wrap", style: "--media-ratio: {ratio};",
                        a {
                            class: "alt-first__media-link",
                            href: "{fullsize}",
                            target: "_blank",
                            rel: "noreferrer",
                            img {
                                class: "alt-first__media",
                                src: "{fullsize}",
                                alt: "",
                                loading: "lazy",
                                decoding: "async",
                            }
                        }
                        if described && !show_alt() {
                            button {
                                r#type: "button",
                                class: "alt-first__alt-pill",
                                "aria-pressed": "false",
                                onclick: move |_| show_alt.set(true),
                                "ALT"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "alt-first__close",
                            "aria-label": "Hide image",
                            onclick: move |_| {
                                touched.set(true);
                                state.set(MediaState::Collapsed);
                            },
                            "×"
                        }
                    }
                },
            }}
            if state() != MediaState::Collapsed && described && show_alt() {
                figcaption { class: "alt-first__caption", "{alt_caption}" }
            }
        }
    }
}


/// Video embed: alt box -> poster + play -> playing, close at every step.
#[component]
fn AltFirstVideo(video: VideoView) -> Element {
    let preference = use_context::<AltTextFirst>();
    let mut state = use_signal(|| {
        if (preference.0)() {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        }
    });
    let mut touched = use_signal(|| false);
    let mut show_alt = use_signal(|| (preference.0)());
    use_effect(move || {
        let enabled = (preference.0)();
        // The preference owns caption visibility: alt-first shows the description,
        // media-first hides it behind the ALT pill.
        show_alt.set(enabled);
        if touched() {
            return;
        }
        state.set(if enabled {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        });
    });

    let ratio = placeholder_ratio(video.aspect_ratio.as_ref());
    let described = video_alt_policy(video.alt.as_deref()).is_described();
    let is_gif = video.presentation.as_deref() == Some("gif");
    let alt_gate = video.alt.clone().unwrap_or_default();
    let alt_caption = alt_gate.clone();
    let poster = video.thumbnail.clone().unwrap_or_default();
    let playlist = video.playlist.clone();
    let kicker = if is_gif { "GIF" } else { "Video" };
    let text_class = if described {
        "alt-first__text"
    } else {
        "alt-first__text alt-first__text--missing"
    };

    rsx! {
        figure { class: "alt-first alt-first--video",
            {match state() {
                MediaState::Collapsed => rsx! {
                    button {
                        r#type: "button",
                        class: "alt-first__gate",
                        "aria-expanded": "false",
                        onclick: move |_| {
                            touched.set(true);
                            state.set(MediaState::Thumbnail);
                        },
                        span { class: "alt-first__kicker", "{kicker}" }
                        span { class: text_class,
                            {if described {
                                alt_gate.clone()
                            } else {
                                format!("No Alt Text ({kicker})")
                            }}
                        }
                        span { class: "alt-first__action",
                            {if is_gif { "Show animation" } else { "Show video" }}
                        }
                    }
                },
                MediaState::Thumbnail => rsx! {
                    div { class: "alt-first__media-wrap", style: "--media-ratio: {ratio};",
                        img { class: "alt-first__media", src: "{poster}", alt: "", loading: "lazy" }
                        button {
                            r#type: "button",
                            class: "alt-first__play",
                            "aria-label": "Play video",
                            onclick: move |_| {
                                touched.set(true);
                                state.set(MediaState::Playing);
                            },
                            "▶"
                        }
                        if described && !show_alt() {
                            button {
                                r#type: "button",
                                class: "alt-first__alt-pill",
                                "aria-pressed": "false",
                                onclick: move |_| show_alt.set(true),
                                "ALT"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "alt-first__close",
                            "aria-label": "Hide video",
                            onclick: move |_| {
                                touched.set(true);
                                state.set(MediaState::Collapsed);
                            },
                            "×"
                        }
                    }
                },
                MediaState::Playing => rsx! {
                    div { class: "alt-first__media-wrap", style: "--media-ratio: {ratio};",
                        video {
                            class: "alt-first__media",
                            controls: true,
                            playsinline: true,
                            preload: "none",
                            autoplay: is_gif,
                            muted: is_gif,
                            loop: is_gif,
                            poster: "{poster}",
                            "data-hls": "{playlist}",
                        }
                        if described && !show_alt() {
                            button {
                                r#type: "button",
                                class: "alt-first__alt-pill",
                                "aria-pressed": "false",
                                onclick: move |_| show_alt.set(true),
                                "ALT"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "alt-first__close",
                            "aria-label": "Hide video",
                            onclick: move |_| {
                                touched.set(true);
                                state.set(MediaState::Collapsed);
                            },
                            "×"
                        }
                    }
                },
            }}
            if state() != MediaState::Collapsed && described && show_alt() {
                figcaption { class: "alt-first__caption", "{alt_caption}" }
            }
        }
    }
}


/// Link embed: no alt text exists in the protocol, so only the thumbnail is gated.
#[component]
fn AltFirstLinkThumbnail(link: ExternalView) -> Element {
    let preference = use_context::<AltTextFirst>();
    let mut state = use_signal(|| {
        if (preference.0)() {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        }
    });
    let mut touched = use_signal(|| false);

    use_effect(move || {
        let enabled = (preference.0)();
        if touched() {
            return;
        }
        state.set(if enabled {
            MediaState::Collapsed
        } else {
            MediaState::Thumbnail
        });
    });

    let host = link.uri.clone();
    let thumb = link.thumb.clone();

    rsx! {
        div { class: "link-card",
            a {
                class: "link-card__body",
                href: "{link.uri}",
                target: "_blank",
                rel: "noreferrer",
                span { class: "link-card__title", "{link.title}" }
                span { class: "link-card__desc", "{link.description}" }
                span { class: "link-card__host", "{host}" }
            }
            if let Some(src) = thumb {
                {match state() {
                    MediaState::Collapsed => rsx! {
                        button {
                            r#type: "button",
                            class: "alt-first__gate alt-first__gate--compact",
                            "aria-expanded": "false",
                            onclick: move |_| {
                                touched.set(true);
                                state.set(MediaState::Thumbnail);
                            },
                            span { class: "alt-first__action", "Show thumbnail" }
                        }
                    },
                    _ => rsx! {
                        div { class: "alt-first__media-wrap alt-first__media-wrap--link",
                            img { class: "alt-first__media", src: "{src}", alt: "", loading: "lazy" }
                            button {
                                r#type: "button",
                                class: "alt-first__close",
                                "aria-label": "Hide thumbnail",
                                onclick: move |_| {
                                    touched.set(true);
                                    state.set(MediaState::Collapsed);
                                },
                                "×"
                            }
                        }
                    },
                }}
            }
        }
    }
}


/// Owned mirror of `TextSegment`, because RSX closures outlive the borrowed text.
enum OwnedSegment {
    Plain(String),
    Mention { text: String, did: String },
    Link { text: String, uri: String },
    Tag { text: String, tag: String },
}

impl From<TextSegment<'_>> for OwnedSegment {
    fn from(segment: TextSegment<'_>) -> Self {
        match segment {
            TextSegment::Plain(text) => Self::Plain(text.to_string()),
            TextSegment::Mention { text, did } => Self::Mention {
                text: text.to_string(),
                did: did.to_string(),
            },
            TextSegment::Link { text, uri } => Self::Link {
                text: text.to_string(),
                uri: uri.to_string(),
            },
            TextSegment::Tag { text, tag } => Self::Tag {
                text: text.to_string(),
                tag: tag.to_string(),
            },
        }
    }
}

/// `https://bsky.app/profile/<actor>` → the actor, when the URL is exactly that.
///
/// Post links (`…/profile/<actor>/post/<rkey>`) are deliberately not converted: building the
/// at-uri needs a DID, and the actor in the URL may be a handle, so a half-conversion would
/// produce a link that 404s inside the app instead of working outside it.
fn bsky_profile_actor(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("https://bsky.app/profile/")?;
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return None;
    }
    Some(rest.to_string())
}

fn render_segment(segment: OwnedSegment) -> Element {
    match segment {
        OwnedSegment::Plain(text) => rsx! { "{text}" },
        // A mention is an account in this app, so it opens the profile page here. It used to
        // go to bsky.app in a new tab, which made every "who is this?" click leave the client.
        OwnedSegment::Mention { text, did } => rsx! {
            a {
                class: "facet facet--mention",
                href: "#/profile/{encode_uri(&did)}",
                "{text}"
            }
        },
        OwnedSegment::Link { text, uri } => {
            match bsky_profile_actor(&uri) {
                Some(actor) => rsx! {
                    a {
                        class: "facet facet--link",
                        href: "#/profile/{encode_uri(&actor)}",
                        "{text}"
                    }
                },
                None => rsx! {
                    a {
                        class: "facet facet--link",
                        href: "{uri}",
                        target: "_blank",
                        rel: "noreferrer",
                        "{text}"
                    }
                },
            }
        }
        OwnedSegment::Tag { text, tag } => rsx! {
            a {
                class: "facet facet--tag",
                href: "#/tag/{encode_uri(&tag)}",
                "{text}"
            }
        },
    }
}

/// Renders any embed, including the media attached to a quote post.
///
/// `origin` rides along so a quote opened from a feed carries that feed, exactly
/// like a card's own permalink — otherwise Back from a quoted post falls through
/// to the last feed instead of the one being read.
fn render_media(embed: EmbedView, origin: &str) -> Element {
    match embed {
        EmbedView::Images { images } => rsx! {
            div { class: "embed",
                for image in images {
                    AltFirstImage { image: image }
                }
            }
        },
        EmbedView::Gallery { items } => rsx! {
            div { class: "embed",
                for item in items {
                    AltFirstImage { image: item.into_image_view() }
                }
            }
        },
        EmbedView::Video(video) => rsx! {
            div { class: "embed", AltFirstVideo { video: video } }
        },
        EmbedView::External { external } => rsx! {
            div { class: "embed", AltFirstLinkThumbnail { link: external } }
        },
        EmbedView::Record { record } => rsx! {
            div { class: "embed", QuoteCard { record: record, origin: origin.to_string() } }
        },
        EmbedView::RecordWithMedia { record, media } => rsx! {
            div { class: "embed",
                QuoteCard { record: record.view().clone(), origin: origin.to_string() }
                {render_media(*media, origin)}
            }
        },
        EmbedView::Unknown => rsx! {},
    }
}

/// A quoted post. Deleted/blocked/detached quotes stay visible as placeholders —
/// silently dropping them changes what the reader thinks the author replied to.
///
/// The body carries `data-post-link` instead of being an anchor: a quote can
/// contain facet links of its own, and nesting anchors is invalid. The delegated
/// capture-phase handler walks up to the nearest `[data-post-link]`, so a click
/// inside a quote opens the quoted post, not the post doing the quoting.
#[component]
fn QuoteCard(record: EmbedRecordView, #[props(default)] origin: String) -> Element {
    match record {
        EmbedRecordView::Record(quoted) => {
            let name = quoted
                .author
                .display_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| quoted.author.handle.clone());
            let handle = format!("@{}", quoted.author.handle);
            let text = quoted
                .value
                .as_ref()
                .and_then(|value| value.text.clone())
                .unwrap_or_default();
            let facets = quoted.value.as_ref().and_then(|value| value.facets.clone());
            let segments: Vec<OwnedSegment> = text_segments(&text, facets.as_deref())
                .into_iter()
                .map(OwnedSegment::from)
                .collect();
            let media = quoted.embeds.clone().unwrap_or_default();
            let href = post_href(&quoted.uri, &origin);

            rsx! {
                div { class: "quote", "data-post-link": href,
                    header { class: "quote__head",
                        span { class: "quote__author", "{name}" }
                        span { class: "quote__meta", "{handle}" }
                    }
                    p { class: "quote__text",
                        for segment in segments {
                            {render_segment(segment)}
                        }
                    }
                    for embed in media {
                        {render_media(embed, &origin)}
                    }
                }
            }
        }
        EmbedRecordView::NotFound { .. } => rsx! {
            p { class: "quote quote--missing", "Quoted post unavailable" }
        },
        EmbedRecordView::Blocked { .. } => rsx! {
            p { class: "quote quote--missing", "Quoted post hidden (blocked)" }
        },
        EmbedRecordView::Detached { .. } => rsx! {
            p { class: "quote quote--missing", "Quoted post removed" }
        },
        EmbedRecordView::Unknown => rsx! {},
    }
}

/// The uri and kind an engagement route names.
fn engagement_target(route: &Route) -> (String, Engagement) {
    match route {
        Route::PostLikes { uri } => (uri.clone(), Engagement::Likes),
        Route::PostReposts { uri } => (uri.clone(), Engagement::Reposts),
        Route::PostQuotes { uri } => (uri.clone(), Engagement::Quotes),
        _ => (String::new(), Engagement::Likes),
    }
}

/// Which list of people/things hangs off a post.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Engagement {
    Likes,
    Reposts,
    Quotes,
}

/// What an engagement page shows: people, or posts (quotes).
enum EngagementList {
    Actors(Vec<alt_first_core::Author>),
    Posts(Vec<PostView>),
}

async fn fetch_engagement(
    uri: &str,
    kind: Engagement,
    state: SessionState,
) -> Result<EngagementList, String> {
    let encoded = encode_uri(uri);
    match kind {
        Engagement::Likes => {
            let response: LikesResponse = viewer_read(
                &format!("/xrpc/app.bsky.feed.getLikes?uri={encoded}&limit=50"),
                state,
            )
            .await?;
            Ok(EngagementList::Actors(
                response.likes.into_iter().map(|like| like.actor).collect(),
            ))
        }
        Engagement::Reposts => {
            let response: RepostedByResponse = viewer_read(
                &format!("/xrpc/app.bsky.feed.getRepostedBy?uri={encoded}&limit=50"),
                state,
            )
            .await?;
            Ok(EngagementList::Actors(response.reposted_by))
        }
        Engagement::Quotes => {
            let response: QuotesResponse = viewer_read(
                &format!("/xrpc/app.bsky.feed.getQuotes?uri={encoded}&limit=50"),
                state,
            )
            .await?;
            Ok(EngagementList::Posts(response.posts))
        }
    }
}

/// Who liked a post, who reposted it, or the posts quoting it — the list behind the
/// counts, which is what the counts link to.
#[component]
fn EngagementView(route: Signal<Route>) -> Element {
    let session_state = use_context::<SessionState>();
    let mut list = use_resource(move || {
        let (uri, kind) = engagement_target(&route());
        async move { fetch_engagement(&uri, kind, session_state).await }
    });
    let (_, kind) = engagement_target(&route());
    let heading = match kind {
        Engagement::Likes => "Liked by",
        Engagement::Reposts => "Reposted by",
        Engagement::Quotes => "Quoted by",
    };
    let empty = match kind {
        Engagement::Likes => "No likes yet.",
        Engagement::Reposts => "No reposts yet.",
        Engagement::Quotes => "Nothing quotes this post yet.",
    };

    rsx! {
        section { class: "engagement",
            button {
                r#type: "button",
                class: "thread__back",
                onclick: move |_| go_back(),
                "← Back"
            }
            h2 { class: "page-title", "{heading}" }
            {match &*list.read() {
                None => rsx! { p { class: "note", "Loading…" } },
                Some(Err(error)) => rsx! {
                    ErrorPanel {
                        message: error.clone(),
                        subject: Subject::Post,
                        on_retry: move |_| list.restart(),
                    }
                },
                Some(Ok(EngagementList::Actors(actors))) => {
                    let actors = actors.clone();
                    if actors.is_empty() {
                        rsx! { p { class: "note", "{empty}" } }
                    } else {
                        rsx! {
                            ul { class: "actor-list",
                                for actor in actors {
                                    li { class: "actor-list__item",
                                        a {
                                            class: "actor",
                                            href: "#/profile/{encode_uri(&actor.handle)}",
                                            {match actor.avatar.clone() {
                                                Some(src) => rsx! {
                                                    img { class: "avatar", src: "{src}", alt: "", loading: "lazy" }
                                                },
                                                None => rsx! { div { class: "avatar avatar--empty" } },
                                            }}
                                            span { class: "actor__name",
                                                {actor.display_name.clone().filter(|name| !name.trim().is_empty()).unwrap_or_else(|| actor.handle.clone())}
                                            }
                                            span { class: "actor__handle", "@{actor.handle}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Some(Ok(EngagementList::Posts(posts))) => {
                    let posts = posts.clone();
                    if posts.is_empty() {
                        rsx! { p { class: "note", "{empty}" } }
                    } else {
                        rsx! {
                            for post in posts {
                                PostCard {
                                    post: post,
                                    origin: format!("#/post/{}/quotes", encode_uri(&engagement_target(&route()).0)),
                                }
                            }
                        }
                    }
                }
            }}
        }
    }
}

/// Writing a new post, on its own page.
///
/// Deliberately shaped like the reply box that already works: same counter, same
/// disabled-while-empty rule, same error line under it. Only the record differs — no `reply`
/// refs — and the destination afterwards, which is the author's own profile because that page
/// refetches and will actually show the post. A feed only picks it up when its revalidation
/// runs, and claiming otherwise would be the kind of lie this project keeps a table for.
#[component]
fn ComposeView() -> Element {
    let session_state = use_context::<SessionState>();
    let mut text = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let Some(signed) = (session_state.0)() else {
        return rsx! {
            section { class: "compose",
                h2 { class: "page-title", "New post" }
                p { class: "note", "Writing a post needs a signed-in session." }
                button {
                    r#type: "button",
                    class: "feed-tab feed-tab--active",
                    onclick: move |_| navigate("#/sign-in"),
                    "Sign in"
                }
            }
        };
    };

    let handle = signed.handle.clone();
    rsx! {
        section { class: "compose",
            h2 { class: "page-title", "New post" }
            form {
                class: "reply-box compose__box",
                onsubmit: move |event| {
                    event.prevent_default();
                    let value = text().trim().to_string();
                    if value.is_empty() || busy() {
                        return;
                    }
                    let Some(signed) = (session_state.0)() else {
                        return;
                    };
                    let handle = handle.clone();
                    busy.set(true);
                    error.set(None);
                    spawn(async move {
                        match create_post(&signed, &value).await {
                            Ok(_) => {
                                text.set(String::new());
                                navigate(&format!("#/profile/{}", encode_uri(&handle)));
                            }
                            Err(err) => error.set(Some(err)),
                        }
                        busy.set(false);
                    });
                },
                textarea {
                    class: "reply-box__input",
                    rows: "5",
                    "aria-label": "Post text",
                    placeholder: "What do you want to say?",
                    value: "{text}",
                    oninput: move |event| text.set(event.value()),
                }
                div { class: "reply-box__row",
                    span { class: "reply-box__count", "{text().chars().count()}/300" }
                    button {
                        r#type: "submit",
                        class: "reply-box__send",
                        // Same rule as the reply box, deliberately: chars, not graphemes, so it
                        // can only ever be stricter than the server's limit.
                        disabled: busy()
                            || text().trim().is_empty()
                            || text().chars().count() > 300,
                        if busy() { "Posting…" } else { "Post" }
                    }
                }
                if let Some(err) = error() {
                    p { class: "reply-box__error", "{err}" }
                }
            }
            p { class: "note",
                "Text only for now. Images would need alt text to be required, which is a \
                 separate decision. Links, mentions and hashtags are linked."
            }
        }
    }
}


/// Reading preferences, on a page — the same shape as Feeds, because a popover for
/// one checkbox was a menu pretending to be a destination.
#[component]
fn SettingsView() -> Element {
    let mut preference = use_context::<AltTextFirst>();
    let mut theme = use_context::<ThemePref>();
    let session = (use_context::<SessionState>().0)();
    // Whether this session is on disk — the question a reader actually has after a reload
    // dropped them at the sign-in page.
    let mut saved = use_signal(|| false);
    use_future(move || async move {
        let value: Option<String> = document::eval(&format!(
            "dioxus.send(localStorage.getItem({}) ?? '');",
            serde_json::to_string(SESSION_KEY).unwrap()
        ))
        .recv()
        .await
        .ok();
        saved.set(value.is_some_and(|text| !text.is_empty()));
    });
    rsx! {
        section { class: "settings-page",
            h2 { class: "page-title", "Settings" }
            // Which kind of session this is, because they differ in what they can be:
            // an app password is a credential for the whole account, an OAuth grant is a
            // scoped session revoked at the server on sign-out.
            if let Some(signed) = session.clone() {
                div { class: "settings-row",
                    span { class: "settings-row__control",
                        if signed.oauth.is_some() { "Signed in with OAuth" } else { "Signed in with an app password" }
                    }
                    p { class: "settings-row__hint",
                        if saved() {
                            "Saved on this device, so a reload resumes it. "
                        } else {
                            "In memory only — a reload will sign you out. "
                        }
                        if let Some(oauth) = &signed.oauth {
                            "@{signed.handle} · scope {oauth.scope}. Signing out revokes this session at your server and deletes the key this browser signs with."
                        } else {
                            "@{signed.handle} · the password is exchanged for a session and never stored; it carries full write access, so revoke it in Bluesky settings when you are done."
                        }
                    }
                }
            }
            div { class: "settings-row",
                label { class: "toggle settings-row__control",
                    input {
                        r#type: "checkbox",
                        checked: (preference.0)(),
                        onchange: move |_| preference.0.set(!(preference.0)()),
                    }
                    span { "Start every embed as alt text" }
                }
                p { class: "settings-row__hint",
                    "New embeds open as their description instead of the image or video. Changing this moves every embed you have not touched yet."
                }
            }
            div { class: "settings-row",
                span { class: "settings-row__control", "Theme" }
                div { class: "theme-choice", role: "group", "aria-label": "Theme",
                    for option in [Theme::System, Theme::Light, Theme::Dark] {
                        button {
                            r#type: "button",
                            class: if (theme.0)() == option {
                                "theme-choice__option theme-choice__option--active"
                            } else {
                                "theme-choice__option"
                            },
                            "aria-pressed": if (theme.0)() == option { "true" } else { "false" },
                            "data-theme-value": option.as_str(),
                            onclick: move |_| {
                                theme.0.set(option);
                                apply_theme(option);
                            },
                            {match option {
                                Theme::System => "System",
                                Theme::Light => "Light",
                                Theme::Dark => "Dark",
                            }}
                        }
                    }
                }
                p { class: "settings-row__hint",
                    "System follows your device's appearance. An explicit Light or Dark is remembered on this device and wins over the system setting."
                }
            }
        }
    }
}

/// What a card knows about the parent of a reply.
///
/// The middle two are the reason this is an enum: "we have not looked it up" and "the AppView
/// will not return it" both used to arrive as `None`, which is also what "not a reply" looked
/// like — so a reply silently rendered as a top-level post whenever the lookup was slow or the
/// request failed.
enum ReplyContext {
    NotAReply,
    Unknown,
    Unavailable,
    Handle(String),
}

/// Feather icons (MIT), inlined so there is no icon-font dependency.
fn action_icon(kind: &str) -> Element {
    let paths: Vec<&str> = match kind {
        "repost" => vec![
            "M17 1l4 4-4 4",
            "M3 11V9a4 4 0 0 1 4-4h14",
            "M7 23l-4-4 4-4",
            "M21 13v2a4 4 0 0 1-4 4H3",
        ],
        "search" => vec![
            "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16z",
            "M21 21l-4.35-4.35",
        ],
        // A pen, for writing: the one action the app could not do until now.
        "compose" => vec!["M12 20h9", "M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4z"],
        "home" => vec![
            "M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
            "M9 22V12h6v10",
        ],
        "bell" => vec![
            "M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9",
            "M13.73 21a2 2 0 0 1-3.46 0",
        ],
        "list" => vec![
            "M8 6h13",
            "M8 12h13",
            "M8 18h13",
            "M3 6h.01",
            "M3 12h.01",
            "M3 18h.01",
        ],
        "user" => vec![
            "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2",
            "M12 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
        ],
        "settings" => vec![
            "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
            "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z",
        ],
        "like" => vec![
            "M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z",
        ],
        // Feather "share": the arrow-out-of-a-box that means "copy a link".
        "share" => vec![
            "M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8",
            "M16 6l-4-4-4 4",
            "M12 2v13",
        ],
        _ => vec![
            "M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z",
        ],
    };

    rsx! {
        svg {
            class: "icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "1.8",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            "aria-hidden": "true",
            for d in paths {
                path { d: "{d}" }
            }
        }
    }
}

#[component]
fn PostCard(
    post: PostView,
    #[props(default)] reposted_by: Option<String>,
    /// Threads pass `false`: a post inside its own thread should not link to itself.
    #[props(default = true)] openable: bool,
    /// True only for the post the reader opened (the thread's focal node). The
    /// "Liked by / Reposted by / quotes" line is for that post alone, as in the
    /// official app — on a feed card it would be noise on every row.
    #[props(default)] focal: bool,
    /// Feed this card is rendered in, carried into the post URL so a tab opened
    /// cold still knows where Back should return to.
    #[props(default)] origin: String,
) -> Element {
    let uri = post.uri.clone();
    // Needed to build the like record's subject.
    let cid = post.cid.clone();
    let permalink = post_href(&uri, &origin);
    let actor = post.author.handle.clone();
    let profile_href = format!("#/profile/{}", encode_uri(&actor));
    let display_name = post
        .author
        .display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| post.author.handle.clone());
    let handle = format!("@{}", post.author.handle);
    // The author's own clock, because that is when the post was written; `indexedAt` is
    // when the AppView first saw it and drifts by seconds or, for a backfilled post,
    // by far more. The visible text is a fallback: `install_when` rewrites it to relative
    // time in the reader's locale and puts the exact local time in `title`.
    let created_at = post
        .record
        .created_at
        .clone()
        .or_else(|| post.indexed_at.clone())
        .unwrap_or_default();
    let when = short_date(&created_at);
    let text = post.record.text.clone().unwrap_or_default();
    let facets = post.record.facets.clone();
    let embed = post.embed.clone();
    let avatar = post.author.avatar.clone();
    let replies = count_label(post.reply_count);
    // Feed and profile cards know the feed they belong to; thread nodes do not (their
    // nesting already shows the parent), so the line is for lists only.
    let parent_handles = use_context::<ParentHandles>().0;
    // What this card knows about the parent of a reply. Three states rather than two, because
    // "the parent has not been looked up (yet, or at all)" is not the same as "this is not a
    // reply": collapsing them made a reply render as a top-level post whenever the lookup was
    // slow or failed, and that is why the line came and went.
    let reply_context = if origin.is_empty() {
        ReplyContext::NotAReply
    } else {
        match post.record.reply.as_ref() {
            None => ReplyContext::NotAReply,
            Some(reply) => match parent_handles.read().get(&reply.parent.uri) {
                Some(Some(handle)) => ReplyContext::Handle(handle.clone()),
                Some(None) => ReplyContext::Unavailable,
                None => ReplyContext::Unknown,
            },
        }
    };

    // One map for both reactions. A card in a feed, a thread and a profile all read
    // it, so an optimistic toggle shows up everywhere the post appears.
    let reaction_ctx = use_context::<ReactionCtx>();
    let session_state = use_context::<SessionState>();
    let deleted_posts = use_context::<DeletedPosts>();
    let viewer_like = post.viewer.as_ref().and_then(|viewer| viewer.like.clone());
    let viewer_repost = post.viewer.as_ref().and_then(|viewer| viewer.repost.clone());
    // `viewer.repost` is the whole point of rendering this: without it a post you
    // already reposted looked untouched, which is what made the control a lie.
    let (like_view, repost_view) = {
        let shared = reaction_ctx.state.read();
        let like = shared
            .get(&(Reaction::Like, uri.clone()))
            .cloned()
            .unwrap_or(ReactionView {
                on: viewer_like.is_some(),
                record: viewer_like,
                count: post.like_count.unwrap_or(0),
            });
        let repost = shared
            .get(&(Reaction::Repost, uri.clone()))
            .cloned()
            .unwrap_or(ReactionView {
                on: viewer_repost.is_some(),
                record: viewer_repost,
                count: post.repost_count.unwrap_or(0),
            });
        (like, repost)
    };
    let like_pending = reaction_ctx.pending.read().contains(&(Reaction::Like, uri.clone()));
    let repost_pending = reaction_ctx.pending.read().contains(&(Reaction::Repost, uri.clone()));
    let like_failure = reaction_ctx.errors.read().get(&(Reaction::Like, uri.clone())).cloned();
    let repost_failure = reaction_ctx
        .errors
        .read()
        .get(&(Reaction::Repost, uri.clone()))
        .cloned();
    let like_class = reaction_class(Reaction::Like, like_view.on, like_pending);
    let repost_class = reaction_class(Reaction::Repost, repost_view.on, repost_pending);
    let like_label = if like_view.on { "Unlike" } else { "Like" };
    let repost_label = if repost_view.on { "Undo repost" } else { "Repost" };
    let like_count = like_view.count;
    let repost_count = repost_view.count;
    let quote_count = post.quote_count.unwrap_or(0);
    // The counts belong inside their buttons (that is where the icon's gap belongs).
    // The way to *see* who is behind them is this line, in the official app's order —
    // reposts, quotes, likes, saves — and only on the opened post. All four slots
    // always render, zeros included, so the line does not reflow per post; a zero is
    // plain text because there is no list behind it.
    let count_noun = |count: i64, singular: &str, plural: &str| {
        if count == 1 {
            format!("1 {singular}")
        } else {
            format!("{count} {plural}")
        }
    };
    let count_link = |count: i64, suffix: &str| {
        if count > 0 {
            Some(format!("#/post/{}/{suffix}", encode_uri(&uri)))
        } else {
            None
        }
    };
    let saves = post.bookmark_count.unwrap_or(0);
    let engagement: Vec<(String, Option<String>, &'static str)> = vec![
        (
            count_noun(repost_count, "repost", "reposts"),
            count_link(repost_count, "reposts"),
            "",
        ),
        (
            count_noun(quote_count, "quote", "quotes"),
            count_link(quote_count, "quotes"),
            "",
        ),
        (
            count_noun(like_count, "like", "likes"),
            count_link(like_count, "likes"),
            "",
        ),
        // Bookmarks are private: a count, never a link.
        (
            count_noun(saves, "save", "saves"),
            None,
            "Bookmarks are private",
        ),
    ];

    // Each button only captures a clone of its view; the shared work lives in
    // toggle_reaction so a like and a repost cannot drift apart.
    let on_like = {
        let target = uri.clone();
        let cid = cid.clone();
        let view = like_view.clone();
        move |_| {
            toggle_reaction(
                reaction_ctx,
                session_state,
                Reaction::Like,
                target.clone(),
                cid.clone(),
                view.clone(),
            )
        }
    };
    let on_repost = {
        let target = uri.clone();
        let cid = cid.clone();
        let view = repost_view.clone();
        move |_| {
            toggle_reaction(
                reaction_ctx,
                session_state,
                Reaction::Repost,
                target.clone(),
                cid.clone(),
                view.clone(),
            )
        }
    };
    // Owned segments: the borrowed ones can't outlive the RSX closure.
    let segments: Vec<OwnedSegment> = text_segments(&text, facets.as_deref())
        .into_iter()
        .map(OwnedSegment::from)
        .collect();

    // The account asked not to be shown to signed-out readers, and the AppView marks it
    // on `author.labels`. Rendering the card anyway is the exact thing that choice is
    // about, so it becomes a sign-in prompt — one gate here covers feeds, threads,
    // quotes and profile lists alike.
    // Gone from the repository, so gone from every list that still has it cached.
    if (deleted_posts.0)().contains(&uri) {
        return rsx! {};
    }

    if (session_state.0)().is_none() && post.hidden_when_logged_out() {
        return rsx! {
            article { class: "post post--gated",
                p { class: "post__gated-note",
                    "This account only shows its posts to signed-in readers."
                }
            }
        };
    }

    rsx! {
        article { class: "post", "data-post-link": if openable { permalink.clone() } else { String::new() },
            if let Some(label) = reposted_by {
                div { class: "post__repost",
                    {action_icon("repost")}
                    span { "{label}" }
                }
            }
            a { class: "avatar-link", href: "{profile_href}", "aria-label": "Open profile",
                {match avatar {
                    Some(src) => rsx! {
                        img { class: "avatar", src: "{src}", alt: "", loading: "lazy" }
                    },
                    None => rsx! { div { class: "avatar avatar--empty" } },
                }}
            }
            div { class: "post__main",
                header { class: "post__head",
                    a { class: "post__author", href: "{profile_href}", "{display_name}" }
                    a { class: "post__meta", href: "{profile_href}", "{handle}" }
                    a { class: "post__meta post__permalink", href: "{permalink}",
                        span {
                            class: "post__when",
                            // Absolute on the post you opened, relative everywhere else —
                            // the official arrangement, and the only way to answer "what
                            // time was this posted" without hovering.
                            "data-when": "{created_at}",
                            "data-format": if focal { "absolute" } else { "relative" },
                            "· {when}"
                        }
                    }
                }
                // Navigation lives on the article (data-post-link) and is handled by
                // a delegated listener that ignores clicks made during a selection,
                // so post text stays selectable.
                match reply_context {
                    // A reply in a feed is otherwise indistinguishable from a top-level
                    // post until you read what it answers.
                    ReplyContext::Handle(handle) => rsx! {
                        p { class: "post__reply-context", "↳ Replying to @{handle}" }
                    },
                    ReplyContext::Unavailable => rsx! {
                        p { class: "post__reply-context", "↳ Replying to an unavailable post" }
                    },
                    // Not looked up yet — or the lookup failed. Saying so is honest and, more
                    // to the point, keeps the card from pretending to be a top-level post.
                    ReplyContext::Unknown => rsx! {
                        p { class: "post__reply-context", "↳ Replying to a post" }
                    },
                    ReplyContext::NotAReply => rsx! {},
                }
                p { class: "post__text",
                    for segment in segments {
                        {render_segment(segment)}
                    }
                }
                if let Some(embed) = embed {
                    {render_media(embed, &origin)}
                }
                if focal {
                    // Above the icons, and only on the opened post. Four slots every
                    // time, so the line sits still as counts change.
                    p { class: "post__engagement",
                        for (index, (label, href, note)) in engagement.into_iter().enumerate() {
                            if index > 0 {
                                span { class: "post__engagement-sep", "·" }
                            }
                            {match href {
                                Some(href) => rsx! {
                                    a { class: "post__engagement-link", href: "{href}", "{label}" }
                                },
                                None => rsx! {
                                    span { class: "post__engagement-count", title: "{note}", "{label}" }
                                },
                            }}
                        }
                    }
                }
                div { class: "post__actions",
                    span { class: "post__action post__action--reply",
                        {action_icon("reply")}
                        if !replies.is_empty() {
                            span { class: "post__action-count", "{replies}" }
                        }
                    }
                    button {
                        r#type: "button",
                        class: repost_class,
                        disabled: repost_pending,
                        aria_pressed: if repost_view.on { "true" } else { "false" },
                        aria_busy: if repost_pending { "true" } else { "false" },
                        aria_label: repost_label,
                        onclick: on_repost,
                        {action_icon("repost")}
                        if repost_count > 0 {
                            span { class: "post__action-count", "{repost_count}" }
                        }
                    }
                    button {
                        r#type: "button",
                        class: like_class,
                        disabled: like_pending,
                        aria_pressed: if like_view.on { "true" } else { "false" },
                        aria_busy: if like_pending { "true" } else { "false" },
                        aria_label: like_label,
                        onclick: on_like,
                        {action_icon("like")}
                        if like_count > 0 {
                            span { class: "post__action-count", "{like_count}" }
                        }
                    }
                    // Share, then the context menu: the two controls the official app
                    // keeps at the end of this row.
                    ShareMenu { handle: actor.clone(), uri: uri.clone() }
                    PostMenu { handle: actor.clone(), did: post.author.did.clone(), uri: uri.clone(), text: text.clone() }
                }
                if let Some(note) = like_failure {
                    // `role="status"` so it is announced, not just drawn.
                    p { class: "post__action-error", role: "status", "{note}" }
                }
                if let Some(note) = repost_failure {
                    p { class: "post__action-error", role: "status", "{note}" }
                }
            }
        }
    }
}

/// "2026-10-25T01:23:45.000Z" -> "Oct 25". No clock dependency, no timezone maths,
/// which is all a feed byline needs while the payload has no relative-time helper.
fn short_date(iso: &str) -> String {
    let date = iso.split('T').next().unwrap_or_default();
    let mut parts = date.split('-');
    let (Some(_year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return String::new();
    };
    let month = match month {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => return String::new(),
    };
    format!("{month} {}", day.trim_start_matches('0'))
}

fn count_label(count: Option<i64>) -> String {
    match count {
        Some(value) if value > 0 => value.to_string(),
        _ => String::new(),
    }
}

/// Class list for a reaction button: its kind, whether it is on, and whether a
/// write for it is in flight (which disables it in the markup).
fn reaction_class(kind: Reaction, on: bool, pending: bool) -> String {
    let (base, active) = match kind {
        Reaction::Like => ("post__action post__action--like", "post__action--liked"),
        Reaction::Repost => ("post__action post__action--repost", "post__action--reposted"),
    };
    let mut class = base.to_string();
    if on {
        class.push(' ');
        class.push_str(active);
    }
    if pending {
        class.push_str(" post__action--pending");
    }
    class
}



#[component]
fn App() -> Element {
    let mut preference = use_context_provider(|| AltTextFirst(Signal::new(true)));
    // Theme comes from storage before the first interactive frame: the CSS media query
    // already painted the right colours, this only re-asserts an explicit choice.
    let mut theme = use_context_provider(|| ThemePref(Signal::new(Theme::System)));
    use_future(move || async move {
        let stored = stored_theme().await;
        theme.0.set(stored);
        apply_theme(stored);
    });
    use_effect(move || {
        inject_css();
        install_hls_shim();
        install_click_navigation();
        install_double_click_top();
        install_menu_dismiss();
        install_feed_strip_drag();
        install_when();
        install_scroll_memory();
        install_auto_load();
        // The DPoP helpers are defined once per page load, before any flow needs them, so a
        // callback arriving on a cold load finds them ready. Only when this build offers
        // OAuth: a fork that never signs in that way should not load key-handling code.
        if sign_in_mode().offers_oauth() && OAUTH_IMPLEMENTED {
            spawn(oauth_flow::install_dpop());
        }
        // Not OAuth-only: app-password sessions rotate too, and two tabs rotating at the same
        // moment is what used to sign one of them out.
        spawn(install_auth_channel());
    });

    let mut session = use_context_provider(|| SessionState(Signal::new(None)));
    let mut route = use_signal(|| parse_hash(""));

    // Reaction state is shared with every card through context, so a like or a
    // repost made in a thread is visible in the feed behind it without refetching.
    let reactions = use_signal(
        std::collections::HashMap::<(Reaction, String), ReactionView>::new,
    );
    let reaction_pending = use_signal(
        std::collections::HashSet::<(Reaction, String)>::new,
    );
    let reaction_errors = use_signal(
        std::collections::HashMap::<(Reaction, String), String>::new,
    );
    // Filled by every page load; read by PostCard for its "Replying to" line.
    let parent_handles = use_signal(std::collections::HashMap::<String, Option<String>>::new);
    use_context_provider(|| ParentHandles(parent_handles));
    use_context_provider(|| ReactionCtx {
        state: reactions,
        pending: reaction_pending,
        errors: reaction_errors,
    });
    let mut login_error = use_signal(|| None::<String>);
    use_context_provider(|| LoginState(login_error));
    let mut saved_feeds = use_signal(Vec::<(String, String)>::new);
    use_context_provider(|| FeedsState(saved_feeds));
    let deleted_posts = use_signal(std::collections::HashSet::<String>::new);
    use_context_provider(|| DeletedPosts(deleted_posts));

    // Hash routing: this listener owns `Route`, so the address bar, back/forward,
    // and Android's back gesture all work without a router crate.
    // One source of truth for the current location.
    //
    // Each iteration is a *fresh* eval that waits for a `hashchange` and gives up
    // after a second, so navigation is immediate but an idle feed costs one eval per
    // second rather than eight. The earlier version held one long-lived channel and
    // died silently with it — that is why Android stopped reacting to taps while the
    // UI kept rendering. Re-arming every second means the channel cannot stay dead.
    use_future(move || async move {
        let mut seen = String::new();
        loop {
            let mut eval = document::eval(
                r#"(async () => {
                    await new Promise((resolve) => {
                        const timer = setTimeout(resolve, 1000);
                        window.addEventListener('hashchange', () => {
                            clearTimeout(timer);
                            resolve();
                        }, { once: true });
                    });
                    dioxus.send(location.hash);
                })()"#,
            );
            let hash = eval.recv::<String>().await.unwrap_or_default();
            if hash != seen {
                seen = hash.clone();
                route.set(parse_hash(&hash));
            }
        }
    });

    // Landing on a bare URL reopens the last feed you were reading. An explicit
    // hash is left alone: the route you asked for must win.
    use_future(move || async move {
        if let Some(target) = restore_target().await {
            navigate(&target);
        }
    });

    // Finish an OAuth sign-in first, then restore a remembered session. Both are "what is the
    // session on this first frame", so they share one effect and the callback wins: if this
    // tab came back from the authorisation server, that is the newer intent.
    //
    // The restore rotates before trusting: an access token from last week is expired, and a
    // revoked refresh token means the stored session is dead and should be dropped.
    use_future(move || async move {
        match oauth_flow::complete().await {
            Ok(Some(completed)) => {
                let fresh = Session {
                    handle: if completed.handle.is_empty() {
                        completed.did.clone()
                    } else {
                        completed.handle.clone()
                    },
                    did: completed.did.clone(),
                    access_jwt: completed.access_jwt.clone(),
                    refresh_jwt: completed.refresh_jwt.clone(),
                    pds: completed.pds.clone(),
                    oauth: Some(completed.oauth.clone()),
                    remembered: completed.remember,
                };
                session.0.set(Some(fresh.clone()));
                if let Ok(feeds) = fetch_saved_feeds(session).await {
                    saved_feeds.set(feeds);
                }
                persist_session(Some(&fresh), completed.remember);
                // Say which it is: the difference is invisible until a reload, and "I
                // refreshed and it was gone" is the report this answers.
                log_to_console(if completed.remember {
                    "signed in; session saved on this device"
                } else {
                    "signed in; session kept in memory only and will not survive a refresh"
                });
                navigate("#/");
            }
            Ok(None) => {
                // A callback URL with nothing to complete means the attempt was lost (storage
                // cleared, another tab, an expired code): say so rather than sit there.
                if oauth_flow::on_callback_route().await {
                    let message = "That sign-in could not be completed — the attempt had expired. Try again."
                        .to_string();
                    log_to_console(&message);
                    login_error.set(Some(message));
                    navigate("#/sign-in");
                } else if let Some(stored) = load_stored_session().await {
                    // Use the stored token rather than rotating on every load. Rotating here
                    // cost a token-endpoint round trip per reload, invalidated the DPoP nonce
                    // cached for the old token — a guaranteed `401` on the first PDS call of
                    // every load — and raced other tabs for a single-use refresh token. A
                    // request that needs a new token still refreshes on its own 401 — reads
                    // included, through `authed_get`.
                    session.0.set(Some(stored.clone()));
                    if let Ok(feeds) = fetch_saved_feeds(session).await {
                        saved_feeds.set(feeds);
                    }
                }
            }
            Err(err) => {
                // Visible on the page, and in the console: the page is where a reader looks,
                // but the console is where the actual server response can be read.
                log_to_console(&err);
                login_error.set(Some(err));
                navigate("#/sign-in");
            }
        }
    });

    let mut feed_items = use_signal(Vec::<(PostView, Option<String>)>::new);
    let mut feed_cursor = use_signal(|| None::<String>);
    let mut feed_error = use_signal(|| None::<String>);
    let mut feed_loading = use_signal(|| false);
    // Guards against a slow response landing after the route changed.
    let mut feed_generation = use_signal(|| 0u64);
    // Fetched pages, kept across post navigation so back does not reset the feed.
    let mut feed_cache = use_signal(FeedCache::default);
    // Fresher page waiting to be accepted; see `PendingRefresh`.
    let mut feed_pending = use_signal(|| Option::<PendingRefresh>::None);
    // Bumped by "Try again" in the error panel; read by the load effect.
    let mut retry_tick = use_signal(|| 0u32);

    // (Re)load the first page whenever the route, session or saved feeds change.
    use_effect(move || {
        let current_route = route();
        let saved = saved_feeds();
        let current = (session.0)();
        let source = source_for(&current_route, &saved);
        // One label resolver for tabs, title and any header, so "Discover" and
        // "What's Hot" — the same generator under two names — cannot disagree.
        let label = source
            .as_ref()
            .map(|source| label_for(source, &saved, current.is_some()))
            .unwrap_or_default();
        update_title(&current_route, &label);
        // "Try again" re-runs this effect; there is no `entered` guard to skip it,
        // so a retry always reaches the fetch below.
        let _retry = retry_tick();
        let generation = (*feed_generation.peek()).wrapping_add(1);
        feed_generation.set(generation);
        feed_error.set(None);
        feed_pending.set(None);
        let Some(source) = source else {
            // Profile and post routes fetch their own data.
            feed_items.set(Vec::new());
            feed_cursor.set(None);
            return;
        };
        // A return visit restores the pages already scrolled plus the reader's
        // place. Refetching page one here is what made "back" look like a reload.
        let key = source.key();
        if let Some((items, cursor)) = feed_cache.peek().get(&key) {
            let known: std::collections::HashSet<String> =
                items.iter().map(|(post, _)| post.uri.clone()).collect();
            feed_cursor.set(cursor);
            feed_loading.set(false);
            // Cached pages are served instantly *and* get their reply parents resolved,
            // which is what the early return used to skip.
            publish_feed_page(items, feed_items, parent_handles);
            restore_scroll();
            // Cached pages are served instantly, then checked: a cache that never
            // revalidates silently pins the reader to whatever the feed looked
            // like last time. The refreshed page is held for the reader to accept,
            // because injecting it would move everything under their thumb.
            spawn(revalidate_feed(
                source,
                current,
                session,
                known,
                generation,
                feed_generation,
                feed_pending,
            ));
            return;
        }
        feed_items.set(Vec::new());
        feed_cursor.set(None);
        feed_loading.set(true);
        spawn(async move {
            let mut current = current;
            let mut result = fetch_feed(source.clone(), current.clone(), None, session).await;
            // Access tokens expire in ~2 h; rotate and retry once instead of
            // surfacing a bare 401.
            if let (Err(error), Some(existing)) = (&result, current.clone()) {
                if is_expired_token(error) {
                    // One implementation of "rotate", shared with reads and reactions, so the
                    // cross-tab race is handled once: a tab whose token another tab just spent
                    // adopts that pair instead of reporting a dead grant.
                    match rotate_session(session, &existing).await {
                        Ok(fresh) => {
                            current = Some(fresh);
                            result = fetch_feed(source, current, None, session).await;
                        }
                        Err(refresh_error) => {
                            log_to_console(&format!("refresh failed: {refresh_error}"));
                            if is_dead_grant(&refresh_error) {
                                forget_session(session, &refresh_error);
                                saved_feeds.set(Vec::new());
                                return;
                            }
                        }
                    }
                }
            }
            if *feed_generation.peek() != generation {
                return; // stale: the route moved on while this was in flight
            }
            match result {
                Ok(response) => {
                    let items = feed_items_of(&response);
                    feed_cache
                        .write()
                        .insert(key, items.clone(), response.cursor.clone());
                    publish_feed_page(items, feed_items, parent_handles);
                    feed_cursor.set(response.cursor);
                    // A cold tab (new tab, or reload) refills to the saved offset.
                    restore_scroll();
                }
                Err(error) => feed_error.set(Some(error)),
            }
            feed_loading.set(false);
        });
    });

    // Cards carry the route they were rendered in, so a post opened cold in a
    // new tab still has somewhere to go Back to.
    let feed_origin = match route() {
        Route::Post { .. }
        | Route::Profile { .. }
        | Route::PostLikes { .. }
        | Route::PostReposts { .. }
        | Route::PostQuotes { .. }
        | Route::Feeds
        | Route::Settings
        | Route::Compose
        | Route::Hashtag { .. }
        | Route::Notifications => String::new(),
        feed => route_hash(&feed),
    };

    // The rail marks what is active, so it needs the route and the identity.
    let current_route = route();
    let signed_handle = (session.0)().map(|signed| signed.handle.clone());
    // One name for `#/`, resolved the same way the tab and title resolve it.
    let discover_name = label_for(
        &FeedSource::Custom {
            uri: DISCOVER_FEED.to_string(),
            label: "Discover".to_string(),
        },
        &saved_feeds(),
        (session.0)().is_some(),
    );

    rsx! {
        div { class: "shell",
            // Desktop navigation, the shape the official web app uses. Only the
            // destinations this client can actually serve: no Notifications, Chat or
            // Lists until those surfaces exist, because a rail item that opens
            // nothing is worse than a rail with fewer items.
            aside { class: "rail", "aria-label": "Primary",
                // The account sits above Home, as in the official rail; the app's own
                // title now lives above the feed tabs in the column instead.
                if let Some(signed) = signed_handle.clone() {
                    {
                        let did = (session.0)().map(|s| s.did.clone()).unwrap_or_default();
                        rsx! {
                            AccountMenu {
                                handle: signed,
                                did: did.clone(),
                                on_sign_out: move |_| sign_out_now(session, saved_feeds),
                            }
                        }
                    }
                } else {
                    // Signed out, the same slot holds the equivalent: who you are (nobody)
                    // and the way in. It belongs here, not in the column — the column is
                    // the reading surface.
                    div { class: "rail__session",
                        span { class: "badge", "public · logged out" }
                        if !matches!(current_route, Route::SignIn) {
                            a {
                                class: "session-button",
                                "data-action": "rail-sign-in",
                                href: "#/sign-in",
                                "Sign in"
                            }
                        }
                    }
                }
                // Order follows the official rail: Home, then Explore, then feeds.
                // Home is shown signed out too — it opens sign-in instead of
                // navigating to a page that can only say "not authorized".
                a {
                    class: if matches!(current_route, Route::Following) { "rail__item rail__item--active" } else { "rail__item" },
                    href: "#/following",
                    // Home always answers a double-click, even from another feed: it
                    // navigates, but skips the saved offset and lands at the top.
                    "data-scroll-top": "home",
                    onclick: move |event| {
                        if (session.0)().is_none() {
                            event.prevent_default();
                            navigate("#/sign-in");
                        }
                    },
                    {action_icon("home")}
                    span { "Home" }
                }
                // `#/` is the Explore page (search + trending), so it is named for
                // what it is rather than for the generator the Feeds page lists.
                a {
                    class: if matches!(current_route, Route::Home) { "rail__item rail__item--active" } else { "rail__item" },
                    href: "#/",
                    {action_icon("search")}
                    span { "Explore" }
                }
                a {
                    class: if matches!(current_route, Route::Notifications) { "rail__item rail__item--active" } else { "rail__item" },
                    href: "#/notifications",
                    {action_icon("bell")}
                    span { "Notifications" }
                }
                // Feeds get their own page, as they do officially, rather than a list
                // growing inside the rail.
                a {
                    class: if matches!(current_route, Route::Feeds) { "rail__item rail__item--active" } else { "rail__item" },
                    href: "#/feeds",
                    {action_icon("list")}
                    span { "Feeds" }
                }
                div { class: "rail__bottom",
                    if let Some(handle) = signed_handle.clone() {
                        a {
                            class: if matches!(&current_route, Route::Profile { actor } if actor == &handle) { "rail__item rail__item--active" } else { "rail__item" },
                            href: "#/profile/{encode_uri(&handle)}",
                            {action_icon("user")}
                            span { "Profile" }
                        }
                    }
                    a {
                        class: if matches!(current_route, Route::Settings) { "rail__item rail__item--active" } else { "rail__item" },
                        href: "#/settings",
                        aria_label: "Settings",
                        {action_icon("settings")}
                        span { "Settings" }
                    }
                    // Below the account and Settings, not above them: the rail's top half is
                    // where you go, and the bottom is where you act.
                    if signed_handle.is_some() {
                        a {
                            class: if matches!(current_route, Route::Compose) { "rail__item rail__item--active" } else { "rail__item" },
                            href: "#/compose",
                            {action_icon("compose")}
                            span { "New post" }
                        }
                    }
                }
            }
            // The rail needs 900 px. Below that this row is the only navigation — without it
            // Explore and Notifications were unreachable on a phone except by typing a URL,
            // and the tab row next to the feed is *feed* tabs, not destinations.
            nav { class: "bottom-nav", "aria-label": "Primary",
                a {
                    class: if matches!(current_route, Route::Following) { "bottom-nav__item bottom-nav__item--active" } else { "bottom-nav__item" },
                    href: "#/following",
                    "data-scroll-top": "home",
                    onclick: move |event| {
                        if (session.0)().is_none() {
                            event.prevent_default();
                            navigate("#/sign-in");
                        }
                    },
                    {action_icon("home")}
                    span { class: "bottom-nav__label", "Home" }
                }
                a {
                    class: if matches!(current_route, Route::Home) { "bottom-nav__item bottom-nav__item--active" } else { "bottom-nav__item" },
                    href: "#/",
                    {action_icon("search")}
                    span { class: "bottom-nav__label", "Explore" }
                }
                a {
                    class: if matches!(current_route, Route::Notifications) { "bottom-nav__item bottom-nav__item--active" } else { "bottom-nav__item" },
                    href: "#/notifications",
                    {action_icon("bell")}
                    span { class: "bottom-nav__label", "Notifications" }
                }
                a {
                    class: if matches!(current_route, Route::Feeds) { "bottom-nav__item bottom-nav__item--active" } else { "bottom-nav__item" },
                    href: "#/feeds",
                    {action_icon("list")}
                    span { class: "bottom-nav__label", "Feeds" }
                }
                if signed_handle.is_some() {
                    a {
                        class: if matches!(current_route, Route::Compose) { "bottom-nav__item bottom-nav__item--active" } else { "bottom-nav__item" },
                        href: "#/compose",
                        {action_icon("compose")}
                        span { class: "bottom-nav__label", "Post" }
                    }
                }
            }
            main { class: "timeline",
            header { class: "app-header",
                div { class: "app-header__row",
                    {match (session.0)() {
                        Some(signed) => rsx! {
                            AccountMenu {
                                handle: signed.handle.clone(),
                                did: signed.did.clone(),
                                on_sign_out: move |_| sign_out_now(session, saved_feeds),
                            }
                        },
                        None => rsx! {
                            // Narrow screens have no rail, so the column carries the
                            // signed-out pair there; at desktop it is hidden in favour of
                            // the rail's copy (see the media query below).
                            span { class: "badge", "public · logged out" }
                            if !matches!(current_route, Route::SignIn) {
                                a {
                                    class: "session-button",
                                    "data-action": "column-sign-in",
                                    href: "#/sign-in",
                                    "Sign in"
                                }
                            }
                        },
                    }}
                    a {
                        class: "session-button session-button--icon",
                        href: "#/settings",
                        aria_label: "Settings",
                        {action_icon("settings")}
                    }

                }
                // The logo: above the tabs and deliberately not pinned, so it is visible
                // only at the very top and scrolls away while the tab row stays put —
                // the same arrangement as the official app.
                h1 { class: "app-header__brand",
                    "{APP_NAME}"
                    span { class: "app-header__tagline", " - a Dioxus Bluesky Client you can " }
                    a {
                        class: "app-header__fork",
                        href: "{REPO_URL}",
                        target: "_blank",
                        rel: "noreferrer noopener",
                        "data-action": "fork-me",
                        "fork for yourself"
                    }
                    span { class: "app-header__tagline", " to customize" }
                }
            }
                // The tab row is a sibling of the header, not a child: `position: sticky` is
                // bounded by its parent's box, and inside the header it had nothing to
                // stick to.
                nav { class: "feed-tabs",
                    // Narrow screens navigate by tab, so Explore needs one even though
                    // it is a page rather than a feed.
                    button {
                        r#type: "button",
                        class: if matches!(route(), Route::Home) { "feed-tab feed-tab--active" } else { "feed-tab" },
                        onclick: move |_| navigate("#/"),
                        "Explore"
                    }
                    for tab in feed_tabs(&saved_feeds(), (session.0)().is_some()) {
                        {
                            let active = route_key(&route()) == tab.key();
                            rsx! {
                        button {
                            r#type: "button",
                            class: if active { "feed-tab feed-tab--active" } else { "feed-tab" },
                            // Double-click on the tab you are already reading returns to
                            // the top, which is the one gesture the saved offset needs.
                            "data-scroll-top": if active { "true" } else { "" },
                            onclick: {
                                let tab = tab.clone();
                                move |_| {
                                    if tab.needs_session() && (session.0)().is_none() {
                                        navigate("#/sign-in");
                                    }
                                    match &tab {
                                        // Following is session-only, so it is not
                                        // remembered: restoring it logged-out would
                                        // strand the app on an error page.
                                        FeedSource::Following => navigate("#/following"),
                                        FeedSource::Custom { uri, .. } => {
                                            navigate_feed(&format!("#/feed/{}", encode_uri(uri)))
                                        }
                                        FeedSource::Author(actor) => {
                                            navigate(&format!("#/profile/{}", encode_uri(actor)))
                                        }
                                    }
                                }
                            },
                            "{tab.label()}"
                        }
                            }
                        }
                    }
                }

            {match route() {
                Route::Home => rsx! { ExploreView {} },
                Route::Notifications => rsx! { NotificationsView {} },
                Route::Settings => rsx! { SettingsView {} },
                Route::Compose => rsx! { ComposeView {} },
                Route::Hashtag { .. } => rsx! { HashtagView { route: route } },

                // These views read the route signal themselves: a plain prop captured
                // inside `use_resource` never re-runs, so navigating reply-to-reply
                // changed the URL and refetched nothing.
                Route::Post { .. } => rsx! { ThreadView { route: route } },
                Route::PostLikes { .. } => rsx! { EngagementView { route: route } },
                Route::PostReposts { .. } => rsx! { EngagementView { route: route } },
                Route::PostQuotes { .. } => rsx! { EngagementView { route: route } },
                Route::Profile { .. } => rsx! { ProfileView { route: route } },
                Route::SignIn => rsx! { SignInView {} },
                Route::Feeds => rsx! {
                    FeedsView {
                        feeds: saved_feeds(),
                        signed_in: (session.0)().is_some(),
                        discover: discover_name.clone(),
                    }
                },
                _ => rsx! {
                    if let Some(error) = feed_error() {
                        ErrorPanel {
                            message: error,
                            on_retry: move |_| retry_tick.set(retry_tick() + 1),
                        }
                    }
                    if feed_loading() && feed_items().is_empty() {
                        p { class: "note", "Loading public feed…" }
                    }
                    // Hidden trigger for periodic/focus revalidation: a feed you are
                    // sitting on should still notice new posts. Not focusable, since
                    // there is nothing for a keyboard user to do with it.
                    button {
                        r#type: "button",
                        class: "refresh-tick",
                        tabindex: "-1",
                        aria_hidden: "true",
                        onclick: move |_| {
                            // Revalidate in place rather than through the feed effect:
                            // the effect bumps the generation counter and can discard a
                            // first page still in flight.
                            let Some(source) = source_for(&route(), &saved_feeds()) else {
                                return;
                            };
                            let Some((items, _)) = feed_cache.peek().get(&source.key()) else {
                                return;
                            };
                            let known = items.iter().map(|(post, _)| post.uri.clone()).collect();
                            spawn(revalidate_feed(
                                source,
                                (session.0)(),
                                session,
                                known,
                                *feed_generation.peek(),
                                feed_generation,
                                feed_pending,
                            ));
                        },
                        "Refresh feed"
                    }
                    if let Some(page) = feed_pending() {
                        {
                            let label = if page.count == 1 {
                                "Show 1 new post".to_string()
                            } else {
                                format!("Show {} new posts", page.count)
                            };
                            rsx! {
                                button {
                                    r#type: "button",
                                    class: "new-posts",
                                    // Announced politely: it appears mid-reading and
                                    // should never interrupt the current line.
                                    "aria-live": "polite",
                                    onclick: move |_| {
                                        let Some(source) = source_for(&route(), &saved_feeds()) else {
                                            return;
                                        };
                                        // Accepting replaces the list, so the cache has
                                        // to agree or going back would restore the old
                                        // page-one under a fresh scroll offset.
                                        let cursor = page.cursor.clone();
                                        // Accepting replaces the list, so it takes the publish
                                        // path — which is also what resolves the replies it
                                        // carries.
                                        publish_feed_page(
                                            page.items.clone(),
                                            feed_items,
                                            parent_handles,
                                        );
                                        feed_cache.write().insert(
                                            source.key(),
                                            page.items.clone(),
                                            cursor.clone(),
                                        );
                                        feed_cursor.set(cursor);
                                        feed_pending.set(None);
                                        scroll_to_top();
                                    },
                                    "{label}"
                                }
                            }
                        }
                    }
                    for (post, label) in feed_items() {
                        PostCard { post: post, reposted_by: label, origin: feed_origin.clone() }
                    }
                    if feed_cursor().is_some() {
                        // Hidden trigger for endless scroll; also a focusable button
                        // so keyboard users can still page a feed manually.
                        button {
                            r#type: "button",
                            class: "load-more load-more--auto",
                            disabled: feed_loading(),
                            onclick: move |_| {
                                let Some(cursor) = feed_cursor() else {
                                    return;
                                };
                                let Some(source) = source_for(&route(), &saved_feeds()) else {
                                    return;
                                };
                                let current = (session.0)();
                                let generation = *feed_generation.peek();
                                let key = source.key();
                                feed_loading.set(true);
                                spawn(async move {
                                    match fetch_feed(source, current, Some(&cursor), session).await {
                                        Ok(response) => {
                                            if *feed_generation.peek() != generation {
                                                return;
                                            }
                                            let merged = extend_feed_page(
                                                feed_items_of(&response),
                                                feed_items,
                                                parent_handles,
                                            );
                                            feed_cache.write().insert(
                                                key,
                                                merged,
                                                response.cursor.clone(),
                                            );
                                            feed_cursor.set(response.cursor);
                                        }
                                        Err(error) => {
                                            if *feed_generation.peek() == generation {
                                                feed_error.set(Some(error));
                                            }
                                        }
                                    }
                                    feed_loading.set(false);
                                });
                            },
                            "Load more"
                        }
                    }
                    if let Some(note) = feed_end_note(
                        feed_items().len(),
                        feed_cursor().is_some(),
                        feed_loading(),
                    ) {
                        p {
                            class: "note feed-end",
                            role: "status",
                            aria_live: "polite",
                            "{note}"
                        }
                    }
                },
            }}
            }
        }
    }
}

fn main() {
    dioxus::launch(App);
}


/// Failure panel shared by feeds, threads and profiles.
///
/// One place decides what a failure means, so "wrong side of the connection" reads
/// the same everywhere and only the noun changes.
#[component]
fn ErrorPanel(
    message: String,
    #[props(default = Subject::Feed)] subject: Subject,
    on_retry: EventHandler<()>,
) -> Element {
    let failure = FeedFailure::parse(&message);
    let headline = failure.headline_for(subject);
    let hint = failure.hint_for(subject);
    let detail = failure.detail.clone();

    rsx! {
        div { class: "feed-error", role: "alert",
            p { class: "feed-error__headline", "{headline}" }
            p { class: "note", "{hint}" }
            if !detail.is_empty() {
                p { class: "note feed-error__detail", "Detail: {detail}" }
            }
            button {
                r#type: "button",
                class: "load-more",
                onclick: move |_| on_retry.call(()),
                "Try again"
            }
        }
    }
}

/// Thread view: ancestors, the focal post, then replies. Depth and breadth are
/// capped so a 200-node thread does not mount 200 cards on open.
#[component]
fn ThreadView(route: Signal<Route>) -> Element {
    // Read the route *inside* the resource: it is the only reactive value here, so
    // this is what makes tapping a reply (or an ancestor) actually refetch.
    let session_state = use_context::<SessionState>();
    let mut thread = use_resource(move || {
        let uri = match route() {
            Route::Post { uri } => uri,
            _ => String::new(),
        };
        async move { fetch_thread(&uri, session_state).await }
    });
    // The reply box. Kept on the thread view rather than in `ThreadNode`: one box per
    // page, answering the post that was opened.
    let mut reply_text = use_signal(String::new);
    let mut reply_error = use_signal(|| None::<String>);
    let mut reply_busy = use_signal(|| false);

    rsx! {
        section { class: "thread",
            button {
                r#type: "button",
                class: "thread__back",
                onclick: move |_| go_back(),
                "← Back"
            }
            {match &*thread.read() {
                None => rsx! { p { class: "note", "Loading thread…" } },
                Some(Err(err)) => rsx! {
                    ErrorPanel {
                        message: err.clone(),
                        subject: Subject::Post,
                        on_retry: move |_| thread.restart(),
                    }
                },
                Some(Ok(response)) => {
                    let mut ancestors: Vec<PostView> = Vec::new();
                    let mut cursor = match &response.thread {
                        ThreadViewNode::Post(view) => view.parent.as_deref(),
                        _ => None,
                    };
                    while let Some(node) = cursor {
                        match node {
                            ThreadViewNode::Post(view) => {
                                ancestors.push(view.post.clone());
                                cursor = view.parent.as_deref();
                            }
                            _ => break,
                        }
                    }
                    ancestors.reverse();
                    // `parentHeight=2` fetches the two nearest ancestors; if the
                    // oldest one is itself a reply, the chain continues above what we
                    // asked for and the only evidence is that post's own record.
                    let earlier = ancestors
                        .first()
                        .and_then(|post| post.record.reply.as_ref().map(|_| post.uri.clone()));
                    // The post the reader opened: the reply box answers this one.
                    let focal = match &response.thread {
                        ThreadViewNode::Post(view) => Some(view.post.clone()),
                        _ => None,
                    };
                    let root = response.thread.clone();
                    rsx! {
                        if let Some(uri) = earlier {
                            button {
                                r#type: "button",
                                class: "thread-more thread-more--junction",
                                onclick: move |_| {
                                    navigate(&format!("#/post/{}", encode_uri(&uri)));
                                },
                                "Show earlier replies"
                            }
                        }
                        if !ancestors.is_empty() {
                            // Ancestors are context, not the conversation: grouped,
                            // labelled and closed off, instead of four cards that look
                            // like the thread until it abruptly changes shape.
                            div { class: "thread-ancestors",
                                p { class: "thread-ancestors__label", "Earlier in this thread" }
                                for post in ancestors {
                                    // An ancestor can be opened too, which moves focus up.
                                    PostCard { post: post, openable: true }
                                }
                            }
                        }
                        ThreadNode { node: root, depth: 0 }
                        if let Some(post) = focal {
                            {
                                let refs = post.reply_refs();
                                // Owned copies for the attributes: `refs` itself is moved
                                // into the send handler below.
                                let reply_root = refs.root.uri.clone();
                                let reply_parent = refs.parent.uri.clone();
                                let handle = post.author.handle.clone();
                                let signed_in = (session_state.0)().is_some();
                                rsx! {
                                    div { class: "reply-box",
                                        // Exposed for testing: the refs a reply will
                                        // carry. Replying to a reply must keep the
                                        // thread root, and that is invisible otherwise.
                                        "data-reply-root": "{reply_root}",
                                        "data-reply-parent": "{reply_parent}",
                                        if !signed_in {
                                            p { class: "reply-box__note", "Sign in to reply to @{handle}." }
                                        } else {
                                            textarea {
                                                class: "reply-box__input",
                                                rows: "3",
                                                "aria-label": "Reply text",
                                                placeholder: "Reply to @{handle}",
                                                value: "{reply_text}",
                                                oninput: move |event| reply_text.set(event.value()),
                                            }
                                            div { class: "reply-box__row",
                                                span { class: "reply-box__count",
                                                    "{reply_text().chars().count()}/300"
                                                }
                                                button {
                                                    r#type: "button",
                                                    class: "reply-box__send",
                                                    disabled: reply_busy()
                                                        || reply_text().trim().is_empty()
                                                        || reply_text().chars().count() > 300,
                                                    onclick: move |_| {
                                                        let Some(signed) = (session_state.0)() else {
                                                            return;
                                                        };
                                                        let text = reply_text().trim().to_string();
                                                        if text.is_empty() {
                                                            return;
                                                        }
                                                        let refs = refs.clone();
                                                        let mut thread = thread;
                                                        reply_busy.set(true);
                                                        reply_error.set(None);
                                                        spawn(async move {
                                                            match create_reply(&signed, &text, &refs).await {
                                                                Ok(_) => {
                                                                    reply_text.set(String::new());
                                                                    thread.restart();
                                                                }
                                                                Err(err) => reply_error.set(Some(err)),
                                                            }
                                                            reply_busy.set(false);
                                                        });
                                                    },
                                                    if reply_busy() { "Replying…" } else { "Reply" }
                                                }
                                            }
                                            if let Some(err) = reply_error() {
                                                p { class: "reply-box__error", "{err}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }}
        }
    }
}

/// The share arrow at the end of the action row: copy a link.
///
/// Two links on purpose. Ours carries the at-uri and a `#`, so it only opens this app;
/// the canonical `bsky.app/profile/<handle>/post/<rkey>` is the form everyone else can
/// read and unfurl. Offering both is honest about the difference instead of handing out
/// one link that half the world cannot open.
#[component]
fn ShareMenu(handle: String, uri: String) -> Element {
    let mut open = use_signal(|| false);
    use_menu_dismiss(open);
    let mut copied = use_signal(|| None::<String>);
    let internal = post_href(&uri, "");
    let external = bsky_permalink(&handle, &uri);

    rsx! {
        div { class: "post__menu-wrap", "data-menu-root": "true",
            button {
                r#type: "button",
                class: "post__menu-toggle post__menu-toggle--share",
                "aria-label": "Share",
                "aria-haspopup": "true",
                "aria-expanded": if open() { "true" } else { "false" },
                "data-action": "share-menu",
                onclick: move |event| {
                    event.stop_propagation();
                    let next = !open();
                    open.set(next);
                    if !next {
                        copied.set(None);
                    }
                },
                {action_icon("share")}
            }
            if open() {
                div { class: "post__menu", role: "menu",
                    button {
                        r#type: "button",
                        class: "post__menu-item",
                        role: "menuitem",
                        "data-action": "menu-copy-internal",
                        title: "{internal}",
                        onclick: {
                            let link = internal.clone();
                            move |event| {
                                event.stop_propagation();
                                copy_post_link(link.clone());
                                copied.set(Some("app".to_string()));
                            }
                        },
                        "Copy link to this app"
                    }
                    button {
                        r#type: "button",
                        class: "post__menu-item",
                        role: "menuitem",
                        "data-action": "menu-copy-bsky",
                        title: "{external}",
                        onclick: {
                            let link = external.clone();
                            move |event| {
                                event.stop_propagation();
                                copy_link(link.clone());
                                copied.set(Some("bsky".to_string()));
                            }
                        },
                        "Copy bsky.app link"
                    }
                    if let Some(kind) = copied() {
                        p { class: "post__menu-note",
                            if kind == "app" { "Copied this app's link." } else { "Copied the bsky.app link." }
                        }
                    }
                }
            }
        }
    }
}

/// Per-post "⋯" menu: mute and block.
///
/// Two kinds of link, on purpose. Our own URL carries the at-uri and a `#`, so it can
/// never be a bsky.app URL; the canonical `bsky.app/profile/<handle>/post/<rkey>` is
/// what everyone else reads and unfurls. Offering both is honest about the difference
/// instead of handing out one link that only half the world can open.
///
/// The card's `PostView` carries no viewer state for its author, so the menu asks for
/// the profile when it opens — one request, and only for a signed-in reader. That is
/// what keeps "Unmute" truthful and gives the unblock its record uri; guessing would
/// create a second block record instead of deleting the first.
#[component]
fn PostMenu(handle: String, did: String, uri: String, text: String) -> Element {
    let session_state = use_context::<SessionState>();
    let mut deleted = use_context::<DeletedPosts>();
    let mut open = use_signal(|| false);
    let mut viewer = use_signal(|| None::<ProfileViewerState>);
    let mut confirm_delete = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut confirm = use_signal(|| false);
    let mut copied = use_signal(|| false);
    use_menu_dismiss(open);

    let signed_in = (session_state.0)().is_some();
    // Two rules, both tested in core rather than re-derived here: you can only delete your own
    // posts, and mute/block are for other accounts — never yourself.
    let viewer_did = (session_state.0)().map(|session| session.did);
    let own_post = alt_first_core::is_own_post(viewer_did.as_deref(), &did);
    let can_moderate = alt_first_core::can_moderate_author(viewer_did.as_deref(), &did);
    let author_did = did;
    let muted = viewer().as_ref().map(|state| state.muted).unwrap_or(false);
    let blocking = viewer().as_ref().and_then(|state| state.blocking.clone());
    let blocked = blocking.is_some();
    let internal = post_href(&uri, "");
    let external = bsky_permalink(&handle, &uri);

    rsx! {
        div { class: "post__menu-wrap", "data-menu-root": "true",
            button {
                r#type: "button",
                class: "post__menu-toggle",
                "aria-label": "More actions",
                "aria-haspopup": "true",
                "aria-expanded": if open() { "true" } else { "false" },
                "data-action": "post-menu",
                onclick: move |event| {
                    event.stop_propagation();
                    let next = !open();
                    open.set(next);
                    confirm.set(false);
                    confirm_delete.set(false);
                    copied.set(false);
                    error.set(None);
                    if !next || !signed_in {
                        return;
                    }
                    if (session_state.0)().is_none() {
                        return;
                    }
                    let handle = handle.clone();
                    spawn(async move {
                        if let Ok(profile) = fetch_profile(&handle, session_state).await {
                            viewer.set(profile.viewer);
                        }
                    });
                },
                "⋯"
            }
            if open() {
                div { class: "post__menu", role: "menu",
                    button {
                        r#type: "button",
                        class: "post__menu-item",
                        role: "menuitem",
                        "data-action": "menu-copy-text",
                        disabled: text.trim().is_empty(),
                        onclick: {
                            let text = text.clone();
                            move |event| {
                                event.stop_propagation();
                                copy_link(text.clone());
                                copied.set(true);
                            }
                        },
                        "Copy post text"
                    }
                    if copied() {
                        p { class: "post__menu-note", "Copied post text." }
                    }
                    if signed_in {
                        {
                            // One clone per handler: each closure owns what it uses, so
                            // a single binding cannot be moved into all three.
                            let mute_did = author_did.clone();
                            let block_did = author_did.clone();
                            let block_existing = blocking.clone();
                            let go_did = author_did.clone();
                            let go_existing = blocking.clone();
                            rsx! {
                        // Mute and block are things you do to *other* accounts; offering them
                        // on your own post is a menu item that cannot mean anything.
                        if can_moderate {
                        div { class: "post__menu-sep" }
                        button {
                            r#type: "button",
                            class: "post__menu-item",
                            role: "menuitem",
                            "data-action": "menu-mute",
                            disabled: busy(),
                            onclick: move |event| {
                                event.stop_propagation();
                                let Some(signed) = (session_state.0)() else {
                                    return;
                                };
                                let did = mute_did.clone();
                                let next = !muted;
                                busy.set(true);
                                error.set(None);
                                spawn(async move {
                                    match set_muted(&signed, &did, next).await {
                                        Ok(()) => {
                                            let mut state = viewer().unwrap_or_default();
                                            state.muted = next;
                                            viewer.set(Some(state));
                                        }
                                        Err(err) => error.set(Some(err)),
                                    }
                                    busy.set(false);
                                });
                            },
                            if muted { "Unmute @{handle}" } else { "Mute @{handle}" }
                        }
                        button {
                            r#type: "button",
                            class: "post__menu-item post__menu-item--danger",
                            role: "menuitem",
                            "data-action": "menu-block",
                            disabled: busy(),
                            onclick: move |event| {
                                event.stop_propagation();
                                if !blocked {
                                    confirm.set(true);
                                    return;
                                }
                                let Some(signed) = (session_state.0)() else {
                                    return;
                                };
                                let did = block_did.clone();
                                let existing = block_existing.clone();
                                busy.set(true);
                                error.set(None);
                                spawn(async move {
                                    match set_blocked(&signed, &did, existing.as_deref(), false).await {
                                        Ok(_) => {
                                            let mut state = viewer().unwrap_or_default();
                                            state.blocking = None;
                                            viewer.set(Some(state));
                                        }
                                        Err(err) => error.set(Some(err)),
                                    }
                                    busy.set(false);
                                });
                            },
                            if blocked { "Unblock @{handle}" } else { "Block @{handle}" }
                        }
                        }
                        if own_post {
                            div { class: "post__menu-sep" }
                            button {
                                r#type: "button",
                                class: "post__menu-item post__menu-item--danger",
                                role: "menuitem",
                                "data-action": "menu-delete-post",
                                disabled: busy(),
                                onclick: move |event| {
                                    event.stop_propagation();
                                    confirm_delete.set(true);
                                    error.set(None);
                                },
                                "Delete post"
                            }
                            if confirm_delete() {
                                div { class: "post__menu-confirm",
                                    p { class: "post__menu-confirm__title", "Delete post?" }
                                    p { class: "post__menu-confirm__body",
                                        "This removes the post from your account. It cannot be undone, and any replies to it will show the post as unavailable."
                                    }
                                    div { class: "post__menu-confirm__row",
                                        button {
                                            r#type: "button",
                                            class: "post__menu-item",
                                            "data-action": "menu-delete-cancel",
                                            disabled: busy(),
                                            onclick: move |event| {
                                                event.stop_propagation();
                                                confirm_delete.set(false);
                                            },
                                            "Cancel"
                                        }
                                        button {
                                            r#type: "button",
                                            class: "post__menu-confirm__go",
                                            "data-action": "menu-delete-confirm",
                                            disabled: busy(),
                                            onclick: move |event| {
                                                event.stop_propagation();
                                                let Some(signed) = (session_state.0)() else {
                                                    return;
                                                };
                                                let uri = uri.clone();
                                                let gone = uri.clone();
                                                confirm_delete.set(false);
                                                busy.set(true);
                                                error.set(None);
                                                spawn(async move {
                                                    match delete_post(&signed, &uri).await {
                                                        Ok(()) => {
                                                            // Local-first: hide it everywhere rather
                                                            // than refetching every cached page.
                                                            deleted.0.write().insert(gone);
                                                        }
                                                        Err(err) => error.set(Some(err)),
                                                    }
                                                    busy.set(false);
                                                });
                                            },
                                            if busy() { "Deleting…" } else { "Delete" }
                                        }
                                    }
                                }
                            }
                        }
                        if confirm() && can_moderate {
                            div { class: "post__menu-confirm",
                                p { class: "post__menu-confirm__title", "Block account?" }
                                p { class: "post__menu-confirm__body",
                                    "Blocked accounts cannot reply in your threads, mention you, or otherwise interact with you."
                                }
                                div { class: "post__menu-confirm__row",
                                    button {
                                        r#type: "button",
                                        class: "post__menu-item",
                                        "data-action": "menu-block-cancel",
                                        disabled: busy(),
                                        onclick: move |event| {
                                            event.stop_propagation();
                                            confirm.set(false);
                                        },
                                        "Cancel"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "post__menu-confirm__go",
                                        "data-action": "menu-block-confirm",
                                        disabled: busy(),
                                        onclick: move |event| {
                                            event.stop_propagation();
                                            let Some(signed) = (session_state.0)() else {
                                                return;
                                            };
                                            let did = go_did.clone();
                                            let existing = go_existing.clone();
                                            confirm.set(false);
                                            busy.set(true);
                                            error.set(None);
                                            spawn(async move {
                                                match set_blocked(&signed, &did, existing.as_deref(), true).await {
                                                    Ok(uri) => {
                                                        let mut state = viewer().unwrap_or_default();
                                                        state.blocking = uri;
                                                        viewer.set(Some(state));
                                                    }
                                                    Err(err) => error.set(Some(err)),
                                                }
                                                busy.set(false);
                                            });
                                        },
                                        if busy() { "Blocking…" } else { "Block" }
                                    }
                                }
                            }
                        }
                        if let Some(err) = error() {
                            p { class: "post__menu-error", "{err}" }
                        }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The canonical bsky.app permalink: handle and record key, not the at-uri.
fn bsky_permalink(handle: &str, post_uri: &str) -> String {
    let rkey = post_uri.rsplit('/').next().unwrap_or_default();
    format!("https://bsky.app/profile/{handle}/post/{rkey}")
}

/// Copy text in the browser.
///
/// `navigator.clipboard` needs a secure origin; the textarea fallback keeps a plain
/// http:// deployment usable, where the async API is simply absent.
/// Copy a link, resolving app-internal `#/...` hrefs against the page first.
///
/// The share menu used to copy the raw href, which is a bare fragment: pasted anywhere, a
/// reader gets `#/post/at://…` and no way to open it. `new URL(href, location.href)` turns it
/// into the real address, including the `/repo/` prefix a project page is served from.
fn copy_post_link(href: String) {
    spawn(async move {
        let mut eval = document::eval(&format!(
            r#"(() => {{
                try {{ dioxus.send(new URL({href}, location.href).href); }}
                catch (error) {{ dioxus.send({href}); }}
            }})()"#,
            href = serde_json::to_string(&href).unwrap()
        ));
        // Fall back to what was given rather than losing the copy.
        let absolute = eval.recv::<String>().await.unwrap_or(href);
        copy_link(absolute);
    });
}

fn copy_link(text: String) {
    document::eval(&format!(
        r#"(() => {{
            const write = {text};
            if (navigator.clipboard && navigator.clipboard.writeText) {{
                navigator.clipboard.writeText(write).catch(() => {{}});
                return;
            }}
            const area = document.createElement('textarea');
            area.value = write;
            area.setAttribute('readonly', '');
            area.style.position = 'fixed';
            area.style.top = '-1000px';
            document.body.appendChild(area);
            area.select();
            try {{ document.execCommand('copy'); }} catch (error) {{}}
            document.body.removeChild(area);
        }})()"#,
        text = serde_json::to_string(&text).unwrap()
    ));
}

/// The server refused to rotate the token pair: the grant is gone (used elsewhere, revoked,
/// or expired). Retrying it forever is what turned one dead token into a feed full of 401s, so
/// drop the session and say so.
fn forget_session(mut session: SessionState, reason: &str) {
    session.0.set(None);
    persist_session(None, false);
    // The caller logs the reason: it already has it in hand, and two identical lines in a
    // console read as two failures.
    let _ = reason;
}

/// True when a refresh failure means "this grant will never work again", as opposed to a
/// network blip that a later attempt could still recover from.
fn is_dead_grant(error: &str) -> bool {
    error.contains("invalid_grant") || error.contains("invalid_token") || error.contains("invalid_client")
}

/// Clear the session, the cached feeds and the remembered tab, then go home.
///
/// Shared by the two account controls (rail and narrow header), so the two cannot
/// disagree about what signing out cleans up.
fn sign_out_now(mut session: SessionState, mut saved_feeds: Signal<Vec<(String, String)>>) {
    let signed_out = (session.0)();
    session.0.set(None);
    saved_feeds.set(Vec::new());
    persist_session(None, false);
    // Forget the feed too: it may have been Following.
    remember_feed("#/");
    navigate("#/");

    // An OAuth session is revoked at the authorisation server as well as dropped here:
    // clearing local state leaves a refresh token that still works elsewhere.
    if let Some(oauth) = signed_out.as_ref().and_then(|session| session.oauth.clone()) {
        let refresh_token = signed_out
            .as_ref()
            .map(|session| session.refresh_jwt.clone())
            .unwrap_or_default();
        spawn(async move { oauth_flow::revoke(&oauth, &refresh_token).await });
    }
}

/// The signed-in account, top-left: avatar plus handle, opening a menu.
///
/// Officially the avatar *is* the account control, and signing out lives inside it
/// rather than on a button of its own — a destructive-ish action with no confirmation
/// next to the tab row was one mis-tap away. The avatar is fetched from the viewer's own
/// profile: the session carries a handle and a DID, not a picture.
#[component]
fn AccountMenu(handle: String, did: String, on_sign_out: EventHandler<()>) -> Element {
    let mut open = use_signal(|| false);
    let mut avatar = use_signal(|| None::<String>);
    use_menu_dismiss(open);

    let fetch_handle = handle.clone();
    use_effect(move || {
        let handle = fetch_handle.clone();
        spawn(async move {
            // Public API on purpose: this reads one field (the avatar) and needs no viewer
            // state. It was also the only authenticated PDS call on Explore, so it paid the
            // DPoP nonce handshake by itself and put a `401` in the console on every load of
            // the home page — for an image URL the public AppView hands out anyway.
            if let Ok(profile) = fetch_public_profile(&handle).await {
                avatar.set(profile.avatar);
            }
        });
    });

    let initial = handle.chars().next().unwrap_or('?').to_uppercase().to_string();
    let profile_href = format!("#/profile/{}", encode_uri(&handle));

    rsx! {
        div { class: "account", "data-menu-root": "true",
            button {
                r#type: "button",
                class: "account__button",
                "aria-haspopup": "true",
                "aria-expanded": if open() { "true" } else { "false" },
                "aria-label": "Account menu for @{handle}",
                "data-action": "account-menu",
                onclick: move |event| {
                    event.stop_propagation();
                    let next = !open();
                    open.set(next);
                },
                {match avatar() {
                    Some(src) => rsx! { img { class: "account__avatar", src: "{src}", alt: "" } },
                    None => rsx! { span { class: "account__initial", "{initial}" } },
                }}
                span { class: "account__handle", "@{handle}" }
            }
            if open() {
                div { class: "post__menu account__menu", role: "menu",
                    a {
                        class: "post__menu-item",
                        role: "menuitem",
                        href: "{profile_href}",
                        "data-action": "account-profile",
                        onclick: move |_| open.set(false),
                        "View profile"
                    }
                    a {
                        class: "post__menu-item",
                        role: "menuitem",
                        href: "#/settings",
                        "data-action": "account-settings",
                        onclick: move |_| open.set(false),
                        "Settings"
                    }
                    div { class: "post__menu-sep" }
                    button {
                        r#type: "button",
                        class: "post__menu-item post__menu-item--danger",
                        role: "menuitem",
                        "data-action": "account-sign-out",
                        onclick: move |event| {
                            event.stop_propagation();
                            open.set(false);
                            on_sign_out.call(());
                        },
                        "Sign out"
                    }
                    p { class: "post__menu-note", "@{did}" }
                }
            }
        }
    }
}

/// One node in a thread.
///
/// A component rather than a plain function because a bushy node has to remember
/// whether its folded replies are open: `<details>` mounts its children immediately,
/// so a root with 198 replies put 249 cards in the DOM at once and took seconds to
/// paint on a phone — the fold looked broken because nothing had rendered yet.
#[component]
fn ThreadNode(
    node: ThreadViewNode,
    depth: usize,
    /// Who this node answers. The AppView only fills `parent` on the requested
    /// post, so replies inherit this from the recursion instead.
    #[props(default)] parent: Option<String>,
) -> Element {
    let mut expanded = use_signal(|| false);

    match node {
        ThreadViewNode::Post(view) => {
            let all_replies = view.replies.clone().unwrap_or_default();
            // No depth cap on *nodes*: a conversation should read like a conversation.
            // The indent is capped instead, because past a few levels the margin has
            // eaten the card and the nesting stops being legible anyway.
            const INDENT_CAP: usize = 3;
            const FOLD_AFTER: usize = 20;
            let split = all_replies.len().min(FOLD_AFTER);
            let (shown, folded) = all_replies.split_at(split);
            let shown: Vec<ThreadViewNode> = shown.to_vec();
            let folded: Vec<ThreadViewNode> = folded.to_vec();
            let folded_count = folded.len();
            // The depth cut is silent in the payload: `replyCount` larger than the
            // children returned is the only evidence a branch goes on. Offer to
            // continue it instead of ending the thread mid-air.
            let missing = unfetched_replies(view.post.reply_count, all_replies.len());
            let junction = continue_thread_label(missing);
            let junction_uri = view.post.uri.clone();

            // The opened post is marked, and it does not draw a rail: a rail there
            // read as "nested under something above", which was wrong.
            let class = if depth == 0 {
                "thread-node thread-node--focal"
            } else {
                "thread-node thread-node--reply"
            };
            let style = format!("--depth: {};", depth.min(INDENT_CAP));
            // Past the cap every level shares an indent, so who a reply answers is no
            // longer visible from the geometry — say it instead.
            let parent_handle = if depth > INDENT_CAP { parent.clone() } else { None };

            rsx! {
                div { class: "{class}", style: "{style}",
                    if let Some(handle) = parent_handle {
                        p { class: "thread-node__parent", "↳ replying to @{handle}" }
                    }
                    // Every node but the one already open can be opened: that is how a
                    // reply becomes the focal post (and gets the counts line).
                    PostCard {
                        post: view.post.clone(),
                        openable: depth != 0,
                        focal: depth == 0,
                        origin: String::new(),
                    }
                    for reply in shown {
                        ThreadNode { node: reply, depth: depth + 1, parent: Some(view.post.author.handle.clone()) }
                    }
                    if folded_count > 0 {
                        button {
                            r#type: "button",
                            class: "thread-more",
                            aria_expanded: if expanded() { "true" } else { "false" },
                            onclick: move |_| expanded.set(!expanded()),
                            if expanded() {
                                if folded_count == 1 {
                                    "Hide 1 reply"
                                } else {
                                    "Hide {folded_count} replies"
                                }
                            } else if folded_count == 1 {
                                "Show 1 reply"
                            } else {
                                "Show {folded_count} replies"
                            }
                        }
                        // Mounted only once opened: this is the whole point.
                        if expanded() {
                            for reply in folded {
                                ThreadNode { node: reply, depth: depth + 1, parent: Some(view.post.author.handle.clone()) }
                            }
                        }
                    }
                    if !junction.is_empty() {
                        // Opening that node as the focal post restarts the thread
                        // there with a fresh depth budget, which is cheaper and more
                        // honest than silently merging a second fetch into the tree.
                        button {
                            r#type: "button",
                            class: "thread-more thread-more--junction",
                            onclick: move |_| {
                                navigate(&format!("#/post/{}", encode_uri(&junction_uri)));
                            },
                            "{junction}"
                        }
                    }
                }
            }
        }
        ThreadViewNode::NotFound { .. } => rsx! {
            p { class: "thread-missing", "Reply unavailable (deleted)" }
        },
        ThreadViewNode::Blocked { .. } => rsx! {
            p { class: "thread-missing", "Reply hidden (blocked)" }
        },
        ThreadViewNode::Unknown => rsx! {},
    }
}


/// Hand a page of cards to the view and make sure their reply parents are known.
///
/// Both the network path and the cache path go through here. The cache path used to
/// return early, so a feed served from cache — a revisit, or a tab restored cold —
/// rendered replies as if they were top-level posts until a fresh page happened to land.
/// Keeping the publish and the parent lookup in one place is what stops the next list
/// from silently losing the line.
fn publish_feed_page(
    items: Vec<(PostView, Option<String>)>,
    mut feed_items: Signal<Vec<(PostView, Option<String>)>>,
    parents: Signal<std::collections::HashMap<String, Option<String>>>,
) {
    let posts: Vec<PostView> = items.iter().map(|(post, _)| post.clone()).collect();
    feed_items.set(items);
    spawn(async move { remember_parents(&posts, parents).await });
}

/// Append a page, and remember who its replies answer.
///
/// The scroll trigger and the "new posts" pill both used to set `feed_items` themselves, so
/// page two onward drew replies as top-level posts — and stayed that way until a fresh page
/// replaced the list. That is the whole bug: the parents of page one were resolved, the parents
/// of every page after it never were.
///
/// Returns the merged list, because the cache has to agree with what was published.
fn extend_feed_page(
    page: Vec<(PostView, Option<String>)>,
    mut feed_items: Signal<Vec<(PostView, Option<String>)>>,
    parents: Signal<std::collections::HashMap<String, Option<String>>>,
) -> Vec<(PostView, Option<String>)> {
    let posts: Vec<PostView> = page.iter().map(|(post, _)| post.clone()).collect();
    let mut items = feed_items();
    items.extend(page);
    feed_items.set(items.clone());
    spawn(async move { remember_parents(&posts, parents).await });
    items
}

/// Notifications need a session: the list is per-account, and the AppView refuses
/// without one.
/// Remember who each reply in a page answers.
///
/// `record.reply.parent` is only an at-uri, and the feed's own `PostView`s do not carry
/// the parent's author — `getPosts` does, for up to 25 uris in one call. Without this a
/// reply in a feed is indistinguishable from a top-level post until you read it.
async fn remember_parents(posts: &[PostView], mut parents: Signal<std::collections::HashMap<String, Option<String>>>) {
    let mut missing: Vec<String> = Vec::new();
    for post in posts {
        if let Some(reply) = &post.record.reply {
            let uri = reply.parent.uri.clone();
            if !uri.is_empty()
                && !parents.peek().contains_key(&uri)
                && !missing.iter().any(|known| known == &uri)
            {
                missing.push(uri);
            }
        }
    }
    missing.sort();
    missing.dedup();
    // getPosts takes 25 uris per call, and a feed page holds 50 posts — truncating
    // here silently left every reply past the 25th parent unlabelled, which is how a
    // card could sit in a feed looking like a top-level post.
    for chunk in missing.chunks(25) {
        let query: Vec<String> = chunk
            .iter()
            .map(|uri| format!("uris={}", encode_uri(uri)))
            .collect();
        let request = request_json::<PostsResponse>(
            "GET",
            &format!("{PUBLIC_API}/xrpc/app.bsky.feed.getPosts?{}", query.join("&")),
            None,
            None,
        )
        .await;
        let Ok(response) = request else {
            // A transport failure must not be cached as "parent unavailable": the next
            // page load should try again.
            continue;
        };
        let mut map = parents.write();
        let mut resolved: Vec<String> = Vec::new();
        for parent in response.posts {
            resolved.push(parent.uri.clone());
            map.insert(parent.uri.clone(), Some(parent.author.handle.clone()));
        }
        // A parent that the AppView will not return is deleted, taken down or hidden.
        // Recording that as `None` keeps the card honest ("Replying to an unavailable
        // post") instead of letting it read as a top-level post.
        for uri in chunk {
            if !resolved.iter().any(|found| found == uri) {
                map.insert(uri.clone(), None);
            }
        }
    }
}

async fn fetch_notifications(state: SessionState) -> Result<NotificationsResponse, String> {
    viewer_read(
        "/xrpc/app.bsky.notification.listNotifications?limit=50",
        state,
    )
    .await
}

/// Puts a sign-in failure where a developer can read the whole thing. The page shows the
/// message too; the console keeps the server's own wording without crowding the UI.
fn log_to_console(message: &str) {
    document::eval(&format!(
        "console.error('sign-in:', {});",
        serde_json::to_string(message).unwrap_or_else(|_| "'unprintable'".to_string())
    ));
}

/// The way in, on its own page: two self-contained sections, one per path.
///
/// Each carries its own handle, because they are genuinely alternatives — a reader picks one
/// and finishes it, rather than filling in a shared field and hoping it applies to both.
#[component]
fn SignInView() -> Element {
    let mut session = use_context::<SessionState>();
    let mut login_error = use_context::<LoginState>().0;
    let mut feeds = use_context::<FeedsState>().0;

    let mut oauth_handle = use_signal(String::new);
    // Separate from the app-password toggle, and on by default: an OAuth session stores a
    // DPoP-bound refresh token, useless without the key in IndexedDB, so keeping it is the
    // same risk class as any ordinary site login. An app password is a bearer credential for
    // the whole account, which is why that one stays opt-in.
    let mut oauth_remember = use_signal(|| true);
    let mut password_handle = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut remember = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let oauth_available = sign_in_mode().offers_oauth() && OAUTH_IMPLEMENTED;

    if let Some(signed) = (session.0)() {
        return rsx! {
            section { class: "signin",
                h2 { class: "page-title", "Sign in" }
                p { class: "note", "Signed in as @{signed.handle}." }
                a { class: "session-button", href: "#/", "Go to the feed" }
            }
        };
    }

    rsx! {
        section { class: "signin",
            h2 { class: "page-title", "Sign in" }
            p { class: "note", "Sign in to your Bluesky account." }

            if oauth_available {
                div { class: "signin__option",
                    h3 { class: "signin__option-title", "Sign in with Bluesky OAuth" }
                    div { class: "signin__field",
                        label { class: "signin__label", r#for: "oauth-handle", "Handle" }
                        input {
                            id: "oauth-handle",
                            class: "actor-input",
                            "aria-label": "Handle for Bluesky sign-in",
                            placeholder: "you.bsky.social",
                            autocomplete: "username",
                            value: "{oauth_handle}",
                            oninput: move |event| oauth_handle.set(event.value()),
                        }
                    }
                    p { class: "note signin__explain",
                        "You approve the request on Bluesky and come back signed in. Every request is then signed by a key that stays in this browser."
                    }
                    button {
                        r#type: "button",
                        class: "session-button signin__primary",
                        "data-action": "oauth-sign-in",
                        disabled: busy(),
                        onclick: move |_| {
                            let identifier = oauth_handle();
                            let remember_me = oauth_remember();
                            login_error.set(None);
                            busy.set(true);
                            spawn(async move {
                                // On success this navigates away to the consent screen; a
                                // failure is shown here because the redirect never happens.
                                if let Err(err) =
                                    oauth_flow::start(&identifier, remember_me).await
                                {
                                    login_error.set(Some(err));
                                    busy.set(false);
                                }
                            });
                        },
                        if busy() { "Opening Bluesky…" } else { "Sign in with Bluesky OAuth" }
                    }
                    label { class: "toggle login-remember",
                        input {
                            r#type: "checkbox",
                            "data-action": "oauth-remember",
                            checked: oauth_remember(),
                            onchange: move |_| oauth_remember.set(!oauth_remember()),
                        }
                        span { "Keep me signed in on this device" }
                    }
                }
                div { class: "signin__or", span { "or" } }
            }

            form {
                class: "login-form signin__option",
                onsubmit: move |event| {
                    event.prevent_default();
                    let identifier = password_handle();
                    let secret = password();
                    login_error.set(None);
                    spawn(async move {
                        match login(&identifier, &secret, remember()).await {
                            Ok(signed) => {
                                session.0.set(Some(signed.clone()));
                                persist_session(Some(&signed), signed.remembered);
                                password.set(String::new());
                                if let Ok(list) = fetch_saved_feeds(session).await {
                                    feeds.set(list);
                                }
                                navigate("#/");
                            }
                            Err(err) => login_error.set(Some(err)),
                        }
                    });
                },
                h3 { class: "signin__option-title", "Sign in with an app password" }
                div { class: "signin__field",
                    label { class: "signin__label", r#for: "password-handle", "Handle" }
                    input {
                        id: "password-handle",
                        class: "actor-input",
                        "aria-label": "Handle",
                        placeholder: "you.bsky.social",
                        autocomplete: "username",
                        value: "{password_handle}",
                        oninput: move |event| password_handle.set(event.value()),
                    }
                }
                div { class: "signin__field",
                    label { class: "signin__label", r#for: "password-secret", "App password" }
                    input {
                        id: "password-secret",
                        class: "actor-input",
                        r#type: "password",
                        "aria-label": "App password",
                        placeholder: "xxxx-xxxx-xxxx-xxxx",
                        autocomplete: "current-password",
                        value: "{password}",
                        oninput: move |event| password.set(event.value()),
                    }
                }
                button { r#type: "submit", "Sign in with app password" }
                label { class: "toggle login-remember",
                    input {
                        r#type: "checkbox",
                        checked: remember(),
                        onchange: move |_| remember.set(!remember()),
                    }
                    span { "Remember me on this device" }
                }
            }

            if let Some(err) = login_error() {
                p { class: "login-error", "{err}" }
            }
            p { class: "note", "{sign_in_note()}" }
        }
    }
}

/// Notifications: who liked, reposted, replied, quoted, mentioned or followed.
#[component]
fn NotificationsView() -> Element {
    let session_state = use_context::<SessionState>();

    if (session_state.0)().is_none() {
        return rsx! {
            section { class: "notifications",
                h2 { class: "page-title", "Notifications" }
                p { class: "note", "Notifications are per-account." }
                button {
                    r#type: "button",
                    class: "feed-tab feed-tab--active",
                    onclick: move |_| navigate("#/sign-in"),
                    "Sign in"
                }
            }
        };
    }

    let mut list = use_resource(move || async move { fetch_notifications(session_state).await });

    rsx! {
        section { class: "notifications",
            h2 { class: "page-title", "Notifications" }
            {match &*list.read() {
                None => rsx! { p { class: "note", "Loading…" } },
                Some(Err(error)) => rsx! {
                    ErrorPanel {
                        message: error.clone(),
                        subject: Subject::Feed,
                        on_retry: move |_| list.restart(),
                    }
                },
                Some(Ok(response)) => {
                    let items = response.notifications.clone();
                    if items.is_empty() {
                        rsx! { p { class: "note", "Nothing here yet." } }
                    } else {
                        rsx! {
                            ul { class: "actor-list",
                                for item in items {
                                    li { class: "actor-list__item",
                                        a {
                                            class: "actor actor--notification",
                                            href: {
                                                match &item.reason_subject {
                                                    Some(uri) => format!("#/post/{}", encode_uri(uri)),
                                                    None => format!("#/profile/{}", encode_uri(&item.author.handle)),
                                                }
                                            },
                                            {match item.author.avatar.clone() {
                                                Some(src) => rsx! {
                                                    img { class: "avatar", src: "{src}", alt: "", loading: "lazy" }
                                                },
                                                None => rsx! { div { class: "avatar avatar--empty" } },
                                            }}
                                            span { class: "actor__name",
                                                {item.author.display_name.clone().filter(|name| !name.trim().is_empty()).unwrap_or_else(|| item.author.handle.clone())}
                                            }
                                            span { class: "actor__handle",
                                                "{notification_verb(&item.reason)}"
                                                if !item.is_read {
                                                    span { class: "notification-dot", "aria-label": "unread", " •" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }}
        }
    }
}

/// Kick off a search. A free function rather than a closure so the form and the
/// retry button can both call it without fighting over ownership.
fn start_search(
    term: String,
    mut results: Signal<Vec<PostView>>,
    mut searched: Signal<Option<String>>,
    mut search_error: Signal<Option<String>>,
    mut searching: Signal<bool>,
    parents: Signal<std::collections::HashMap<String, Option<String>>>,
) {
    let trimmed = term.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    searching.set(true);
    search_error.set(None);
    searched.set(Some(trimmed.clone()));
    spawn(async move {
        match search_posts(&trimmed).await {
            Ok(found) => {
                // Search results are list cards too, so they get the same
                // "Replying to" line as a feed.
                let posts = found.posts.clone();
                results.set(found.posts);
                remember_parents(&posts, parents).await;
            }
            Err(error) => {
                results.set(Vec::new());
                search_error.set(Some(error));
            }
        }
        searching.set(false);
    });
}

/// Everything posted with one hashtag, as a page of its own.
///
/// `#tag` used to open `bsky.app/hashtag/<tag>` in a new tab — the same "internal-looking link
/// leaves the client" problem mentions had. The search behind it is the one Explore already
/// uses, so this is a destination without a new backend.
#[component]
fn HashtagView(route: Signal<Route>) -> Element {
    let parent_handles = use_context::<ParentHandles>().0;
    let mut results = use_resource(move || {
        let tag = match route() {
            Route::Hashtag { tag } => tag,
            _ => String::new(),
        };
        async move {
            let found = search_posts(&format!("#{tag}")).await;
            // Results are list cards too, so they get the "Replying to" line like a feed.
            if let Ok(response) = &found {
                remember_parents(&response.posts, parent_handles).await;
            }
            found
        }
    });

    let tag = match route() {
        Route::Hashtag { tag } => tag,
        _ => String::new(),
    };
    let origin = format!("#/tag/{}", encode_uri(&tag));

    rsx! {
        section { class: "hashtag",
            button {
                r#type: "button",
                class: "thread__back",
                onclick: move |_| go_back(),
                "← Back"
            }
            h2 { class: "page-title", "#{tag}" }
            {match &*results.read() {
                None => rsx! { p { class: "note", "Searching…" } },
                Some(Err(error)) => rsx! {
                    ErrorPanel {
                        message: error.clone(),
                        subject: Subject::Feed,
                        on_retry: move |_| results.restart(),
                    }
                },
                Some(Ok(response)) => {
                    if response.posts.is_empty() {
                        rsx! { p { class: "note", "No posts with this hashtag yet." } }
                    } else {
                        rsx! {
                            for post in response.posts.clone() {
                                PostCard { post: post, origin: origin.clone() }
                            }
                        }
                    }
                }
            }}
        }
    }
}


///
/// `#/` is this page rather than a feed — the trending generator it used to point at
/// keeps its own `#/feed/<uri>` route, which is where the topics below lead.
#[component]
fn ExploreView() -> Element {
    let mut query = use_signal(String::new);
    let parents = use_context::<ParentHandles>().0;
    let results = use_signal(Vec::<PostView>::new);
    let searched = use_signal(|| None::<String>);
    let search_error = use_signal(|| None::<String>);
    let searching = use_signal(|| false);
    let trending = use_resource(move || async move { fetch_trending().await });

    rsx! {
        section { class: "explore",
            form {
                class: "explore__search",
                onsubmit: move |event| {
                    event.prevent_default();
                    start_search(query(), results, searched, search_error, searching, parents);
                },
                input {
                    class: "actor-input",
                    r#type: "search",
                    placeholder: "Search posts",
                    "aria-label": "Search posts",
                    value: "{query}",
                    oninput: move |event| query.set(event.value()),
                }
                button { r#type: "submit", "Search" }
            }
            if let Some(error) = search_error() {
                ErrorPanel {
                    message: error,
                    subject: Subject::Feed,
                    on_retry: move |_| {
                        if let Some(term) = searched() {
                            start_search(term, results, searched, search_error, searching, parents);
                        }
                    },
                }
            }
            if searching() {
                p { class: "note", "Searching…" }
            }
            if let Some(term) = searched() {
                if !searching() && search_error().is_none() && results().is_empty() {
                    p { class: "note", "No posts matched \"{term}\"." }
                }
                for post in results() {
                    PostCard { post: post, origin: "#/".to_string() }
                }
            } else {
                section { class: "explore__trending",
                    h2 { class: "page-title", "Trending" }
                    {match &*trending.read() {
                        None => rsx! { p { class: "note", "Loading trending…" } },
                        Some(Err(error)) => rsx! {
                            p { class: "note", "Couldn't load trending — {error}" }
                        },
                        Some(Ok(response)) => {
                            let topics: Vec<TrendingTopic> = response.topics.clone();
                            rsx! {
                                ul { class: "trend-list",
                                    for topic in topics {
                                        li { class: "trend-list__item",
                                            button {
                                                r#type: "button",
                                                class: "trend-list__button",
                                                onclick: {
                                                    let feed = topic.feed_uri();
                                                    move |_| {
                                                        if let Some(uri) = feed.clone() {
                                                            navigate(&format!("#/feed/{}", encode_uri(&uri)));
                                                        }
                                                    }
                                                },
                                                span { class: "trend-list__name", "{topic.label()}" }
                                                if let Some(description) = topic.description.clone() {
                                                    span { class: "trend-list__meta", "{description}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }}
                }
            }
        }
    }
}

/// The account's feeds, on a page of their own — the rail links here instead of
/// growing a list of every saved feed inline.
#[component]
fn FeedsView(feeds: Vec<(String, String)>, signed_in: bool, discover: String) -> Element {
    rsx! {
        section { class: "feeds-page",
            h2 { class: "page-title", "Feeds" }
            ul { class: "feed-list",
                if signed_in {
                    li {
                        a { class: "feed-list__link", href: "#/following",
                            span { class: "feed-list__name", "Following" }
                            span { class: "feed-list__meta", "Everyone you follow" }
                        }
                    }
                }
                li {
                    a { class: "feed-list__link", href: "#/",
                        span { class: "feed-list__name", "{discover}" }
                        span { class: "feed-list__meta", "Bluesky's trending feed" }
                    }
                }
                for (uri, label) in feeds {
                    li {
                        a { class: "feed-list__link", href: "#/feed/{encode_uri(&uri)}",
                            span { class: "feed-list__name", "{label}" }
                            span { class: "feed-list__meta", "Saved feed" }
                        }
                    }
                }
            }
            if !signed_in {
                p { class: "note", "Sign in to see the feeds your account has saved." }
            }
        }
    }
}

/// Profile: banner, identity, counts, profile links (Patreon/Ko-fi style), then posts.
#[component]
fn ProfileView(route: Signal<Route>) -> Element {
    let current_actor = move || match route() {
        Route::Profile { actor } => actor,
        _ => String::new(),
    };
    let profile_origin = format!("#/profile/{}", encode_uri(&current_actor()));
    let session_state = use_context::<SessionState>();
    let mut profile = use_resource(move || {
        let actor = current_actor();
        async move { fetch_profile(&actor, session_state).await }
    });
    let parent_handles_signal = use_context::<ParentHandles>().0;
    // Optimistic overrides for mute/block, so the button flips immediately instead of
    // waiting for a refetch. `None` means "believe the profile".
    let mut muted = use_signal(|| None::<bool>);
    let mut blocked = use_signal(|| None::<bool>);
    let mut block_uri = use_signal(|| None::<String>);
    let mut action_error = use_signal(|| None::<String>);
    let mut action_busy = use_signal(|| false);
    // A block is public and it affects what other people see, so it is confirmed.
    let mut confirm_block = use_signal(|| false);
    use_effect(move || {
        // Read the route so this re-runs when another profile is opened; otherwise the
        // previous account's "blocked" state would follow you to the next profile.
        let _ = current_actor();
        muted.set(None);
        blocked.set(None);
        block_uri.set(None);
        action_error.set(None);
        confirm_block.set(false);
    });
    // What the posts list needs to know: the optimistic override, else the fetched
    // viewer state. Read through a closure so both sections agree.
    let blocked_now = move || {
        blocked().unwrap_or_else(|| {
            profile
                .read()
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(|fetched| fetched.viewer.as_ref())
                .map(|viewer| viewer.blocking.is_some())
                .unwrap_or(false)
        })
    };
    // An account that opted out of logged-out visibility is labelled on the profile
    // itself, so the whole page — avatar, bio, posts — has to become a prompt. The card
    // gate in `PostCard` covers lists; this one covers the page it would still leave
    // standing.
    let profile_gated = move || {
        if (session_state.0)().is_some() {
            return false;
        }
        profile
            .read()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map(|fetched| fetched.hidden_when_logged_out())
            .unwrap_or(false)
    };
    if profile_gated() {
        return rsx! {
            section { class: "profile profile--gated",
                // No Back button here on purpose: this page is reached by following a
                // link into an account that does not show itself signed out, so there is
                // often no history to go back to. The rail and the pinned tabs are the
                // way out.
                p { class: "note",
                    "This account only shows its profile to signed-in readers."
                }
            }
        };
    }

    let mut posts = use_resource(move || {
        let actor = current_actor();
        async move {
            let result = fetch_author_feed(&actor, session_state).await;
            if let Ok(response) = &result {
                let posts: Vec<PostView> = response
                    .feed
                    .iter()
                    .map(|item| item.post.clone())
                    .collect();
                remember_parents(&posts, parent_handles_signal).await;
            }
            result
        }
    });

    rsx! {
        section { class: "profile",
            button {
                r#type: "button",
                class: "thread__back",
                onclick: move |_| go_back(),
                "← Back"
            }
            {match &*profile.read() {
                None => rsx! { p { class: "note", "Loading profile…" } },
                Some(Err(err)) => rsx! {
                    ErrorPanel {
                        message: err.clone(),
                        subject: Subject::Profile,
                        on_retry: move |_| profile.restart(),
                    }
                },
                Some(Ok(profile)) => {
                    let banner = profile.banner.clone();
                    let avatar = profile.avatar.clone();
                    let name = profile.display().to_string();
                    let handle = format!("@{}", profile.handle);
                    let description = profile.description.clone().unwrap_or_default();
                    let pronouns = profile.pronouns.clone().unwrap_or_default();
                    let stats = format!(
                        "{} following · {} followers · {} posts",
                        profile.follows_count.unwrap_or(0),
                        profile.followers_count.unwrap_or(0),
                        profile.posts_count.unwrap_or(0),
                    );
                    let links: Vec<ProfileLink> = profile
                        .links
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(ProfileLink::is_safe)
                        .collect();
                    // Mute/block are per-viewer state: a public read has no `viewer` at
                    // all, and your own profile must not offer to block itself.
                    let viewer = profile.viewer.clone().unwrap_or_default();
                    let signed_in = (session_state.0)().is_some();
                    let is_self = (session_state.0)()
                        .map(|signed| signed.did == profile.did)
                        .unwrap_or(false);

                    rsx! {
                        div { class: "profile__header",
                            if let Some(banner) = banner {
                                img { class: "profile__banner", src: "{banner}", alt: "", loading: "lazy" }
                            } else {
                                div { class: "profile__banner profile__banner--empty" }
                            }
                            if let Some(avatar) = avatar {
                                img { class: "profile__avatar", src: "{avatar}", alt: "", loading: "lazy" }
                            } else {
                                div { class: "profile__avatar profile__avatar--empty" }
                            }
                        }
                        h2 { class: "profile__name",
                            "{name}"
                            if !pronouns.trim().is_empty() {
                                span { class: "profile__pronouns", " {pronouns}" }
                            }
                        }
                        p { class: "profile__handle", "{handle}" }
                        if !description.trim().is_empty() {
                            p { class: "profile__desc", "{description}" }
                        }
                        p { class: "profile__stats", "{stats}" }
                        if !is_self && signed_in {
                            {
                                // One clone per closure: `did` is moved into each
                                // handler, so a single binding cannot serve both.
                                let mute_did = profile.did.clone();
                                let block_did = profile.did.clone();
                                let block_record = block_uri().or_else(|| viewer.blocking.clone());
                                let muted_now = muted().unwrap_or(viewer.muted);
                                let blocked_now = blocked().unwrap_or(viewer.blocking.is_some());
                                rsx! {
                        if confirm_block() {
                            // Copy matches the official client, so the consequence is
                            // stated in the words people have already read elsewhere.
                            div { class: "block-confirm", role: "alertdialog",
                                "aria-labelledby": "block-confirm-title",
                                p { class: "block-confirm__title", id: "block-confirm-title", "Block account?" }
                                p { class: "block-confirm__body",
                                    "Blocked accounts cannot reply in your threads, mention you, or otherwise interact with you."
                                }
                                div { class: "block-confirm__row",
                                    button {
                                        r#type: "button",
                                        class: "profile__action",
                                        "data-action": "block-cancel",
                                        disabled: action_busy(),
                                        onclick: move |_| confirm_block.set(false),
                                        "Cancel"
                                    }
                                    button {
                                        r#type: "button",
                                        class: "block-confirm__confirm",
                                        "data-action": "block-confirm",
                                        disabled: action_busy(),
                                        onclick: move |_| {
                                            let Some(signed) = (session_state.0)() else {
                                                return;
                                            };
                                            confirm_block.set(false);
                                            apply_block(
                                                signed,
                                                block_did.clone(),
                                                block_record.clone(),
                                                true,
                                                blocked,
                                                block_uri,
                                                action_busy,
                                                action_error,
                                            );
                                        },
                                        if action_busy() { "Blocking…" } else { "Block" }
                                    }
                                }
                            }
                        } else {
                            div { class: "profile__actions",
                                button {
                                    r#type: "button",
                                    class: if muted_now { "profile__action profile__action--on" } else { "profile__action" },
                                    "aria-pressed": if muted_now { "true" } else { "false" },
                                    "data-action": "mute",
                                    disabled: action_busy(),
                                    onclick: move |_| {
                                        let Some(signed) = (session_state.0)() else {
                                            return;
                                        };
                                        let did = mute_did.clone();
                                        let next = !muted_now;
                                        action_busy.set(true);
                                        action_error.set(None);
                                        spawn(async move {
                                            match set_muted(&signed, &did, next).await {
                                                Ok(()) => muted.set(Some(next)),
                                                Err(err) => action_error.set(Some(err)),
                                            }
                                            action_busy.set(false);
                                        });
                                    },
                                    if muted_now { "Unmute" } else { "Mute" }
                                }
                                button {
                                    r#type: "button",
                                    class: if blocked_now { "profile__action profile__action--danger" } else { "profile__action" },
                                    "aria-pressed": if blocked_now { "true" } else { "false" },
                                    "data-action": "block",
                                    disabled: action_busy(),
                                    onclick: move |_| {
                                        let Some(signed) = (session_state.0)() else {
                                            return;
                                        };
                                        if !blocked_now {
                                            // Ask first: a block is public, and it
                                            // changes what they can do to you.
                                            confirm_block.set(true);
                                            return;
                                        }
                                        apply_block(
                                            signed,
                                            block_did.clone(),
                                            block_record.clone(),
                                            false,
                                            blocked,
                                            block_uri,
                                            action_busy,
                                            action_error,
                                        );
                                    },
                                    if blocked_now { "Unblock" } else { "Block" }
                                }
                            }
                        }
                                }
                            }
                            if let Some(err) = action_error() {
                                p { class: "profile__action-error", "{err}" }
                            }
                        }
                        if signed_in && !is_self && viewer.blocked_by {
                            // They block this account: nothing here can be interacted
                            // with, and saying so is better than a silent empty profile.
                            p { class: "note", "This account blocks you." }
                        }
                        if !links.is_empty() {
                            div { class: "profile__links",
                                for link in links {
                                    a {
                                        class: "profile__link",
                                        href: "{link.url}",
                                        target: "_blank",
                                        rel: "noreferrer noopener",
                                        if let Some(icon) = link.icon.clone() {
                                            img { class: "profile__link-icon", src: "{icon}", alt: "", loading: "lazy" }
                                        }
                                        span { "{link.title}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }}
            if blocked_now() {
                // A block hides their posts from this app too, rather than showing a
                // list the AppView will refuse to fill.
                p { class: "note", "You blocked this account. Their posts are hidden here." }
            } else {
                {match &*posts.read() {
                None => rsx! { p { class: "note", "Loading posts…" } },
                Some(Err(err)) => rsx! {
                    ErrorPanel {
                        message: err.clone(),
                        subject: Subject::AuthorFeed,
                        on_retry: move |_| posts.restart(),
                    }
                },
                Some(Ok(response)) => {
                    let items: Vec<(PostView, Option<String>)> = response
                        .feed
                        .iter()
                        .map(|item| {
                            let label = match &item.reason {
                                Some(FeedViewReason::Repost { by, .. }) => {
                                    let name = by
                                        .display_name
                                        .clone()
                                        .filter(|name| !name.trim().is_empty())
                                        .unwrap_or_else(|| format!("@{}", by.handle));
                                    Some(format!("Reposted by {name}"))
                                }
                                Some(FeedViewReason::Pin) => Some("Pinned post".to_string()),
                                _ => None,
                            };
                            (item.post.clone(), label)
                        })
                        .collect();
                    rsx! {
                        for (post, label) in items {
                            PostCard { post: post, reposted_by: label, origin: profile_origin.clone() }
                        }
                    }
                }
            }}
            }
        }
    }
}
