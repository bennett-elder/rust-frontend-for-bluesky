#!/usr/bin/env python3
"""Generate the JSON fixtures the Rust tests parse.

Why synthetic: the fixtures used to be captured AppView payloads from specific accounts,
which baked real third-party DIDs into the repository and made the tests depend on how
those accounts happened to look that day. These are deterministic, offline, and use only
`did:plc:example*` identifiers — while still exercising the shapes that matter: facets,
videos (with alt and GIF presentation), quotes, reposts, nested replies, blocked and
missing nodes.

`scripts/probe_public_api.py` still talks to the live AppView, for inspecting real
payloads; it writes to `.tmp/probe/` rather than overwriting these.

    python3 scripts/make_fixtures.py
"""

from __future__ import annotations

import json
import pathlib

FIXTURES = pathlib.Path(__file__).resolve().parent / "fixtures"
MEDIA = "https://media.example.test"
ROOT_DID = "did:plc:exampleauthor"
OTHER_DID = "did:plc:examplereplier"
POSTS = "at://did:plc:exampleauthor/app.bsky.feed.post"


def author(did: str, handle: str, name: str | None = None) -> dict:
    return {"did": did, "handle": handle, "displayName": name, "avatar": f"{MEDIA}/avatar/{handle}.jpg"}


def images_view(*alts: str | None) -> dict:
    """One image per argument; the argument is its alt text (None means undescribed)."""
    return {
        "$type": "app.bsky.embed.images#view",
        "images": [
            {"thumb": f"{MEDIA}/thumb/{index}.jpg", "fullsize": f"{MEDIA}/full/{index}.jpg", "alt": alt or ""}
            for index, alt in enumerate(alts)
        ],
    }


def video_view(*, alt: str | None, presentation: str | None = None) -> dict:
    view = {
        "$type": "app.bsky.embed.video#view",
        "cid": "bafyexamplevideo",
        "playlist": "https://video.example.test/playlist.m3u8",
        "thumbnail": f"{MEDIA}/thumb/video.jpg",
        "aspectRatio": {"width": 1280, "height": 720},
    }
    if alt is not None:
        view["alt"] = alt
    if presentation is not None:
        view["presentation"] = presentation
    return view


def external_view(uri: str, title: str, description: str) -> dict:
    return {
        "$type": "app.bsky.embed.external#view",
        "external": {"uri": uri, "title": title, "description": description, "thumb": f"{MEDIA}/thumb/link.jpg"},
    }


def record_view(uri: str) -> dict:
    return {
        "$type": "app.bsky.embed.record#view",
        "record": {
            "$type": "app.bsky.embed.record#viewRecord",
            "uri": uri,
            "cid": "bafyexamplequoted",
            "author": author(OTHER_DID, "replier.test", "Someone Else"),
            "value": {"$type": "app.bsky.feed.post", "text": "The quoted post", "createdAt": "2026-01-01T00:00:00.000Z"},
            "indexedAt": "2026-01-01T00:00:01.000Z",
            "labels": [],
        },
    }


def span(text: str, needle: str) -> dict:
    """A facet index in BYTES, the way the protocol means it."""
    start = text.encode().find(needle.encode())
    if start < 0:
        raise SystemExit(f"{needle!r} not in {text!r}")
    return {"byteStart": start, "byteEnd": start + len(needle.encode())}


def facet(text: str, needle: str, feature: dict) -> dict:
    return {"index": span(text, needle), "features": [feature]}


def post(uri: str, *, text: str, created: str, embed: dict | None = None,
         reply: dict | None = None, facets: list | None = None,
         counts: tuple[int, int, int] = (2, 3, 4), labels: list | None = None) -> dict:
    record: dict = {"$type": "app.bsky.feed.post", "text": text, "createdAt": created}
    if facets:
        record["facets"] = facets
    if reply:
        record["reply"] = reply
    view = {
        "uri": uri,
        "cid": "bafyexamplecid",
        "author": author(ROOT_DID, "example.test", "Example Person"),
        "record": record,
        "indexedAt": created,
        "replyCount": counts[0],
        "repostCount": counts[1],
        "likeCount": counts[2],
        "quoteCount": 1,
        "bookmarkCount": 0,
        "labels": labels or [],
    }
    if embed:
        view["embed"] = embed
    return view


