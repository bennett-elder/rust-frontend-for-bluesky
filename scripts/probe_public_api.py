#!/usr/bin/env python3
"""Probe the public (unauthenticated) Bluesky AppView.

Purpose:
  1. Confirm read-only endpoints work with no token and no login.
  2. Record the CORS response header a browser would rely on.
  3. Save the raw JSON to .tmp/probe/ for inspection. The tests use synthetic
     fixtures instead, so a probe run never changes what CI parses.

Usage:
    python3 scripts/probe_public_api.py
    python3 scripts/probe_public_api.py --actor bsky.app --query screenshot --limit 5
"""

from __future__ import annotations

import argparse
import json
import pathlib
import urllib.parse
import urllib.request

PUBLIC_API = "https://api.bsky.app"
# Live payloads are for inspection; the committed fixtures are synthetic and stable
# (scripts/make_fixtures.py). Writing here keeps a probe run from changing test data.
FIXTURES = pathlib.Path(__file__).resolve().parent.parent / ".tmp" / "probe"

# A browser sends Origin, and only gets to read the response if the server
# echoes an Access-Control-Allow-Origin header. Send it so we see the real one.
BROWSER_ORIGIN = "http://127.0.0.1:8099"


def fetch(path: str) -> tuple[bytes, dict[str, str]]:
    request = urllib.request.Request(
        f"{PUBLIC_API}{path}",
        headers={
            "User-Agent": "alt-first-probe/0.1",
            "Origin": BROWSER_ORIGIN,
        },
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return response.read(), dict(response.headers)


def report(name: str, path: str, filename: str) -> bytes:
    body, headers = fetch(path)
    (FIXTURES / filename).write_bytes(body)
    print(f"GET {path}")
    print(f"  bytes: {len(body)}")
    print(f"  access-control-allow-origin: {headers.get('Access-Control-Allow-Origin')!r}")
    return body


def summarize(posts: list[dict]) -> None:
    print(f"  posts: {len(posts)}")
    for post in posts:
        embed = post.get("embed") or {}
        kind = embed.get("$type", "none")
        print(f"    @{post['author']['handle']:<26} {kind}")
        for image in embed.get("images", []):
            alt = image.get("alt", "")
            aspect = image.get("aspectRatio") or {}
            label = "alt" if alt.strip() else "NO ALT"
            print(f"        image {aspect.get('width')}x{aspect.get('height')} [{label}] {alt[:70]!r}")


def summarize_thread(node: dict, stats: dict, depth: int = 0) -> None:
    """Walk a getPostThread tree and record facet/reply coverage."""
    kind = node.get("$type", "")
    short = kind.split("#")[-1] or "unknown"

    if short in {"notFoundPost", "blockedPost"}:
        stats["special"].add(short)
        if depth < 2:
            print(f"    {'  ' * depth}[{short}]")
        return

    post = node.get("post") or {}
    record = post.get("record") or {}
    facets = record.get("facets") or []

    stats["nodes"] += 1
    if facets:
        stats["nodes_with_facets"] += 1
        stats["facets"] += len(facets)
        for facet in facets:
            for feature in facet.get("features", []):
                stats["features"].add(feature.get("$type", "").split("#")[-1])
    if record.get("reply"):
        stats["reply_refs"] += 1

    if depth < 2 and facets:
        text = record.get("text", "")
        print(f"    {'  ' * depth}{text[:48]!r} facets={len(facets)}")

    for child in node.get("replies") or []:
        summarize_thread(child, stats, depth + 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--actor", default="bsky.app", help="handle or DID to read")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--thread-depth", type=int, default=2, dest="thread_depth")
    parser.add_argument(
        "--links-actor",
        default=None,
        dest="links_actor",
        help="actor whose profile carries links; omitted keeps the synthetic fixture",
    )
    args = parser.parse_args()

    FIXTURES.mkdir(parents=True, exist_ok=True)

    feed_path = (
        f"/xrpc/app.bsky.feed.getAuthorFeed?actor={args.actor}"
        f"&limit={args.limit}"
    )
    body = report("author feed", feed_path, "author_feed.json")
    feed = json.loads(body)
    posts = [item["post"] for item in feed.get("feed", [])]
    summarize(posts)

    # A repost-heavy feed: attribution (`reason`) lives on the feed item, not the post.
    reposts = sum(1 for item in feed.get("feed", []) if item.get("reason"))
    print(f"  reposts: {reposts}/{len(feed.get('feed', []))} (attribution is on the feed item)")

    # searchPosts returns 403 on the public host, so a thread is the reliable way
    # to capture facets and reply structure.
    with_replies = [post for post in posts if post.get("replyCount", 0) > 0]
    # The smallest thread, not the first: a popular post pulls in hundreds of repliers,
    # and a fixture is test data, not an archive of other people's conversations.
    candidate = min(with_replies, key=lambda post: post["replyCount"]) if with_replies else None
    if candidate is None:
        print(f"\nno post with replies in @{args.actor}; try --actor with a chattier account")
        return 1

    thread_path = (
        "/xrpc/app.bsky.feed.getPostThread?uri="
        + urllib.parse.quote(candidate["uri"], safe="")
        + f"&depth={args.thread_depth}&parentHeight=2"
    )
    body = report("thread", thread_path, "thread.json")
    thread = json.loads(body).get("thread", {})

    stats = {
        "nodes": 0,
        "nodes_with_facets": 0,
        "facets": 0,
        "features": set(),
        "reply_refs": 0,
        "special": set(),
    }
    summarize_thread(thread, stats)
    print(
        "  nodes={} with_facets={} facets={} reply_refs={} features={} special={}".format(
            stats["nodes"],
            stats["nodes_with_facets"],
            stats["facets"],
            stats["reply_refs"],
            sorted(stats["features"]) or "none",
            sorted(stats["special"]) or "none",
        )
    )

    # A profile, for the detailed shape: counts, description, pronouns and labels.
    # Neutral by default — an organisation account, not a private individual.
    report(
        "profile",
        f"/xrpc/app.bsky.actor.getProfile?actor={args.actor}",
        "profile.json",
    )

    if args.links_actor:
        report(
            "profile links",
            f"/xrpc/app.bsky.actor.getProfile?actor={args.links_actor}",
            "profile_links.json",
        )
    else:
        # profile_links.json is deliberately synthetic: the test only needs the link
        # schema and `is_safe`, and no live account should be baked into a fixture to
        # prove that. Regenerating without --links-actor leaves it untouched.
        print("profile links: skipped (kept as the synthetic fixture)")

    # Thread gates tell a client who may reply; capture one if present.
    threadgate = json.loads(body).get("threadgate")
    print(f"  threadgate: {'present' if threadgate else 'absent'}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
