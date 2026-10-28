//! Protocol-side types and the "alt text first" rendering policy.
//!
//! These types mirror the `#view` definitions of the AT Protocol embed lexicons,
//! which is what the AppView actually returns inside an `app.bsky.feed.defs#postView`:
//!
//! * `app.bsky.embed.images#viewImage`  -> `thumb`, `fullsize`, `alt` (required), `aspectRatio`
//! * `app.bsky.embed.gallery#viewImage` -> `thumbnail`, `fullsize`, `alt` (required), `aspectRatio` (required)
//! * `app.bsky.embed.video#view`        -> `playlist`, `thumbnail`, `alt` (OPTIONAL), `aspectRatio`
//!
//! The important asymmetry: an image embed *must* carry an `alt` string, but the
//! string is allowed to be empty, and a video embed may omit it entirely. So a
//! client that puts alt text first always needs an explicit "undescribed" state.

use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

/// AT Protocol OAuth: the pure parts (PKCE, JWK thumbprint, DPoP signing input).
pub mod oauth;

/// Which sign-in paths a build offers — re-exported so callers do not need the module path.
pub use oauth::SignInMode;

/// RFC 3986 unreserved characters stay literal: ALPHA / DIGIT / `-` / `.` / `_` / `~`.
/// Everything else is percent-encoded, which is what a query value or a path
/// segment requires — `+` in a query means space, and `/`, `:`, `&`, `=` all have
/// structural meaning.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Percent-encodes an arbitrary value for use as a URL component (query value or
/// hash-route segment). Cursors, at-URIs and handles all go through this.
pub fn encode_uri_component(value: &str) -> String {
    utf8_percent_encode(value, URI_COMPONENT).to_string()
}

/// Inverse of [`encode_uri_component`]. Invalid UTF-8 is replaced rather than
/// panicking: a malformed URL should degrade to a bad route, not a crash.
pub fn decode_uri_component(value: &str) -> String {
    percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// Whether the reader may mute or block this author.
///
/// Never on your own account: muting yourself hides your posts from your own timelines, and
/// blocking yourself is a request the server has no meaningful answer for. The menu used to
/// offer both on your own posts, because nothing checked whose post it was.
pub fn can_moderate_author(viewer_did: Option<&str>, author_did: &str) -> bool {
    match viewer_did {
        Some(viewer) => viewer != author_did,
        None => false,
    }
}

/// Whether this post is the reader's own, i.e. whether deleting it is even possible: a
/// `deleteRecord` targets a record in your own repository.
pub fn is_own_post(viewer_did: Option<&str>, author_did: &str) -> bool {
    matches!(viewer_did, Some(viewer) if viewer == author_did)
}

/// The record key of an at-uri: `at://did/app.bsky.feed.post/<rkey>` → `<rkey>`.
///
/// Deleting a record needs the key on its own (`com.atproto.repo.deleteRecord` takes
/// `collection` + `rkey`, not a uri), and getting it wrong deletes nothing while looking like
/// it worked. Rejects anything that is not a well-formed at-uri with a collection.
pub fn record_key(at_uri: &str) -> Option<&str> {
    let rest = at_uri.strip_prefix("at://")?;
    let mut parts = rest.split('/');
    let authority = parts.next()?;
    let collection = parts.next()?;
    let rkey = parts.next()?;
    if authority.is_empty() || collection.is_empty() || rkey.is_empty() {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(rkey)
}

/// `app.bsky.embed.defs#aspectRatio`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AspectRatio {
    pub width: i64,
    pub height: i64,
}

impl AspectRatio {
    /// `None` for degenerate values (`0` width/height, NaN, infinity).
    pub fn ratio(self) -> Option<f64> {
        if self.width <= 0 || self.height <= 0 {
            return None;
        }
        let ratio = self.width as f64 / self.height as f64;
        if ratio.is_finite() && ratio > 0.0 {
            Some(ratio)
        } else {
            None
        }
    }
}

/// Max height of a feed image, matching the official client: it never renders
/// taller than 1:2 in a feed, so a very tall image is letterboxed rather than
/// pushing the rest of the timeline off-screen.
pub const MIN_FEED_RATIO: f64 = 0.5;

/// Used when the record has no `aspectRatio` (optional for images and videos).
pub const DEFAULT_RATIO: f64 = 16.0 / 9.0;

/// Aspect ratio to reserve *before* any media is loaded.
///
/// Reserving space up front is what keeps the "show the text first, load the
/// image only on demand" model from causing layout shift when the image is
/// eventually revealed.
pub fn placeholder_ratio(aspect_ratio: Option<&AspectRatio>) -> f64 {
    aspect_ratio
        .and_then(|a| a.ratio())
        .map(|r| r.max(MIN_FEED_RATIO))
        .unwrap_or(DEFAULT_RATIO)
}

/// `app.bsky.embed.images#viewImage`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageView {
    pub thumb: String,
    pub fullsize: String,
    /// Required by the lexicon, but may be an empty string.
    pub alt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
}

/// `app.bsky.embed.gallery#viewImage` (the multi-image embed, soft limit 10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryItemView {
    pub thumbnail: String,
    pub fullsize: String,
    pub alt: String,
    pub aspect_ratio: AspectRatio,
}

/// `app.bsky.embed.video#view`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoView {
    pub cid: String,
    /// HLS playlist (`.m3u8`) served by the video service.
    pub playlist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// OPTIONAL: video records may have no alt text at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<AspectRatio>,
    /// `default` or `gif`; `gif` means "behaves like an animation, autoplay muted".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<String>,
}

/// What the gate in front of the media should render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltPolicy {
    /// There is usable alt text: show it as the primary content of the post.
    AltThenMedia,
    /// No usable alt text: do not pretend otherwise, say so explicitly.
    UndescribedPrompt,
}

impl AltPolicy {
    pub fn is_described(self) -> bool {
        matches!(self, AltPolicy::AltThenMedia)
    }
}

/// Classifies an alt string. Whitespace-only counts as missing, because a
/// timeline full of blank description boxes is worse than an honest prompt.
pub fn alt_policy(alt: &str) -> AltPolicy {
    if alt.trim().is_empty() {
        AltPolicy::UndescribedPrompt
    } else {
        AltPolicy::AltThenMedia
    }
}

/// Video alt is `Option`, so fold it into the same policy as images.
pub fn video_alt_policy(alt: Option<&str>) -> AltPolicy {
    alt_policy(alt.unwrap_or_default())
}

/// `app.bsky.richtext.facet`. Facets annotate byte ranges of `record.text`;
/// the text itself remains the source of truth, so a bad facet must degrade to
/// plain text rather than break rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Facet {
    pub index: ByteRange,
    #[serde(default)]
    pub features: Vec<FacetFeature>,
}

/// Byte offsets into UTF-8 text — NOT character indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteRange {
    pub byte_start: usize,
    pub byte_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum FacetFeature {
    #[serde(rename = "app.bsky.richtext.facet#mention")]
    Mention { did: String },
    #[serde(rename = "app.bsky.richtext.facet#link")]
    Link { uri: String },
    #[serde(rename = "app.bsky.richtext.facet#tag")]
    Tag { tag: String },
    #[serde(other)]
    Unknown,
}

/// Link facets for text this client is about to publish.
///
/// atproto stores annotations as **byte** ranges into the post text, so a URL is only clickable
/// if the record carries `app.bsky.richtext.facet#link` over exactly the right offsets. Nothing
/// here resolves mentions — that needs a handle-to-DID lookup — so this is the half that lets a
/// reader share a URL, which is the half you notice when it is missing.
///
/// A scanner rather than a regex on purpose: the crate has no regex dependency, and the rules
/// that actually matter are three — a scheme, a host, and not swallowing the punctuation that
/// ends the sentence.
pub fn link_facets(text: &str) -> Vec<Facet> {
    // ASCII-only lowercasing leaves every byte offset identical to `text`, which is exactly
    // what the ranges have to be.
    let lowered = text.to_ascii_lowercase();
    let mut facets = Vec::new();
    let mut search = 0usize;

    while let Some(offset) = lowered[search..].find("http") {
        let start = search + offset;
        search = start + 4;

        let scheme = if lowered[start..].starts_with("https://") {
            8
        } else if lowered[start..].starts_with("http://") {
            7
        } else {
            continue;
        };
        // `xhttp://…` is not a link; the scheme has to start a token.
        if text[..start]
            .chars()
            .next_back()
            .is_some_and(|prev| prev.is_alphanumeric())
        {
            continue;
        }

        // Walk to the end of the URL, keeping balanced parentheses (Wikipedia) and stopping at
        // an unbalanced closing one (a URL wrapped in a sentence's brackets).
        let base = start + scheme;
        let mut end = base;
        let mut depth = 0usize;
        for (offset, ch) in text[base..].char_indices() {
            if ch.is_whitespace() {
                break;
            }
            match ch {
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                _ => {}
            }
            end = base + offset + ch.len_utf8();
        }

        // A sentence that ends in a URL ends with punctuation the URL does not own.
        while end > base {
            let last = text[..end].chars().next_back().expect("end > base");
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"') {
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        if end <= base {
            continue;
        }

        let uri = &text[start..end];
        let host = uri[scheme..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        if host.is_empty() || !host.contains('.') {
            // `https://`, `https:///path`, or a dotless name. A `localhost` URL is
            // deliberately not a link: nobody posts one to a public network, so a clause
            // honouring it would be untested-in-practice behaviour carried forever.
            continue;
        }

        facets.push(Facet {
            index: ByteRange {
                byte_start: start,
                byte_end: end,
            },
            features: vec![FacetFeature::Link {
                uri: uri.to_string(),
            }],
        });
        search = end;
    }

    facets
}

/// A piece of post text with an optional annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSegment<'a> {
    Plain(&'a str),
    Mention { text: &'a str, did: &'a str },
    Link { text: &'a str, uri: &'a str },
    Tag { text: &'a str, tag: &'a str },
}

/// Splits text into segments using facets, safely.
///
/// Offsets are UTF-8 bytes, so an emoji earlier in the post shifts every later
/// offset. A facet is dropped when it is out of bounds, splits a character,
/// has no usable feature, or overlaps an earlier facet — in all of those cases
/// the affected text still renders as plain text.
pub fn text_segments<'a>(text: &'a str, facets: Option<&'a [Facet]>) -> Vec<TextSegment<'a>> {
    let plain = |text: &'a str| vec![TextSegment::Plain(text)];
    let Some(facets) = facets else {
        return plain(text);
    };

    enum Kind<'a> {
        Mention(&'a str),
        Link(&'a str),
        Tag(&'a str),
    }

    let mut ranges: Vec<(usize, usize, &'a str, Kind<'a>)> = facets
        .iter()
        .filter_map(|facet| {
            let (start, end) = (facet.index.byte_start, facet.index.byte_end);
            if start >= end || end > text.len() {
                return None;
            }
            // `get` returns None when the range splits a multi-byte character.
            let slice = text.get(start..end)?;
            let kind = facet.features.iter().find_map(|feature| match feature {
                FacetFeature::Mention { did } => Some(Kind::Mention(did.as_str())),
                FacetFeature::Link { uri } => Some(Kind::Link(uri.as_str())),
                FacetFeature::Tag { tag } => Some(Kind::Tag(tag.as_str())),
                FacetFeature::Unknown => None,
            })?;
            Some((start, end, slice, kind))
        })
        .collect();

    ranges.sort_by_key(|(start, _, _, _)| *start);

    let mut segments = Vec::new();
    let mut cursor = 0usize;
    for (start, end, slice, kind) in ranges {
        if start < cursor {
            continue; // overlaps something already emitted
        }
        if start > cursor {
            if let Some(gap) = text.get(cursor..start) {
                segments.push(TextSegment::Plain(gap));
            }
        }
        segments.push(match kind {
            Kind::Mention(did) => TextSegment::Mention { text: slice, did },
            Kind::Link(uri) => TextSegment::Link { text: slice, uri },
            Kind::Tag(tag) => TextSegment::Tag { text: slice, tag },
        });
        cursor = end;
    }
    if cursor < text.len() {
        if let Some(tail) = text.get(cursor..) {
            segments.push(TextSegment::Plain(tail));
        }
    }
    segments
}

/// `app.bsky.actor.defs#viewerState`, the parts a profile needs: whether *this*
/// viewer muted or blocked the account, and the block record's uri so it can be
/// undone. `blocking` is a record uri, not a boolean — that uri is the only handle
/// on the record, exactly like `viewer.like`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileViewerState {
    #[serde(default)]
    pub muted: bool,
    /// The `app.bsky.graph.block` record uri, present only while blocked.
    #[serde(default)]
    pub blocking: Option<String>,
    /// Whether *they* block the viewer. A profile that says this must not offer to
    /// follow or reply; blocking back would be the only sensible action.
    #[serde(default)]
    pub blocked_by: bool,
    #[serde(default)]
    pub following: Option<String>,
}

/// `com.atproto.repo.strongRef`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StrongRef {
    pub uri: String,
    pub cid: String,
}

/// `app.bsky.feed.post#replyRef` — the graph edges of a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyRef {
    pub root: StrongRef,
    pub parent: StrongRef,
}

/// `app.bsky.feed.defs#threadViewPost`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadViewPost {
    pub post: PostView,
    #[serde(default)]
    pub parent: Option<Box<ThreadViewNode>>,
    #[serde(default)]
    pub replies: Option<Vec<ThreadViewNode>>,
}

