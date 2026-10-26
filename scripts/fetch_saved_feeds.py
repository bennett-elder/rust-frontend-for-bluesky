#!/usr/bin/env python3
"""Probe whether an account's saved/pinned feeds are visible in public data.

The assumption this started from was wrong, and the probe is what disproved it.
Feed preferences are *not* stored as a repo record: `describeRepo` on real
accounts lists `app.bsky.actor.profile`, `feed.post`, `feed.like`, `graph.follow`
and so on, but never `app.bsky.actor.preferences`. Saved feeds live behind
`app.bsky.actor.getPreferences`, which is authenticated and only ever returns the
calling account's own preferences.

So: saved feeds are not enumerable from public data. This script is kept as the
check for that claim (`RecordNotFound` on every account tested, including
bsky.app's own), and as a starting point for importing feeds properly once the
client has OAuth.

Usage:
    python3 scripts/fetch_saved_feeds.py alice.bsky.social bob.bsky.social
"""

from __future__ import annotations

import json
import sys
import urllib.parse
import urllib.request

PUBLIC_API = "https://api.bsky.app"
TIMEOUT = 20


def get_json(url: str) -> tuple[dict, dict[str, str]]:
    request = urllib.request.Request(url, headers={"User-Agent": "alt-first-feeds/0.1"})
    with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
        return json.loads(response.read()), dict(response.headers)


def resolve(handle: str) -> tuple[str, str]:
    """handle -> (did, pds_endpoint)"""
    profile, _ = get_json(
        f"{PUBLIC_API}/xrpc/com.atproto.identity.resolveHandle?handle={urllib.parse.quote(handle)}"
    )
    did = profile["did"]

    if did.startswith("did:plc:"):
        document, _ = get_json(f"https://plc.directory/{did}")
    else:
        document, _ = get_json(f"https://{did.removeprefix('did:web:')}/.well-known/did.json")

    pds = next(
        service["serviceEndpoint"]
        for service in document.get("service", [])
        if service.get("id", "").endswith("#atproto_pds")
    )
    return did, pds


def saved_feeds(handle: str) -> None:
    did, pds = resolve(handle)
    path = (
        "/xrpc/com.atproto.repo.getRecord"
        f"?repo={urllib.parse.quote(did)}"
        "&collection=app.bsky.actor.preferences&rkey=self"
    )
    try:
        record, headers = get_json(f"{pds}{path}")
    except urllib.error.HTTPError as error:
        body = error.read().decode(errors="replace")
        print(f"@{handle}: no preferences record ({error.code}) {body[:90]}")
        return

    print(f"@{handle}  did={did}  pds={pds}")
    print(f"  cors: {headers.get('Access-Control-Allow-Origin')!r}")

    value = record.get("value", {})
    items = (value.get("savedFeedsPrefV2") or {}).get("items") or []
    if items:
        print(f"  savedFeedsPrefV2: {len(items)} entries")
        for item in items:
            kind = item.get("type", "?")
            pinned = "pinned" if item.get("pinned") else "      "
            print(f"    {pinned} {kind:9} {item.get('value', '')}")
    else:
        print("  savedFeedsPrefV2: absent")

    legacy = value.get("savedFeedsPref") or {}
    if legacy:
        for key in ("pinned", "saved"):
            for uri in legacy.get(key, []):
                print(f"    legacy {key:6} {uri}")

    if not items and not legacy:
        print(f"  (other preference keys: {', '.join(sorted(value.keys())) or 'none'})")


def main() -> int:
    handles = sys.argv[1:]
    if not handles:
        print(__doc__)
        return 1
    for handle in handles:
        try:
            saved_feeds(handle)
        except Exception as error:  # noqa: BLE001 - probe script
            print(f"@{handle}: failed: {error}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
