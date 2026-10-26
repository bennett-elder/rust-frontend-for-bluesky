#!/usr/bin/env python3
"""Resolve bsky.app feed URLs to at-URIs and check if they work logged-out.

Accepts either a bsky.app URL or an at:// generator URI:

    python3 scripts/probe_feeds.py https://bsky.app/profile/bsky.app/feed/mutuals
    python3 scripts/probe_feeds.py at://did:plc:.../app.bsky.feed.generator/thevids

Reports HTTP status, how many posts came back anonymously, and the feedContext
hints the generator attached. A non-200 means the generator needs a viewer.
"""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.parse
import urllib.request

PUBLIC_API = "https://api.bsky.app"
TIMEOUT = 25


def get_json(url: str) -> dict:
    request = urllib.request.Request(url, headers={"User-Agent": "alt-first-feeds/0.1"})
    with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
        return json.loads(response.read())


def to_at_uri(value: str) -> str:
    """bsky.app/profile/<handle>/feed/<rkey> -> at://<did>/app.bsky.feed.generator/<rkey>"""
    if value.startswith("at://"):
        return value

    parsed = urllib.parse.urlparse(value)
    parts = [part for part in parsed.path.split("/") if part]
    # profile/<handle>/feed/<rkey>
    if len(parts) != 4 or parts[0] != "profile" or parts[2] != "feed":
        raise ValueError(f"not a feed URL: {value}")

    handle, rkey = parts[1], parts[3]
    resolved = get_json(
        f"{PUBLIC_API}/xrpc/com.atproto.identity.resolveHandle?handle={urllib.parse.quote(handle)}"
    )
    return f"at://{resolved['did']}/app.bsky.feed.generator/{rkey}"


def probe(value: str) -> None:
    try:
        at_uri = to_at_uri(value)
    except Exception as error:  # noqa: BLE001 - probe script
        print(f"FAIL  {value}\n      {error}")
        return

    query = (
        f"{PUBLIC_API}/xrpc/app.bsky.feed.getFeed"
        f"?feed={urllib.parse.quote(at_uri, safe='')}&limit=5"
    )
    try:
        feed = get_json(query)
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")[:120]
        print(f"{error.code:>4}  {at_uri}\n      {body}")
        return
    except Exception as error:  # noqa: BLE001
        print(f"FAIL  {at_uri}\n      {error}")
        return

    items = feed.get("feed", [])
    print(f" 200  {at_uri}")
    print(f"      posts={len(items)} cursor={'yes' if feed.get('cursor') else 'no'}")
    for item in items[:3]:
        post = item.get("post", {})
        handle = post.get("author", {}).get("handle", "?")
        context = item.get("feedContext") or "-"
        text = (post.get("record", {}).get("text") or "").replace("\n", " ")[:42]
        print(f"      @{handle:<28} ctx={context:<16} {text!r}")


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 1
    for value in sys.argv[1:]:
        probe(value)
    return 0


if __name__ == "__main__":
    sys.exit(main())