/// A node in a `getPostThread` tree. Blocked and deleted posts are first-class
/// variants: a thread must render them as placeholders, not drop them, or reply
/// indentation silently lies about the conversation structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum ThreadViewNode {
    #[serde(rename = "app.bsky.feed.defs#threadViewPost")]
    Post(ThreadViewPost),
    #[serde(rename = "app.bsky.feed.defs#notFoundPost")]
    NotFound {
        #[serde(default)]
        uri: String,
        #[serde(default)]
        not_found: bool,
    },
    #[serde(rename = "app.bsky.feed.defs#blockedPost")]
    Blocked {
        #[serde(default)]
        uri: String,
        #[serde(default)]
        blocked: bool,
    },
    #[serde(other)]
    Unknown,
}

impl ThreadViewNode {
    pub fn post(&self) -> Option<&PostView> {
        match self {
            Self::Post(view) => Some(&view.post),
            _ => None,
        }
    }

    /// Depth-first count of every node, including blocked/deleted placeholders.
    pub fn count_nodes(&self) -> usize {
        match self {
            Self::Post(view) => {
                1 + view
                    .replies
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(ThreadViewNode::count_nodes)
                    .sum::<usize>()
            }
            Self::Unknown => 1,
            _ => 1,
        }
    }
}

/// `app.bsky.feed.getPostThread` output (threadgate is ignored for now).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostThreadResponse {
    pub thread: ThreadViewNode,
}

/// `app.bsky.actor.defs#profileViewDetailed`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileViewDetailed {
    pub did: String,
    pub handle: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub banner: Option<String>,
    #[serde(default)]
    pub followers_count: Option<i64>,
    #[serde(default)]
    pub follows_count: Option<i64>,
    #[serde(default)]
    pub posts_count: Option<i64>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Profile links (the "add Patreon / Ko-fi / newsletter" feature).
    #[serde(default)]
    pub links: Option<Vec<ProfileLink>>,
    #[serde(default)]
    pub pronouns: Option<String>,
    /// Only populated for an authenticated read, like `PostView.viewer`.
    #[serde(default)]
    pub viewer: Option<ProfileViewerState>,
    /// Account-level labels; `!no-unauthenticated` means hide the whole profile.
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
}

impl ProfileViewDetailed {
    pub fn hidden_when_logged_out(&self) -> bool {
        hidden_when_logged_out(self.labels.as_deref())
    }
}

/// `app.bsky.actor.link` — a labelled outbound link on a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileLink {
    pub uri: String,
    #[serde(default)]
    pub cid: String,
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
}

impl ProfileLink {
    /// Only http(s) links are rendered: a `javascript:` URL in a profile record
    /// must never become a clickable anchor.
    pub fn is_safe(&self) -> bool {
        self.url.starts_with("https://") || self.url.starts_with("http://")
    }
}

impl ProfileViewDetailed {
    /// Display name, falling back to the handle so a profile is never blank.
    pub fn display(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.handle)
    }
}

/// `app.bsky.actor.getPreferences` — the account's saved/pinned feeds.
///
/// These are *not* public: the record lives behind an authenticated endpoint, so
/// they only appear once the reader signs in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreferencesResponse {
    #[serde(default)]
    pub preferences: Vec<Preference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum Preference {
    #[serde(rename = "app.bsky.actor.defs#savedFeedsPrefV2")]
    SavedFeedsV2 {
        #[serde(default)]
        items: Vec<SavedFeed>,
    },
    #[serde(rename = "app.bsky.actor.defs#savedFeedsPref")]
    SavedFeedsLegacy {
        #[serde(default)]
        pinned: Vec<String>,
        #[serde(default)]
        saved: Vec<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFeed {
    #[serde(default)]
    pub id: String,
    /// `feed` for a generator, `timeline` for the built-in Following.
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub label: Option<String>,
}

/// How many leading posts of a refreshed page are ones the reader has not seen.
///
/// Stops at the first known post: past that point the ordering may simply have
/// shifted, and counting further would overstate how much is actually new.
pub fn new_post_count(fresh: &[String], known: &std::collections::HashSet<String>) -> usize {
    fresh.iter().take_while(|uri| !known.contains(*uri)).count()
}

/// `app.bsky.feed.getLikes` — who liked a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LikesResponse {
    #[serde(default)]
    pub likes: Vec<LikeRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeRecord {
    /// A `ProfileView` — the fields we render are the author ones.
    pub actor: Author,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// `app.bsky.feed.getRepostedBy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepostedByResponse {
    #[serde(default)]
    pub reposted_by: Vec<Author>,
}

/// `app.bsky.feed.getQuotes` — the posts that quote this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotesResponse {
    #[serde(default)]
    pub posts: Vec<PostView>,
}

/// `app.bsky.notification.listNotifications` — requires a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsResponse {
    #[serde(default)]
    pub notifications: Vec<Notification>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    #[serde(default)]
    pub uri: String,
    pub author: Author,
    /// `like` | `repost` | `follow` | `mention` | `reply` | `quote` | `starterpack-joined` …
    #[serde(default)]
    pub reason: String,
    /// The post the notification is about, when there is one.
    #[serde(default)]
    pub reason_subject: Option<String>,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub indexed_at: Option<String>,
}

/// What a notification says the person did. Unknown reasons get a neutral verb
/// rather than an invented one: the app does not know what a future reason means.
pub fn notification_verb(reason: &str) -> &'static str {
    match reason {
        "like" => "liked your post",
        "repost" => "reposted your post",
        "follow" => "followed you",
        "mention" => "mentioned you",
        "reply" => "replied to you",
        "quote" => "quoted your post",
        "starterpack-joined" => "joined from your starter pack",
        _ => "interacted with you",
    }
}

/// `app.bsky.feed.getPosts` — batch post lookup, used to resolve the author a
/// reply answers, which the feed's `record.reply.parent` only names as an at-uri.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostsResponse {
    #[serde(default)]
    pub posts: Vec<PostView>,
}

/// `app.bsky.feed.searchPosts` — served by `api.bsky.app`, not the public AppView,
/// which refuses search with a 403.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPostsResponse {
    #[serde(default)]
    pub posts: Vec<PostView>,
}

/// One trending story from `app.bsky.unspecced.getTrendingTopics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendingTopic {
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Bluesky's own path for the topic, `/profile/<did>/feed/<rkey>`.
    #[serde(default)]
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendingTopicsResponse {
    #[serde(default)]
    pub topics: Vec<TrendingTopic>,
}

impl TrendingTopic {
    /// What to show as the headline: the display name, else the topic.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.topic)
    }

    /// The feed generator behind the topic, as an at-uri, when the link points at one.
    pub fn feed_uri(&self) -> Option<String> {
        let rest = self.link.strip_prefix("/profile/")?;
        let (did, rkey) = rest.split_once("/feed/")?;
        if did.is_empty() || rkey.is_empty() || rkey.contains('/') {
            return None;
        }
        Some(format!("at://{did}/app.bsky.feed.generator/{rkey}"))
    }
}

/// Replies the AppView did not send.
///
/// `getPostThread` truncates at its `depth` and simply leaves the deeper branches
/// out; nothing in the payload marks the cut. `replyCount` against the children
/// actually returned is the only signal that a branch continues — which is what
/// turns a thread that "just ends" into one that offers to go on.
pub fn unfetched_replies(reply_count: Option<i64>, fetched: usize) -> usize {
    let counted = reply_count.unwrap_or(0).max(0) as usize;
    counted.saturating_sub(fetched)
}

