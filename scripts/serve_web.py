#!/usr/bin/env python3
"""Static server for the built web bundle, with caching disabled.

`python3 -m http.server` plus dx's unhashed wasm filename is a trap: Chrome will
happily reuse a bundle from an earlier build and you spend an afternoon testing
code that no longer exists. This sends no-store for everything so a reload means
a reload.

    python3 scripts/serve_web.py 8099 target/dx/bluesky-alt-first-app/debug/web/public
"""

import functools
import http.server
import socketserver
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-store, must-revalidate")
        self.send_header("Pragma", "no-cache")
        self.send_header("Expires", "0")
        super().end_headers()

    def log_message(self, fmt, *args):  # quieter than the default
        sys.stderr.write("%s\n" % (fmt % args))


def main() -> int:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8099
    root = sys.argv[2] if len(sys.argv) > 2 else "target/dx/bluesky-alt-first-app/debug/web/public"
    handler = functools.partial(Handler, directory=root)
    # Threading, not plain TCPServer: a single-threaded one serves one connection at a time,
    # and a browser holding a keep-alive socket is enough to wedge every other client — curl
    # hangs, a second tab waits, and it looks like the server died. ThreadingHTTPServer sets
    # allow_reuse_address and daemon threads as well.
    with http.server.ThreadingHTTPServer(("", port), handler) as httpd:
        print(f"serving {root} on http://127.0.0.1:{port}/ (no-store)", flush=True)
        httpd.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