def build_author_feed() -> dict:
    text = "A post with a mention @replier.test, a link https://example.com/page and a #tag"
    facets = [
        facet(text, "@replier.test", {"$type": "app.bsky.richtext.facet#mention", "did": OTHER_DID}),
        facet(text, "https://example.com/page", {"$type": "app.bsky.richtext.facet#link", "uri": "https://example.com/page"}),
        facet(text, "#tag", {"$type": "app.bsky.richtext.facet#tag", "tag": "tag"}),
    ]
    items = [
        {"post": post(f"{POSTS}/facet", text=text, created="2026-01-02T03:04:05.000Z", facets=facets)},
        {"post": post(f"{POSTS}/image-alt", text="Described image", created="2026-01-02T03:05:05.000Z",
                      embed=images_view("A described image"))},
        {"post": post(f"{POSTS}/image-no-alt", text="No description", created="2026-01-02T03:06:05.000Z",
                      embed=images_view(None) )},
        {"post": post(f"{POSTS}/video", text="A video with alt", created="2026-01-02T03:07:05.000Z",
                      embed=video_view(alt="A described clip"))},
        {"post": post(f"{POSTS}/video-no-alt", text="A silent loop", created="2026-01-02T03:08:05.000Z",
                      embed=video_view(alt=None))},
        {"post": post(f"{POSTS}/gif", text="A loop", created="2026-01-02T03:09:05.000Z",
                      embed=video_view(alt="A loop, described", presentation="gif"))},
        {"post": post(f"{POSTS}/link", text="A link card", created="2026-01-02T03:10:05.000Z",
                      embed=external_view("https://example.com/story", "A story", "About something"))},
        {
            "post": post(f"{POSTS}/quote", text="Quoting this", created="2026-01-02T03:11:05.000Z",
                         embed=record_view(f"{POSTS}/original")),
            "reason": {"$type": "app.bsky.feed.defs#reasonRepost",
                       "by": author(OTHER_DID, "replier.test", "Someone Else"),
                       "indexedAt": "2026-01-02T04:00:00.000Z"},
        },
        {"post": post(f"{POSTS}/pinned", text="Pinned post", created="2026-01-02T03:12:05.000Z"),
         "reason": {"$type": "app.bsky.feed.defs#reasonPin"}},
    ]
    return {"feed": items, "cursor": "example-cursor"}


def build_thread() -> dict:
    root_uri = f"{POSTS}/root"
    text = "Root post mentioning @replier.test, linking https://example.com/root and #conversation"
    root_facets = [
        facet(text, "@replier.test", {"$type": "app.bsky.richtext.facet#mention", "did": OTHER_DID}),
        facet(text, "https://example.com/root", {"$type": "app.bsky.richtext.facet#link", "uri": "https://example.com/root"}),
        facet(text, "#conversation", {"$type": "app.bsky.richtext.facet#tag", "tag": "conversation"}),
    ]
    strong = {"root": {"uri": root_uri, "cid": "bafyexampleroot"},
              "parent": {"uri": root_uri, "cid": "bafyexampleroot"}}

    def reply_node(suffix: str, body: str, *, parent: str = root_uri, replies: list | None = None) -> dict:
        node = {
            "$type": "app.bsky.feed.defs#threadViewPost",
            "post": post(f"{POSTS}/{suffix}", text=body, created="2026-01-02T05:00:00.000Z",
                         reply={"root": strong["root"], "parent": {"uri": parent, "cid": "bafyexampleroot"}}),
        }
        if replies:
            node["replies"] = replies
        return node

    deep = reply_node("deep", "A reply further down")
    middle = reply_node("middle", "A nested reply", replies=[deep, {"$type": "app.bsky.feed.defs#blockedPost", "uri": f"{POSTS}/blocked", "blocked": True}])
    return {
        "thread": {
            "$type": "app.bsky.feed.defs#threadViewPost",
            "post": post(root_uri, text=text, created="2026-01-02T04:00:00.000Z", facets=root_facets, counts=(4, 5, 6)),
            "replies": [
                reply_node("first", "First reply"),
                middle,
                reply_node("second", "Another reply"),
                {"$type": "app.bsky.feed.defs#notFoundPost", "uri": f"{POSTS}/gone", "notFound": True},
            ],
        }
    }


def build_profile() -> dict:
    return {
        "did": ROOT_DID,
        "handle": "example.test",
        "displayName": "Example Person",
        "description": "A synthetic profile used by the tests, not a real account.",
        "avatar": f"{MEDIA}/avatar/example.test.jpg",
        "banner": f"{MEDIA}/banner/example.test.jpg",
        "followersCount": 1234,
        "followsCount": 12,
        "postsCount": 34,
        "createdAt": "2023-01-01T00:00:00.000Z",
        "labels": [],
        "viewer": {"muted": False},
    }


def build_profile_links() -> dict:
    return {
        "did": OTHER_DID,
        "handle": "replier.test",
        "displayName": "Someone Else",
        "links": [
            {
                "uri": "at://did:plc:examplereplier/app.bsky.actor.profile/self",
                "cid": "bafyexampleprofile",
                "url": "https://example.com/",
                "title": "Website",
                "icon": f"{MEDIA}/icon/site.jpg",
            },
            {
                "uri": "at://did:plc:examplereplier/app.bsky.actor.profile/self",
                "cid": "bafyexampleprofile",
                "url": "https://example.com/newsletter",
                "title": "Newsletter",
            },
        ],
    }


def write(name: str, payload: dict) -> None:
    path = FIXTURES / name
    path.write_text(json.dumps(payload, indent=1, sort_keys=False) + "\n")
    print(f"wrote {path.relative_to(FIXTURES.parent.parent)} ({path.stat().st_size} bytes)")


def main() -> int:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    write("author_feed.json", build_author_feed())
    write("thread.json", build_thread())
    write("profile.json", build_profile())
    write("profile_links.json", build_profile_links())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