/// Label for the junction that continues a branch the depth cut short.
pub fn continue_thread_label(missing: usize) -> String {
    match missing {
        0 => String::new(),
        1 => "Continue thread (1 reply)".to_string(),
        count => format!("Continue thread ({count} replies)"),
    }
}

/// Why a request produced no data, in terms a reader can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedFailureKind {
    /// No response at all: offline, DNS failure, or a dropped connection. The
    /// browser deliberately hides which one, so the hint covers all three.
    Unreachable,
    /// 401/403: no session, expired session, or not allowed.
    Refused,
    /// 400/404: the feed or post is gone — deleted, renamed, or made private.
    Missing,
    /// 429: rate limited.
    RateLimited,
    /// 5xx: Bluesky's side, not the reader's.
    ServerError,
    /// Anything else.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedFailure {
    pub kind: FeedFailureKind,
    /// 0 when the request never got a response.
    pub status: u16,
    /// Short version of whatever the server or browser said.
    pub detail: String,
}

impl FeedFailure {
    /// Parse our own transport error string, `HTTP {status} from {url}: {body}`.
    /// Anything unrecognized is `Unknown` rather than a transport failure: local
    /// refusals ("Sign in to load Following") would otherwise be misreported as
    /// "can't reach Bluesky", which is exactly the kind of wrong diagnosis this
    /// type exists to prevent.
    pub fn parse(message: &str) -> Self {
        let trimmed = message.trim();
        let mut status: Option<u16> = None;
        let mut detail = trimmed.to_string();

        if let Some(rest) = trimmed.strip_prefix("HTTP ") {
            if let Some((head, tail)) = rest.split_once(" from ") {
                if let Ok(parsed) = head.trim().parse::<u16>() {
                    status = Some(parsed);
                    detail = tail
                        .split_once(": ")
                        .map(|(_, body)| body)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                }
            }
        }

        let kind = if trimmed.starts_with("Sign in to") {
            // Our own precondition, not a network failure.
            FeedFailureKind::Refused
        } else {
            match status {
                Some(0) => FeedFailureKind::Unreachable,
                Some(401 | 403) => FeedFailureKind::Refused,
                Some(400 | 404) => FeedFailureKind::Missing,
                Some(429) => FeedFailureKind::RateLimited,
                Some(500..=599) => FeedFailureKind::ServerError,
                _ => FeedFailureKind::Unknown,
            }
        };

        // Server bodies are JSON blobs; enough to identify, not enough to scroll.
        Self {
            kind,
            status: status.unwrap_or(0),
            detail: detail.chars().take(160).collect(),
        }
    }

    /// Which side of the connection is at fault, in one line.
    pub fn headline(&self) -> &'static str {
        match self.kind {
            FeedFailureKind::Unreachable => "Can't reach Bluesky",
            FeedFailureKind::Refused => "Not authorized",
            FeedFailureKind::Missing => "That feed isn't available",
            FeedFailureKind::RateLimited => "Too many requests",
            FeedFailureKind::ServerError => "Bluesky is having trouble",
            FeedFailureKind::Unknown => "Couldn't load this feed",
        }
    }

    /// What the reader can actually do about it.
    pub fn hint(&self) -> &'static str {
        match self.kind {
            FeedFailureKind::Unreachable => {
                "The request got no response at all. Either you are offline, or \
                 bsky.app can't be reached from this network (DNS or blocking)."
            }
            FeedFailureKind::Refused => {
                "This needs a signed-in session. If you were signed in, the session \
                 may have expired — sign in again."
            }
            FeedFailureKind::Missing => {
                "The feed may have been deleted, renamed, or made private. Custom \
                 feeds disappear this way more often than they fail."
            }
            FeedFailureKind::RateLimited => "Bluesky is throttling requests. Wait a minute, then retry.",
            FeedFailureKind::ServerError => {
                "This is on Bluesky's side, not your connection. Nothing to fix \
                 locally — retry shortly."
            }
            FeedFailureKind::Unknown => "No diagnosis available; the detail below is all there is.",
        }
    }
}

/// What was being loaded, so the wording can name the right thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Feed,
    Post,
    Profile,
    /// A profile's own post list, which fails separately from the profile itself.
    AuthorFeed,
}

impl Subject {
    fn noun(self) -> &'static str {
        match self {
            Subject::Feed => "feed",
            Subject::Post => "post",
            Subject::Profile => "profile",
            Subject::AuthorFeed => "posts",
        }
    }
}

impl FeedFailure {
    /// Headline with the right noun: a missing post must not be announced as a
    /// missing feed, even though the transport diagnosis is identical.
    pub fn headline_for(&self, subject: Subject) -> String {
        match (self.kind, subject) {
            (FeedFailureKind::Missing, Subject::AuthorFeed) => {
                "That account's posts aren't available".to_string()
            }
            (FeedFailureKind::Missing, subject) => {
                format!("That {} isn't available", subject.noun())
            }
            _ => self.headline().to_string(),
        }
    }

    /// Hint with the right noun. Only the "it's gone" case differs: a feed has
    /// been deleted or made private, a post may have been removed, an account may
    /// be suspended.
    pub fn hint_for(&self, subject: Subject) -> &'static str {
        match (self.kind, subject) {
            (FeedFailureKind::Missing, Subject::Post) => {
                "The post may have been deleted, or its author blocked you."
            }
            (FeedFailureKind::Missing, Subject::Profile) => {
                "The account may be deactivated, suspended, or blocking you."
            }
            (FeedFailureKind::Missing, Subject::AuthorFeed) => {
                "The account may be deactivated, or it may be blocking you."
            }
            _ => self.hint(),
        }
    }
}

/// Label for the whats-hot generator.
///
/// Its `displayName` is "Discover", but anonymous traffic gets global trending,
/// which is honestly "What's Hot" — so: the account's saved name wins, then
/// "Discover" when signed in, then the anonymous label.
pub fn discover_label(saved: Option<&str>, signed_in: bool) -> String {
    if let Some(saved) = saved.filter(|label| !label.trim().is_empty()) {
        return saved.to_string();
    }
    if signed_in {
        "Discover".to_string()
    } else {
        "What's Hot".to_string()
    }
}

/// What a tab title is about.
///
/// Exists so the title is built in one testable place: the "Home" route used to
/// hardcode `What's Hot` and ignore the resolved label entirely, which meant a
/// signed-in reader with Discover selected saw the wrong name in the tab.
pub enum TitleSubject<'a> {
    Feed { label: &'a str },
    Following,
    Feeds,
    Settings,
    Compose,
    Explore,
    Notifications,
    Engagement { label: &'a str },
    Post,
    Profile { handle: &'a str },
    SignIn,
}

pub fn tab_title(subject: TitleSubject<'_>, app: &str) -> String {
    match subject {
        TitleSubject::Feed { label } => {
            let name = if label.trim().is_empty() { "Feed" } else { label };
            format!("{name} — {app}")
        }
        TitleSubject::Following => format!("Following — {app}"),
        TitleSubject::Feeds => format!("Feeds — {app}"),
        TitleSubject::Settings => format!("Settings — {app}"),
        TitleSubject::Compose => format!("New post — {app}"),
        TitleSubject::Explore => format!("Explore — {app}"),
        TitleSubject::Notifications => format!("Notifications — {app}"),
        TitleSubject::Engagement { label } => format!("{label} — {app}"),
        TitleSubject::Post => format!("Post — {app}"),
        TitleSubject::Profile { handle } => format!("@{handle} — {app}"),
        TitleSubject::SignIn => format!("Sign in — {app}"),
    }
}

/// What to tell the reader at the bottom of a paginated feed.
///
/// Endless scroll hides the paging affordance, so the trailing note is the only
/// signal that the list ended rather than stalled. Keeping the choice pure makes
/// the "we actually ran out of posts" branch testable without crawling a feed.
pub fn feed_end_note(item_count: usize, has_cursor: bool, loading: bool) -> Option<&'static str> {
    if has_cursor {
        return loading.then_some("Loading more…");
    }
    // An empty, cursorless feed is a blank state, not an ending.
    (item_count > 0).then_some("You've reached the end of this feed.")
}

impl PreferencesResponse {
    /// Pinned-first generator URIs. `timeline` entries are skipped: Following is
    /// not a generator and is reached through `getTimeline` instead.
    pub fn saved_feed_uris(&self) -> Vec<String> {
        let mut pinned = Vec::new();
        let mut rest = Vec::new();

        for preference in &self.preferences {
            match preference {
                Preference::SavedFeedsV2 { items } => {
                    for item in items {
                        if item.kind != "feed" {
                            continue;
                        }
                        if item.pinned {
                            pinned.push(item.value.clone());
                        } else {
                            rest.push(item.value.clone());
                        }
                    }
                }
                Preference::SavedFeedsLegacy { pinned: p, saved } => {
                    pinned.extend(p.iter().cloned());
                    rest.extend(saved.iter().cloned());
                }
                Preference::Unknown => {}
            }
        }

        let mut seen = std::collections::HashSet::new();
        pinned
            .into_iter()
            .chain(rest)
            .filter(|uri| seen.insert(uri.clone()))
            .collect()
    }
}

/// `app.bsky.feed.getFeedGenerators` — batched names for saved feeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedGeneratorsResponse {
    #[serde(default)]
    pub feeds: Vec<FeedGeneratorView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedGeneratorView {
    pub uri: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub creator: Option<Author>,
}

/// `com.atproto.label.defs#label`, the parts that change behaviour.
///
/// The one that matters here is `!no-unauthenticated`: the account has opted out of
/// being visible to signed-out readers, and the AppView stamps every post and profile
/// of theirs with it. An app that ignores labels happily shows exactly the content the
/// author asked not to show publicly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub val: String,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub cid: Option<String>,
}

/// The label an account gets when it opts out of logged-out visibility.
pub const NO_UNAUTHENTICATED: &str = "!no-unauthenticated";

/// Whether a set of labels means "only show this to signed-in readers".
pub fn hidden_when_logged_out(labels: Option<&[Label]>) -> bool {
    labels
        .map(|labels| labels.iter().any(|label| label.val == NO_UNAUTHENTICATED))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Author {
    pub did: String,
    pub handle: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    /// Account-level labels. `!no-unauthenticated` lands here, not on the post, so this
    /// is the field that decides whether a signed-out reader may see anything at all.
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
}

impl Author {
    pub fn hidden_when_logged_out(&self) -> bool {
        hidden_when_logged_out(self.labels.as_deref())
    }
}

/// The `record` field is an open union; only the post fields are typed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostRecord {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Rich-text annotations over `text` (mentions, links, tags).
    #[serde(default)]
    pub facets: Option<Vec<Facet>>,
    /// Present when this post is a reply; the edges of the thread graph.
    #[serde(default)]
    pub reply: Option<ReplyRef>,
}

/// `app.bsky.feed.defs#postView` (the parts this client needs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    pub uri: String,
    /// Content hash of the record; `like` records must reference it.
    #[serde(default)]
    pub cid: String,
    pub author: Author,
    pub record: PostRecord,
    /// Present only when the request carried a session: what *this* viewer has done.
    #[serde(default)]
    pub viewer: Option<ViewerState>,
    #[serde(default)]
    pub embed: Option<EmbedView>,
    /// ISO-8601 timestamp the AppView indexed the post at.
    #[serde(default)]
    pub indexed_at: Option<String>,
    #[serde(default)]
    pub reply_count: Option<i64>,
    #[serde(default)]
    pub repost_count: Option<i64>,
    #[serde(default)]
    pub like_count: Option<i64>,
    /// Posts quoting this one. Present in every `PostView`, and the only number on
    /// a card that used to have no way to see what it counts.
    #[serde(default)]
    pub quote_count: Option<i64>,
    /// Bookmarks. Private: there is no public list of who saved a post, so this
    /// count is shown but cannot open anything.
    #[serde(default)]
    pub bookmark_count: Option<i64>,
    /// Moderation labels. `!no-unauthenticated` means the account opted out of
    /// logged-out visibility, so this must not be rendered without a session.
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
}

impl PostView {
    /// True when this post may only be shown to a signed-in reader.
    ///
    /// Two places to check, because the AppView puts the account's choice on the
    /// *author* and a post-level label on the post: either one is enough to hide it.
    pub fn hidden_when_logged_out(&self) -> bool {
        hidden_when_logged_out(self.labels.as_deref()) || self.author.hidden_when_logged_out()
    }
}

impl PostView {
    /// The `root`/`parent` strong refs that a reply to this post must carry.
    ///
    /// Answering a reply has to point `root` at the *thread* root, not at the post
    /// being answered: the AppView uses those two refs to place the new record in the
    /// conversation, and pointing `root` at a mid-thread post forks the thread — the
    /// reply then appears in a conversation of its own, or in none.
    ///
    /// A post that is not itself a reply is its own root.
    pub fn reply_refs(&self) -> ReplyRef {
        let parent = StrongRef {
            uri: self.uri.clone(),
            cid: self.cid.clone(),
        };
        let root = self
            .record
            .reply
            .as_ref()
            .map(|reply| reply.root.clone())
            .unwrap_or_else(|| parent.clone());
        ReplyRef { root, parent }
    }
}

/// The signed-in viewer's relationship to a post. The like URI doubles as the
/// record to delete when un-liking, so it is the only state a toggle needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerState {
    #[serde(default)]
    pub like: Option<String>,
    #[serde(default)]
    pub repost: Option<String>,
}

/// `app.bsky.embed.external#view`.
///
/// Note: the protocol has **no alt text** for link preview thumbnails. There is
/// nothing to put first, so this embed's collapsed state can only offer
/// "show thumbnail" — it must not pretend `title`/`description` describe the image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalView {
    pub uri: String,
    pub title: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

/// `app.bsky.embed.record#view` — a quoted post, which may be gone or blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum EmbedRecordView {
    #[serde(rename = "app.bsky.embed.record#viewRecord")]
    Record(Box<QuotedPost>),
    #[serde(rename = "app.bsky.embed.record#viewNotFound")]
    NotFound {
        #[serde(default)]
        uri: String,
        #[serde(default)]
        not_found: bool,
    },
    #[serde(rename = "app.bsky.embed.record#viewBlocked")]
    Blocked {
        #[serde(default)]
        uri: String,
        #[serde(default)]
        blocked: bool,
    },
    #[serde(rename = "app.bsky.embed.record#viewDetached")]
    Detached {
        #[serde(default)]
        uri: String,
        #[serde(default)]
        detached: bool,
    },
    #[serde(other)]
    Unknown,
}

/// `app.bsky.embed.record#viewRecord`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotedPost {
    pub uri: String,
    #[serde(default)]
    pub cid: String,
    pub author: Author,
    /// The quoted record itself (for `app.bsky.feed.post`: text, facets, reply).
    #[serde(default)]
    pub value: Option<PostRecord>,
    /// Media attached to the quoted post, as views.
    #[serde(default)]
    pub embeds: Option<Vec<EmbedView>>,
}

impl EmbedRecordView {
    pub fn quoted(&self) -> Option<&QuotedPost> {
        match self {
            Self::Record(quoted) => Some(quoted),
            _ => None,
        }
    }
}

/// `recordWithMedia#view.record` arrives in two shapes in the wild: the record
/// view itself, or a `{ "record": … }` wrapper with no `$type` on the wrapper.
/// Rejecting the second one blanks an entire feed, so accept both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RecordRef {
    View(EmbedRecordView),
    Wrapped { record: EmbedRecordView },
}

impl RecordRef {
    pub fn view(&self) -> &EmbedRecordView {
        match self {
            Self::View(view) => view,
            Self::Wrapped { record } => record,
        }
    }
}

/// The embed union as it appears in a `postView`, tagged by `$type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum EmbedView {
    #[serde(rename = "app.bsky.embed.images#view")]
    Images { images: Vec<ImageView> },
    #[serde(rename = "app.bsky.embed.gallery#view")]
    Gallery { items: Vec<GalleryItemView> },
    #[serde(rename = "app.bsky.embed.video#view")]
    Video(VideoView),
    #[serde(rename = "app.bsky.embed.external#view")]
    External { external: ExternalView },
    #[serde(rename = "app.bsky.embed.record#view")]
    Record { record: EmbedRecordView },
    #[serde(rename = "app.bsky.embed.recordWithMedia#view")]
    RecordWithMedia {
        record: RecordRef,
        media: Box<EmbedView>,
    },
    #[serde(other)]
    Unknown,
}

impl GalleryItemView {
    /// Gallery items carry the same alt/aspect data as images under different
    /// field names (`thumbnail` vs `thumb`), so the same component can render them.
    pub fn into_image_view(self) -> ImageView {
        ImageView {
            thumb: self.thumbnail,
            fullsize: self.fullsize,
            alt: self.alt,
            aspect_ratio: Some(self.aspect_ratio),
        }
    }
}

/// Why an item is in an author feed. Without this, reposts masquerade as the
/// feed owner's own posts — an attribution bug, not a cosmetic one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "$type")]
pub enum FeedViewReason {
    #[serde(rename = "app.bsky.feed.defs#reasonRepost")]
    Repost {
        by: Author,
        #[serde(default)]
        indexed_at: Option<String>,
    },
    #[serde(rename = "app.bsky.feed.defs#reasonPin")]
    Pin,
    #[serde(other)]
    Unknown,
}

/// `app.bsky.feed.getAuthorFeed` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedViewPost {
    pub post: PostView,
    #[serde(default)]
    pub reason: Option<FeedViewReason>,
}

impl FeedViewPost {
    /// The account that reposted this into the feed, if it is a repost.
    pub fn reposted_by(&self) -> Option<&Author> {
        match &self.reason {
            Some(FeedViewReason::Repost { by, .. }) => Some(by),
            _ => None,
        }
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self.reason, Some(FeedViewReason::Pin))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedResponse {
    #[serde(default)]
    pub feed: Vec<FeedViewPost>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn you_cannot_moderate_or_delete_someone_elses_post_and_cannot_moderate_your_own() {
        let me = Some("did:plc:me");
        assert_eq!(is_own_post(me, "did:plc:me"), true);
        assert_eq!(is_own_post(me, "did:plc:them"), false);
        assert_eq!(is_own_post(None, "did:plc:them"), false);

        // Not offered on your own post...
        assert_eq!(can_moderate_author(me, "did:plc:me"), false);
        // ...but offered on anyone else's.
        assert_eq!(can_moderate_author(me, "did:plc:them"), true);
        // Signed out, neither is possible.
        assert_eq!(can_moderate_author(None, "did:plc:them"), false);
    }

    #[test]
    fn record_key_reads_the_last_path_segment_of_an_at_uri() {
        // Deleting a record takes `collection` + `rkey`, so this is the piece that has to be
        // right: a wrong key deletes nothing and reports success-shaped nonsense.
        assert_eq!(
            record_key("at://did:plc:abc123/app.bsky.feed.post/3kxyz"),
            Some("3kxyz")
        );
        // Not an at-uri, or not a record uri.
        assert_eq!(record_key("https://bsky.app/profile/x/post/3kxyz"), None);
        assert_eq!(record_key("at://did:plc:abc123"), None);
        assert_eq!(record_key("at://did:plc:abc123/app.bsky.feed.post"), None);
        assert_eq!(record_key("at:///app.bsky.feed.post/3kxyz"), None);
        // Extra segments mean this is not a record uri.
        assert_eq!(record_key("at://did:plc:abc123/app.bsky.feed.post/3kxyz/extra"), None);
        assert_eq!(record_key("at://did:plc:abc123/app.bsky.feed.post/"), None);
    }

    use super::*;

    #[test]
    fn trending_topics_resolve_to_feed_generators() {
        let topic = TrendingTopic {
            topic: "Brazil votes".to_string(),
            display_name: Some("Brazil votes in presidential runoff".to_string()),
            description: Some("A tight race.".to_string()),
            link: "/profile/did:plc:exampletopic/feed/181f93831aa0".to_string(),
        };
        assert_eq!(topic.label(), "Brazil votes in presidential runoff");
        assert_eq!(
            topic.feed_uri().as_deref(),
            Some("at://did:plc:exampletopic/app.bsky.feed.generator/181f93831aa0")
        );

        // A link that is not a generator path must not become one.
        let bare = TrendingTopic {
            link: "/search?q=brazil".to_string(),
            ..topic.clone()
        };
        assert_eq!(bare.feed_uri(), None);
        let no_rkey = TrendingTopic {
            link: "/profile/did:plc:abc/feed/".to_string(),
            ..topic.clone()
        };
        assert_eq!(no_rkey.feed_uri(), None);
        // No display name: the topic is the headline.
        let plain = TrendingTopic {
            display_name: None,
            ..topic
        };
        assert_eq!(plain.label(), "Brazil votes");
    }

    fn reply_fixture(uri: &str, cid: &str, reply: Option<ReplyRef>) -> PostView {
        PostView {
            uri: uri.to_string(),
            cid: cid.to_string(),
            author: Author {
                did: "did:plc:author".to_string(),
                handle: "author.test".to_string(),
                display_name: None,
                avatar: None,
                labels: None,
            },
            record: PostRecord {
                text: Some("hello".to_string()),
                created_at: None,
                facets: None,
                reply,
            },
            viewer: None,
            embed: None,
            indexed_at: None,
            reply_count: None,
            repost_count: None,
            like_count: None,
            quote_count: None,
            bookmark_count: None,
            labels: None,
        }
    }

    fn strong(uri: &str, cid: &str) -> StrongRef {
        StrongRef { uri: uri.to_string(), cid: cid.to_string() }
    }

    fn labelled(val: &str) -> Option<Vec<Label>> {
        Some(vec![Label {
            val: val.to_string(),
            src: None,
            uri: None,
            cid: None,
        }])
    }

    #[test]
    fn account_level_labels_hide_posts_and_profiles_when_logged_out() {
        // The AppView puts `!no-unauthenticated` on the *author*, not the post, so
        // checking only the post's own labels would leak an opted-out account.
        let mut post = reply_fixture("at://did:plc:author/app.bsky.feed.post/x", "cid", None);
        assert!(!post.hidden_when_logged_out());
        post.author.labels = labelled(NO_UNAUTHENTICATED);
        assert!(post.hidden_when_logged_out());

        let mut profile = ProfileViewDetailed {
            did: "did:plc:author".to_string(),
            handle: "author.test".to_string(),
            display_name: None,
            description: None,
            avatar: None,
            banner: None,
            followers_count: None,
            follows_count: None,
            posts_count: None,
            created_at: None,
            links: None,
            pronouns: None,
            viewer: None,
            labels: None,
        };
        assert!(!profile.hidden_when_logged_out());
        profile.labels = labelled(NO_UNAUTHENTICATED);
        assert!(profile.hidden_when_logged_out());

        // Any other label is not this rule.
        let mut spam = reply_fixture("at://did:plc:author/app.bsky.feed.post/y", "cid", None);
        spam.author.labels = labelled("spam");
        assert!(!spam.hidden_when_logged_out());
    }

    #[test]
    fn replying_to_a_top_level_post_makes_it_its_own_root() {
        let post = reply_fixture("at://did:plc:author/app.bsky.feed.post/one", "cid-one", None);
        let refs = post.reply_refs();
        assert_eq!(refs.parent, strong(&post.uri, "cid-one"));
        assert_eq!(refs.root, refs.parent);
    }

    #[test]
    fn replying_to_a_reply_keeps_the_thread_root() {
        // Answering a comment must point root at the original post, not at the
        // comment: root == parent would fork the conversation.
        let root = strong("at://did:plc:author/app.bsky.feed.post/root", "cid-root");
        let post = reply_fixture(
            "at://did:plc:author/app.bsky.feed.post/two",
            "cid-two",
            Some(ReplyRef { root: root.clone(), parent: strong("at://did:plc:author/app.bsky.feed.post/one", "cid-one") }),
        );
        let refs = post.reply_refs();
        assert_eq!(refs.root, root);
        assert_eq!(refs.parent, strong(&post.uri, "cid-two"));
    }

    #[test]
    fn a_reply_without_a_root_still_replies_to_itself() {
        // Malformed but seen in the wild: no root. Falling back to the post keeps the
        // record valid instead of writing a reply with a null root.
        let post = reply_fixture(
            "at://did:plc:author/app.bsky.feed.post/three",
            "cid-three",
            None,
        );
        assert_eq!(post.reply_refs().root, strong(&post.uri, "cid-three"));
    }

    #[test]
    fn profile_viewer_state_is_read_when_present() {
        let blocked: ProfileViewDetailed = serde_json::from_value(serde_json::json!({
            "did": "did:plc:them",
            "handle": "them.test",
            "viewer": {
                "muted": false,
                "blocking": "at://did:plc:me/app.bsky.graph.block/3abc",
                "blockedBy": false
            }
        }))
        .unwrap();
        let viewer = blocked.viewer.expect("viewer present");
        assert_eq!(viewer.blocking.as_deref(), Some("at://did:plc:me/app.bsky.graph.block/3abc"));
        assert!(!viewer.muted);

        let muted: ProfileViewDetailed = serde_json::from_value(serde_json::json!({
            "did": "did:plc:them",
            "handle": "them.test",
            "viewer": { "muted": true }
        }))
        .unwrap();
        let viewer = muted.viewer.expect("viewer present");
        assert!(viewer.muted);
        assert_eq!(viewer.blocking, None);

        // A public read has no viewer at all, and must not fabricate one.
        let anon: ProfileViewDetailed = serde_json::from_value(serde_json::json!({
            "did": "did:plc:them",
            "handle": "them.test"
        }))
        .unwrap();
        assert_eq!(anon.viewer, None);
    }

    #[test]
    fn unfetched_replies_measures_what_the_depth_cut() {
        // The tree is complete: as many children as the count claims.
        assert_eq!(unfetched_replies(Some(5), 5), 0);
        // Everything past the depth was dropped, and the count is the only hint.
        assert_eq!(unfetched_replies(Some(5), 0), 5);
        assert_eq!(unfetched_replies(Some(198), 20), 178);
        // No count, nothing to claim: do not invent a junction.
        assert_eq!(unfetched_replies(None, 0), 0);
        // The API can return more than it counts (hidden replies); never negative.
        assert_eq!(unfetched_replies(Some(2), 5), 0);
        assert_eq!(unfetched_replies(Some(-1), 0), 0);
    }

    #[test]
    fn continue_thread_label_is_plural_aware() {
        // Nothing missing means no junction at all, not an empty button.
        assert_eq!(continue_thread_label(0), "");
        assert_eq!(continue_thread_label(1), "Continue thread (1 reply)");
        assert_eq!(continue_thread_label(7), "Continue thread (7 replies)");
    }

    #[test]
    fn discover_is_labelled_by_viewer_context() {
        // Saved name wins over everything: the account already decided.
        assert_eq!(discover_label(Some("Discover"), true), "Discover");
        assert_eq!(discover_label(Some("Mine"), true), "Mine");
        // Signed in, nothing saved: personalized ranking, so Discover.
        assert_eq!(discover_label(None, true), "Discover");
        // Anonymous: global trending, and the honest name for that is What's Hot.
        assert_eq!(discover_label(None, false), "What's Hot");
        // A blank saved label is not a name.
        assert_eq!(discover_label(Some("   "), false), "What's Hot");
    }

    #[test]
    fn tab_titles_use_the_resolved_feed_label() {
        // The bug this guards: Home hardcoding "What's Hot" while the reader has
        // Discover selected, i.e. the tab name and the tab title disagreeing.
        assert_eq!(
            tab_title(TitleSubject::Feed { label: "Discover" }, "App"),
            "Discover — App"
        );
        assert_eq!(
            tab_title(TitleSubject::Feed { label: "ALT4Me" }, "App"),
            "ALT4Me — App"
        );
        // An unresolved label still produces a usable title.
        assert_eq!(tab_title(TitleSubject::Feed { label: "  " }, "App"), "Feed — App");
        assert_eq!(tab_title(TitleSubject::Following, "App"), "Following — App");
        // The Feeds page is a destination of its own, not a feed with a label.
        assert_eq!(tab_title(TitleSubject::Feeds, "App"), "Feeds — App");
        assert_eq!(tab_title(TitleSubject::Settings, "App"), "Settings — App");
        assert_eq!(tab_title(TitleSubject::SignIn, "App"), "Sign in — App");
        assert_eq!(tab_title(TitleSubject::Explore, "App"), "Explore — App");
        assert_eq!(
            tab_title(TitleSubject::Notifications, "App"),
            "Notifications — App"
        );
        // Reasons the app knows read as sentences; anything else stays neutral.
        assert_eq!(notification_verb("like"), "liked your post");
        assert_eq!(notification_verb("follow"), "followed you");
        assert_eq!(notification_verb("something-new"), "interacted with you");
        // The who-liked / who-reposted pages name themselves.
        assert_eq!(
            tab_title(TitleSubject::Engagement { label: "Likes" }, "App"),
            "Likes — App"
        );
        assert_eq!(
            tab_title(TitleSubject::Profile { handle: "bsky.app" }, "App"),
            "@bsky.app — App"
        );
    }

    #[test]
    fn feed_failure_names_who_is_at_fault() {
        // 0 means the request never got an answer: offline / DNS / blocked.
        let offline = FeedFailure::parse(
            "HTTP 0 from https://api.bsky.app/x: TypeError: Load failed",
        );
        assert_eq!(offline.kind, FeedFailureKind::Unreachable);
        assert_eq!(offline.status, 0);
        assert_eq!(offline.detail, "TypeError: Load failed");

        // A generator that vanished reads as Missing, not as a network problem.
        let gone = FeedFailure::parse("HTTP 404 from https://x/y: {\"error\":\"notFound\"}");
        assert_eq!(gone.kind, FeedFailureKind::Missing);
        assert_eq!(gone.status, 404);

        assert_eq!(
            FeedFailure::parse("HTTP 502 from https://x/y: InternalServerError").kind,
            FeedFailureKind::ServerError
        );
        assert_eq!(
            FeedFailure::parse("HTTP 429 from https://x/y: rate limited").kind,
            FeedFailureKind::RateLimited
        );
        assert_eq!(
            FeedFailure::parse("HTTP 403 from https://x/y: forbidden").kind,
            FeedFailureKind::Refused
        );
        // Unrecognized text is not assumed to be a network failure.
        assert_eq!(
            FeedFailure::parse("something odd").kind,
            FeedFailureKind::Unknown
        );
        // Our own precondition must not masquerade as "can't reach Bluesky".
        assert_eq!(
            FeedFailure::parse("Sign in to load Following").kind,
            FeedFailureKind::Refused
        );
        // Headline and hint always exist, so the UI always has something to say.
        assert!(!offline.headline().is_empty() && !offline.hint().is_empty());
        // A missing post is not announced as a missing feed.
        assert_eq!(gone.headline_for(Subject::Post), "That post isn't available");
        assert_eq!(gone.headline_for(Subject::Feed), "That feed isn't available");
        assert_ne!(gone.hint_for(Subject::Post), gone.hint_for(Subject::Feed));
        // A profile's post list is neither a feed nor the profile itself.
        assert_eq!(
            gone.headline_for(Subject::AuthorFeed),
            "That account's posts aren't available"
        );
    }

    #[test]
    fn new_post_count_counts_only_the_unseen_head() {
        let known: std::collections::HashSet<String> =
            ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let uris = |list: &[&str]| -> Vec<String> { list.iter().map(|s| s.to_string()).collect() };
        // Three genuinely new posts above everything already read.
        assert_eq!(new_post_count(&uris(&["x", "y", "z", "a"]), &known), 3);
        // Same page: nothing to announce.
        assert_eq!(new_post_count(&uris(&["a", "b", "c"]), &known), 0);
        // A known post first means the list merely shifted; do not inflate it.
        assert_eq!(new_post_count(&uris(&["a", "x", "y"]), &known), 0);
    }

    #[test]
    fn feed_end_note_marks_the_final_page_only_when_posts_were_shown() {
        // Still more to fetch: quiet while idle, a spinner note while in flight.
        assert_eq!(feed_end_note(30, true, false), None);
        assert_eq!(feed_end_note(30, true, true), Some("Loading more…"));
        // Cursor gone with posts on screen means the feed genuinely ended.
        assert_eq!(
            feed_end_note(30, false, false),
            Some("You've reached the end of this feed.")
        );
        // Nothing to say about a feed that never returned anything.
        assert_eq!(feed_end_note(0, false, false), None);
    }

    /// A real `app.bsky.embed.images#view` payload as returned by the AppView.
    const IMAGES_VIEW: &str = r#"{
        "$type": "app.bsky.embed.images#view",
        "images": [{
            "thumb": "https://cdn.bsky.app/img/feed_thumbnail/plain/did:plc:abc/bafyrei@jpeg",
            "fullsize": "https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:abc/bafyrei@jpeg",
            "alt": "A black cat asleep on a laptop keyboard",
            "aspectRatio": {"width": 1200, "height": 800}
        }]
    }"#;

    /// A real `app.bsky.embed.video#view` payload, with no alt text (allowed).
    const VIDEO_VIEW: &str = r#"{
        "$type": "app.bsky.embed.video#view",
        "cid": "bafyreib2rxk3rh6kzwq",
        "playlist": "https://video.bsky.app/watch/did%3Aplc%3Aabc/bafyrei/playlist.m3u8",
        "thumbnail": "https://video.bsky.app/watch/did%3Aplc%3Aabc/bafyrei/thumbnail.jpg",
        "aspectRatio": {"width": 1080, "height": 1920}
    }"#;

    #[test]
    fn deserializes_image_view_alt_text() {
        let view: serde_json::Value = serde_json::from_str(IMAGES_VIEW).unwrap();
        let image: ImageView =
            serde_json::from_value(view["images"][0].clone()).expect("image view");

        assert_eq!(image.alt, "A black cat asleep on a laptop keyboard");
        assert!(alt_policy(&image.alt).is_described());
        assert_eq!(placeholder_ratio(image.aspect_ratio.as_ref()), 1.5);
    }

    #[test]
    fn video_alt_is_optional() {
        let video: VideoView = serde_json::from_str(VIDEO_VIEW).expect("video view");

        assert_eq!(video.alt, None);
        assert_eq!(
            video_alt_policy(video.alt.as_deref()),
            AltPolicy::UndescribedPrompt
        );
        // 1080x1920 is 9:16 = 0.5625, which is inside the 1:2 feed bound.
        assert_eq!(placeholder_ratio(video.aspect_ratio.as_ref()), 0.5625);
        // Anything taller than 1:2 is clamped so the timeline stays usable.
        assert_eq!(
            placeholder_ratio(Some(&AspectRatio {
                width: 500,
                height: 2000
            })),
            MIN_FEED_RATIO
        );
    }

    #[test]
    fn blank_alt_is_not_described() {
        assert_eq!(alt_policy(""), AltPolicy::UndescribedPrompt);
        assert_eq!(alt_policy("   \n\t"), AltPolicy::UndescribedPrompt);
        assert_eq!(alt_policy("chart of signups"), AltPolicy::AltThenMedia);
    }

    #[test]
    fn degenerate_aspect_ratios_fall_back_to_default() {
        assert_eq!(placeholder_ratio(None), DEFAULT_RATIO);
        assert_eq!(
            placeholder_ratio(Some(&AspectRatio {
                width: 0,
                height: 100
            })),
            DEFAULT_RATIO
        );
        // Wider than 16:9 keeps its own ratio; nothing clamps the upper bound.
        assert_eq!(
            placeholder_ratio(Some(&AspectRatio {
                width: 2000,
                height: 500
            })),
            4.0
        );
    }

    #[test]
    fn gallery_item_requires_aspect_ratio() {
        let json = r#"{
            "thumbnail": "https://cdn.bsky.app/img/feed_thumbnail/plain/did:plc:abc/bafy@jpeg",
            "fullsize": "https://cdn.bsky.app/img/feed_fullsize/plain/did:plc:abc/bafy@jpeg",
            "alt": "four panels of a comic",
            "aspectRatio": {"width": 1000, "height": 1000}
        }"#;
        let item: GalleryItemView = serde_json::from_str(json).expect("gallery item");
        assert_eq!(placeholder_ratio(Some(&item.aspect_ratio)), 1.0);
    }
}


/// These tests run against a real `getAuthorFeed` response captured from
/// `https://api.bsky.app` (see `scripts/probe_public_api.py`), so a
/// schema change in the AppView shows up as a failing test rather than a blank UI.
#[cfg(test)]
mod feed_tests {
    use super::*;

    const AUTHOR_FEED: &str = include_str!("../../scripts/fixtures/author_feed.json");

    #[test]
    fn parses_a_real_public_author_feed() {
        let feed: FeedResponse = serde_json::from_str(AUTHOR_FEED).expect("author feed");
        // The fixture is refreshed by scripts/probe_public_api.py, so assert on
        // shape rather than an exact count.
        assert!(feed.feed.len() >= 5, "fixture looks too small");

        let described_images = feed
            .feed
            .iter()
            .filter_map(|item| item.post.embed.as_ref())
            .filter_map(|embed| match embed {
                EmbedView::Images { images } => Some(images),
                _ => None,
            })
            .flatten()
            .filter(|image| alt_policy(&image.alt).is_described())
            .count();

        assert!(described_images > 0, "expected at least one described image");

        // Every embed in this fixture is one this client can render. If the
        // AppView starts returning a new $type, this fails loudly.
        let unknown = feed
            .feed
            .iter()
            .filter(|item| matches!(item.post.embed, Some(EmbedView::Unknown)))
            .count();
        assert_eq!(unknown, 0, "unhandled embed $type in the fixture");
    }

    #[test]
    fn parses_quote_posts_from_the_fixture() {
        let feed: FeedResponse = serde_json::from_str(AUTHOR_FEED).expect("author feed");
        let quotes: Vec<&QuotedPost> = feed
            .feed
            .iter()
            .filter_map(|item| item.post.embed.as_ref())
            .filter_map(|embed| match embed {
                EmbedView::Record { record } => record.quoted(),
                EmbedView::RecordWithMedia { record, .. } => record.view().quoted(),
                _ => None,
            })
            .collect();

        assert!(!quotes.is_empty(), "fixture should contain quote posts");
        for quoted in quotes {
            assert!(!quoted.author.handle.is_empty());
            assert!(quoted.uri.starts_with("at://"));
        }
    }

    #[test]
    fn record_with_media_accepts_the_wrapped_shape() {
        let json = serde_json::json!({
            "$type": "app.bsky.embed.recordWithMedia#view",
            "media": {
                "$type": "app.bsky.embed.images#view",
                "images": [{
                    "thumb": "https://cdn.example/t.jpg",
                    "fullsize": "https://cdn.example/f.jpg",
                    "alt": "a chart"
                }]
            },
            "record": {
                "record": {
                    "$type": "app.bsky.embed.record#viewRecord",
                    "uri": "at://did:plc:x/app.bsky.feed.post/abc",
                    "cid": "bafy",
                    "author": {"did": "did:plc:x", "handle": "x.example"},
                    "value": {"$type": "app.bsky.feed.post", "text": "quoted"}
                }
            }
        });

        let embed: EmbedView = serde_json::from_value(json).expect("recordWithMedia");
        let EmbedView::RecordWithMedia { record, media } = embed else {
            panic!("wrong variant");
        };
        let quoted = record.view().quoted().expect("quoted post");
        assert_eq!(quoted.author.handle, "x.example");
        assert!(matches!(*media, EmbedView::Images { .. }));
    }

    #[test]
    fn record_with_media_accepts_the_direct_shape() {
        let json = serde_json::json!({
            "$type": "app.bsky.embed.recordWithMedia#view",
            "media": {"$type": "app.bsky.embed.images#view", "images": []},
            "record": {
                "$type": "app.bsky.embed.record#viewRecord",
                "uri": "at://did:plc:x/app.bsky.feed.post/abc",
                "cid": "bafy",
                "author": {"did": "did:plc:x", "handle": "x.example"}
            }
        });
        let embed: EmbedView = serde_json::from_value(json).expect("direct");
        let EmbedView::RecordWithMedia { record, .. } = embed else {
            panic!("wrong variant");
        };
        assert!(record.view().quoted().is_some());
    }

    #[test]
    fn reposts_carry_their_attribution() {
        let feed: FeedResponse = serde_json::from_str(AUTHOR_FEED).expect("author feed");
        let reposts: Vec<&FeedViewPost> = feed
            .feed
            .iter()
            .filter(|item| item.reposted_by().is_some())
            .collect();

        assert!(!reposts.is_empty(), "fixture should contain reposts");
        for item in reposts {
            let by = item.reposted_by().expect("reposter");
            assert!(!by.handle.is_empty());
            // The post author is someone else — that difference is exactly what
            // the feed UI has to show, otherwise the repost looks like the account's
            // own post.
            assert_ne!(item.post.author.did, by.did);
        }
        assert!(
            feed.feed.iter().any(|item| item.reposted_by().is_none()),
            "fixture should also contain the account's own posts"
        );
    }

    #[test]
    fn video_alt_and_presentation_survive_roundtrip() {
        let feed: FeedResponse = serde_json::from_str(AUTHOR_FEED).expect("author feed");
        let videos: Vec<&VideoView> = feed
            .feed
            .iter()
            .filter_map(|item| item.post.embed.as_ref())
            .filter_map(|embed| match embed {
                EmbedView::Video(video) => Some(video),
                _ => None,
            })
            .collect();

        assert!(!videos.is_empty(), "fixture should contain video embeds");
        assert!(
            videos.iter().any(|video| video.alt.is_some()),
            "fixture should contain a described video"
        );
        assert!(
            videos.iter().any(|video| video.presentation.as_deref() == Some("gif")),
            "fixture should contain a GIF-presentation video"
        );
        // The playlist is HLS, which is why the video component needs an HLS shim.
        assert!(videos.iter().all(|video| video.playlist.ends_with(".m3u8")));
    }
}


#[cfg(test)]
mod facet_tests {
    use super::*;
    use serde_json::json;

    fn facet(start: usize, end: usize, feature: serde_json::Value) -> Facet {
        serde_json::from_value(json!({
            "index": {"byteStart": start, "byteEnd": end},
            "features": [feature],
        }))
        .expect("facet")
    }

    fn mention(start: usize, end: usize) -> Facet {
        facet(
            start,
            end,
            json!({"$type": "app.bsky.richtext.facet#mention", "did": "did:plc:alice"}),
        )
    }

    #[test]
    fn mention_offsets_are_bytes_not_chars() {
        // "🦋" is 4 UTF-8 bytes, plus one space: the mention starts at byte 5,
        // while a character-index implementation would say 2.
        let text = "🦋 @alice.bsky.social hi";
        let facets = vec![mention(5, 23)];

        let segments = text_segments(text, Some(&facets));
        assert_eq!(
            segments,
            vec![
                TextSegment::Plain("🦋 "),
                TextSegment::Mention {
                    text: "@alice.bsky.social",
                    did: "did:plc:alice"
                },
                TextSegment::Plain(" hi"),
            ]
        );
    }

    #[test]
    fn link_facets_mark_the_url_and_nothing_else() {
        let url = "https://bennett-elder.github.io/rust-frontend-for-bluesky/";
        let text = format!("sharing {url} today");
        let facets = link_facets(&text);
        assert_eq!(facets.len(), 1);
        assert_eq!(facets[0].index.byte_start, text.find("https").unwrap());
        assert_eq!(&text[facets[0].index.byte_end..], " today");
        assert_eq!(
            facets[0].features,
            vec![FacetFeature::Link {
                uri: url.to_string()
            }]
        );
    }

    #[test]
    fn link_facets_do_not_swallow_the_punctuation_that_ends_the_sentence() {
        let text = "see https://example.test/a.";
        let facet = &link_facets(text)[0];
        assert_eq!(&text[facet.index.byte_start..facet.index.byte_end], "https://example.test/a");

        let comma = "https://example.test/a, and more";
        let facet = &link_facets(comma)[0];
        assert_eq!(
            &comma[facet.index.byte_start..facet.index.byte_end],
            "https://example.test/a"
        );
    }

    #[test]
    fn link_facets_count_bytes_rather_than_characters() {
        // The emoji is four bytes: a char index here would put the facet inside the URL's
        // scheme and break every reader that trusted it.
        let text = "👋 https://example.test";
        let facet = &link_facets(text)[0];
        assert_eq!(facet.index.byte_start, text.find("https").unwrap());
        assert_ne!(facet.index.byte_start, text.chars().position(|c| c == 'h').unwrap());
        assert_eq!(
            &text[facet.index.byte_start..facet.index.byte_end],
            "https://example.test"
        );
    }

    #[test]
    fn link_facets_keep_balanced_parentheses() {
        let wiki = "https://en.wikipedia.org/wiki/Foo_(bar)";
        let facet = &link_facets(wiki)[0];
        assert_eq!(facet.index.byte_end, wiki.len());
        assert_eq!(
            facet.features,
            vec![FacetFeature::Link {
                uri: wiki.to_string()
            }]
        );

        let wrapped = "(see https://example.test/x)";
        let facet = &link_facets(wrapped)[0];
        assert_eq!(
            &wrapped[facet.index.byte_start..facet.index.byte_end],
            "https://example.test/x"
        );
    }

    #[test]
    fn link_facets_find_every_link_in_order() {
        let text = "a https://one.test b http://two.test/c";
        let facets = link_facets(text);
        assert_eq!(facets.len(), 2);
        assert_eq!(&text[facets[0].index.byte_start..facets[0].index.byte_end], "https://one.test");
        assert_eq!(
            &text[facets[1].index.byte_start..facets[1].index.byte_end],
            "http://two.test/c"
        );
    }

    #[test]
    fn link_facets_accept_an_upper_case_scheme_and_keep_the_original_text() {
        let text = "HTTPS://Example.test/Path";
        let facets = link_facets(text);
        assert_eq!(facets.len(), 1);
        assert_eq!(
            facets[0].features,
            vec![FacetFeature::Link {
                uri: text.to_string()
            }]
        );
    }

    #[test]
    fn link_facets_ignore_what_is_not_a_link() {
        assert!(link_facets("no links here").is_empty());
        assert!(link_facets("http://").is_empty());
        assert!(link_facets("https:///path-only").is_empty());
        assert!(link_facets("xhttp://example.test").is_empty());
    }

    #[test]
    fn link_facets_need_a_scheme_and_a_dotted_host() {
        // The official client also links bare domains (`bsky.social` becomes
        // `https://bsky.social`); this scanner does not, so a test claiming otherwise would be
        // lying. Recorded here so the gap is visible in the suite rather than in a post that
        // silently came out unlinked.
        assert!(link_facets("see bsky.social for details").is_empty());
        // And a localhost URL is outside what this client posts, so it stays plain text.
        assert!(link_facets("running on localhost:8099").is_empty());
        assert!(link_facets("running on http://localhost:8099/").is_empty());
    }

    #[test]
    fn facet_that_splits_a_character_is_ignored() {
        let text = "🦋 hi";
        let facets = vec![mention(1, 3)]; // inside the butterfly
        assert_eq!(
            text_segments(text, Some(&facets)),
            vec![TextSegment::Plain(text)]
        );
    }

    #[test]
    fn out_of_bounds_facets_are_ignored() {
        let text = "@a";
        let facets = vec![mention(0, 99)];
        assert_eq!(
            text_segments(text, Some(&facets)),
            vec![TextSegment::Plain(text)]
        );
    }

    #[test]
    fn overlapping_facets_keep_the_first() {
        let text = "abcdef";
        let facets = vec![
            facet(
                0,
                3,
                json!({"$type": "app.bsky.richtext.facet#link", "uri": "https://a.example"}),
            ),
            facet(
                2,
                5,
                json!({"$type": "app.bsky.richtext.facet#tag", "tag": "def"}),
            ),
        ];

        let segments = text_segments(text, Some(&facets));
        assert_eq!(
            segments,
            vec![
                TextSegment::Link {
                    text: "abc",
                    uri: "https://a.example"
                },
                TextSegment::Plain("def"),
            ]
        );
    }

    #[test]
    fn unknown_features_are_ignored_and_text_survives() {
        let text = "hello world";
        let facets = vec![facet(0, 5, json!({"$type": "example.thing#unknown"}))];
        assert_eq!(
            text_segments(text, Some(&facets)),
            vec![TextSegment::Plain(text)]
        );
    }

    /// Real thread captured from `getPostThread` (209 nodes, 11 posts with facets).
    #[test]
    fn real_thread_facets_land_on_text() {
        const THREAD: &str = include_str!("../../scripts/fixtures/thread.json");

        fn walk(node: &serde_json::Value, mentions: &mut usize, links: &mut usize, tags: &mut usize) {
            if let Some(record) = node.get("post").and_then(|post| post.get("record")) {
                let text = record.get("text").and_then(|text| text.as_str());
                if let (Some(text), Some(raw_facets)) =
                    (text, record.get("facets").and_then(|facets| facets.as_array()))
                {
                    let facets: Vec<Facet> = serde_json::from_value(raw_facets.clone().into())
                        .expect("facets");
                    for segment in text_segments(text, Some(&facets)) {
                        match segment {
                            TextSegment::Mention { text, did } => {
                                assert!(text.starts_with('@'), "mention was {text:?}");
                                assert!(did.starts_with("did:"), "did was {did:?}");
                                *mentions += 1;
                            }
                            TextSegment::Link { text, uri } => {
                                assert!(!text.is_empty());
                                assert!(uri.starts_with("http"), "uri was {uri:?}");
                                *links += 1;
                            }
                            TextSegment::Tag { text, tag } => {
                                assert!(!text.is_empty() && !tag.is_empty());
                                *tags += 1;
                            }
                            TextSegment::Plain(_) => {}
                        }
                    }
                }
            }

            for child in node
                .get("replies")
                .and_then(|replies| replies.as_array())
                .into_iter()
                .flatten()
            {
                walk(child, mentions, links, tags);
            }
        }

        let root: serde_json::Value = serde_json::from_str(THREAD).expect("thread fixture");
        let (mut mentions, mut links, mut tags) = (0, 0, 0);
        walk(&root["thread"], &mut mentions, &mut links, &mut tags);

        assert!(mentions > 0, "expected mentions in the fixture");
        assert!(links > 0, "expected links in the fixture");
        assert!(tags > 0, "expected tags in the fixture");
    }
}


#[cfg(test)]
mod thread_tests {
    use super::*;

    const THREAD: &str = include_str!("../../scripts/fixtures/thread.json");

    fn reply_edges(node: &ThreadViewNode, out: &mut usize) {
        if let ThreadViewNode::Post(view) = node {
            if view.post.record.reply.is_some() {
                *out += 1;
            }
            for child in view.replies.as_deref().unwrap_or_default() {
                reply_edges(child, out);
            }
        }
    }

    fn count_placeholders(node: &ThreadViewNode, blocked: &mut usize, missing: &mut usize) {
        match node {
            ThreadViewNode::Post(view) => {
                for child in view.replies.as_deref().unwrap_or_default() {
                    count_placeholders(child, blocked, missing);
                }
            }
            ThreadViewNode::Blocked { .. } => *blocked += 1,
            ThreadViewNode::NotFound { .. } => *missing += 1,
            ThreadViewNode::Unknown => {}
        }
    }

    #[test]
    fn parses_the_thread_fixture() {
        let response: PostThreadResponse = serde_json::from_str(THREAD).expect("thread");
        // Synthetic and stable, so this can assert structure exactly: a root, nested
        // replies, and both kinds of placeholder the renderer has to handle.
        assert!(response.thread.count_nodes() >= 5);
        assert!(response.thread.post().is_some());

        let mut edges = 0;
        reply_edges(&response.thread, &mut edges);
        assert!(edges >= 3, "expected reply refs, got {edges}");

        let mut blocked = 0;
        let mut missing = 0;
        count_placeholders(&response.thread, &mut blocked, &mut missing);
        assert!(blocked >= 1, "fixture should carry a blocked node");
        assert!(missing >= 1, "fixture should carry a missing node");
    }

    #[test]
    fn reply_refs_point_at_root_and_parent() {
        let response: PostThreadResponse = serde_json::from_str(THREAD).expect("thread");
        let mut checked = 0;

        fn walk(node: &ThreadViewNode, checked: &mut usize) {
            let ThreadViewNode::Post(view) = node else {
                return;
            };
            if let Some(reply) = &view.post.record.reply {
                assert!(reply.root.uri.starts_with("at://"));
                assert!(reply.parent.uri.starts_with("at://"));
                assert!(!reply.root.cid.is_empty());
                *checked += 1;
            }
            for child in view.replies.as_deref().unwrap_or_default() {
                walk(child, checked);
            }
        }

        walk(&response.thread, &mut checked);
        assert!(checked > 0);
    }

    #[test]
    fn blocked_and_deleted_nodes_are_preserved() {
        // Not present in this fixture, so pin the wire shapes explicitly.
        let blocked: ThreadViewNode = serde_json::from_value(serde_json::json!({
            "$type": "app.bsky.feed.defs#blockedPost",
            "uri": "at://did:plc:x/app.bsky.feed.post/abc",
            "blocked": true,
        }))
        .expect("blocked");
        assert!(matches!(blocked, ThreadViewNode::Blocked { .. }));
        assert_eq!(blocked.count_nodes(), 1);

        let gone: ThreadViewNode = serde_json::from_value(serde_json::json!({
            "$type": "app.bsky.feed.defs#notFoundPost",
            "uri": "at://did:plc:x/app.bsky.feed.post/def",
            "notFound": true,
        }))
        .expect("notFound");
        assert!(matches!(gone, ThreadViewNode::NotFound { .. }));
        assert!(gone.post().is_none());
    }

    #[test]
    fn count_nodes_walks_whole_tree() {
        let leaf = serde_json::json!({
            "$type": "app.bsky.feed.defs#notFoundPost",
            "uri": "at://x",
            "notFound": true,
        });
        let view = serde_json::json!({
            "$type": "app.bsky.feed.defs#threadViewPost",
            "post": {
                "uri": "at://did:plc:x/app.bsky.feed.post/root",
                "author": {"did": "did:plc:x", "handle": "x.example"},
                "record": {"text": "hi"},
            },
            "replies": [leaf.clone(), leaf.clone()],
        });
        let node: ThreadViewNode = serde_json::from_value(view).expect("tree");
        assert_eq!(node.count_nodes(), 3);
    }
}


#[cfg(test)]
mod preferences_tests {
    use super::*;

    const PREFS: &str = r#"{
        "preferences": [
            {"$type": "app.bsky.actor.defs#adultContentPref", "enabled": false},
            {"$type": "app.bsky.actor.defs#savedFeedsPrefV2", "items": [
                {"id": "1", "type": "timeline", "value": "following", "pinned": true},
                {"id": "2", "type": "feed", "value": "at://did:plc:aaa/app.bsky.feed.generator/one", "pinned": true},
                {"id": "3", "type": "feed", "value": "at://did:plc:bbb/app.bsky.feed.generator/two", "pinned": false},
                {"id": "4", "type": "feed", "value": "at://did:plc:aaa/app.bsky.feed.generator/one", "pinned": false}
            ]},
            {"$type": "app.bsky.actor.defs#threadViewPref", "sort": "oldest"}
        ]
    }"#;

    #[test]
    fn saved_feeds_are_pinned_first_deduped_and_skip_the_timeline() {
        let prefs: PreferencesResponse = serde_json::from_str(PREFS).expect("prefs");
        assert_eq!(
            prefs.saved_feed_uris(),
            vec![
                "at://did:plc:aaa/app.bsky.feed.generator/one".to_string(),
                "at://did:plc:bbb/app.bsky.feed.generator/two".to_string(),
            ]
        );
    }

    #[test]
    fn legacy_saved_feeds_still_parse() {
        let prefs: PreferencesResponse = serde_json::from_str(
            r#"{"preferences": [{"$type": "app.bsky.actor.defs#savedFeedsPref",
                 "pinned": ["at://did:plc:a/app.bsky.feed.generator/p"],
                 "saved": ["at://did:plc:b/app.bsky.feed.generator/s"]}]}"#,
        )
        .expect("legacy prefs");
        assert_eq!(
            prefs.saved_feed_uris(),
            vec![
                "at://did:plc:a/app.bsky.feed.generator/p".to_string(),
                "at://did:plc:b/app.bsky.feed.generator/s".to_string(),
            ]
        );
    }

    #[test]
    fn unknown_preferences_do_not_break_parsing() {
        let prefs: PreferencesResponse = serde_json::from_str(
            r#"{"preferences": [{"$type": "com.example.thing#pref", "x": 1}]}"#,
        )
        .expect("unknown pref");
        assert!(prefs.saved_feed_uris().is_empty());
    }

    #[test]
    fn feed_generator_names_are_read_for_tabs() {
        let response: FeedGeneratorsResponse = serde_json::from_str(
            r#"{"feeds": [
                {"uri": "at://did:plc:a/app.bsky.feed.generator/one", "displayName": "ALT4Me--Alt Requests",
                 "creator": {"did": "did:plc:a", "handle": "bsky.app"}},
                {"uri": "at://did:plc:b/app.bsky.feed.generator/two"}
            ]}"#,
        )
        .expect("generators");
        assert_eq!(response.feeds[0].display_name.as_deref(), Some("ALT4Me--Alt Requests"));
        assert_eq!(
            response.feeds[0].creator.as_ref().map(|c| c.handle.as_str()),
            Some("bsky.app")
        );
        assert!(response.feeds[1].display_name.is_none());
    }
}


#[cfg(test)]
mod profile_tests {
    use super::*;

    const PROFILE: &str = include_str!("../../scripts/fixtures/profile.json");

    #[test]
    fn parses_the_profile_fixture() {
        let profile: ProfileViewDetailed = serde_json::from_str(PROFILE).expect("profile");
        // Synthetic fixture (scripts/make_fixtures.py): assert the shape, not some
        // particular account's biography.
        assert!(profile.did.starts_with("did:plc:"));
        assert!(!profile.handle.is_empty());
        assert_eq!(profile.display(), "Example Person");
        assert!(profile.avatar.is_some());
        assert!(profile.banner.is_some());
        assert!(!profile.description.clone().unwrap_or_default().trim().is_empty());
        assert!(profile.followers_count.unwrap_or(0) > 0);
        assert!(profile.posts_count.unwrap_or(0) > 0);
    }

    #[test]
    fn display_falls_back_to_handle_when_the_name_is_blank() {
        let mut profile: ProfileViewDetailed = serde_json::from_str(PROFILE).expect("profile");
        let handle = profile.handle.clone();
        profile.display_name = Some("   ".to_string());
        assert_eq!(profile.display(), handle);
        profile.display_name = None;
        assert_eq!(profile.display(), handle);
    }

    #[test]
    fn profile_links_parse_and_reject_unsafe_schemes() {
        // Fixture is a public account (pfrazee.com) so no one's own profile is
        // baked into the test data.
        const LINKED: &str = include_str!("../../scripts/fixtures/profile_links.json");
        let profile: ProfileViewDetailed = serde_json::from_str(LINKED).expect("profile");
        let links = profile.links.expect("links");
        assert!(!links.is_empty(), "fixture should carry at least one link");
        assert!(links[0].url.starts_with("https://"));
        assert!(!links[0].title.trim().is_empty());
        assert!(links.iter().all(ProfileLink::is_safe));

        let unsafe_link = ProfileLink {
            uri: "at://x".into(),
            cid: String::new(),
            url: "javascript:alert(1)".into(),
            title: "nope".into(),
            icon: None,
        };
        assert!(!unsafe_link.is_safe());
    }

    #[test]
    fn sparse_profiles_parse() {
        // A minimal account: no banner, no description, no counts.
        let profile: ProfileViewDetailed = serde_json::from_str(
            r#"{"did": "did:plc:x", "handle": "x.example"}"#,
        )
        .expect("sparse profile");
        assert_eq!(profile.display(), "x.example");
        assert!(profile.followers_count.is_none());
        assert!(profile.banner.is_none());
    }
}


#[cfg(test)]
mod url_encoding_tests {
    use super::*;

    const SAMPLES: &[&str] = &[
        "at://did:plc:z72i7hdynmk6r22z27h6tvur/app.bsky.feed.generator/whats-hot",
        "at://did:plc:x/app.bsky.feed.post/3mxwsrbliic2f",
        "jay.bsky.team",
        // Cursor-shaped: base64 alphabet plus padding, which is what broke paging.
        "3lyr5g2bq2c2b+abc/def==",
        // Characters that are structural in a URL.
        "a b+c/d?e=f&g#h%i",
        // Multi-byte, to prove byte-level encoding round-trips.
        "🦋 alt text",
    ];

    #[test]
    fn every_sample_round_trips() {
        for sample in SAMPLES {
            let encoded = encode_uri_component(sample);
            assert_eq!(decode_uri_component(&encoded), *sample, "sample: {sample}");
        }
    }

    #[test]
    fn structural_characters_are_encoded() {
        assert_eq!(
            encode_uri_component("at://did:plc:x/app.bsky.feed.post/3abc"),
            "at%3A%2F%2Fdid%3Aplc%3Ax%2Fapp.bsky.feed.post%2F3abc"
        );

        for dangerous in ['/', ':', '+', '=', '&', '?', '#'] {
            assert!(
                !encode_uri_component(&format!("a{dangerous}b")).contains(dangerous),
                "{dangerous} must not survive encoding"
            );
        }
    }

    #[test]
    fn unreserved_characters_stay_literal() {
        assert_eq!(encode_uri_component("aZ0-._~"), "aZ0-._~");
    }

    #[test]
    fn decoding_a_malformed_value_does_not_panic() {
        // A truncated escape and invalid UTF-8 both degrade instead of crashing.
        assert_eq!(decode_uri_component("abc%"), "abc%");
        assert!(!decode_uri_component("%FF%FE").is_empty());
    }
}

