#!/usr/bin/env python3
"""Does the installed Android app accept a WebView devtools attach, and is the
artifact debuggable?

Both answers come from the device, not from the build, because both have fooled
this project already. `dx`'s Kotlin is a red herring: it generates
`class MainActivity : WryActivity()` with no debugging call, and a Kotlin
`setWebContentsDebuggingEnabled(false)` after `super.onCreate` does not undo the
socket either (tried; the WebView is already created by then). The flag comes
from the Rust side — wry compiles the call in under
`#[cfg(any(debug_assertions, feature = "devtools"))]` and `libmain.so` here is a
debug-profile build (overflow-check strings, debug info), so the socket opens on
every launch.

That makes `DEBUGGABLE` the part worth reading: on this artifact `adb run-as`
lists `app_webview`, so a remembered `altfirst.session` is readable with or
without devtools.

    python3 scripts/check_android_devtools.py      # exit 1 if a socket is open
"""

from __future__ import annotations

import re
import subprocess
import sys
import time

PACKAGE = "com.example.BlueskyAltFirstApp"
ACTIVITY = "dev.dioxus.main.MainActivity"


def adb(*args: str) -> str:
    result = subprocess.run(["adb", *args], capture_output=True, text=True, timeout=120)
    return (result.stdout or "") + (result.stderr or "")


def sockets() -> dict[int, str]:
    """pid -> socket name for every WebView devtools socket on the device."""
    found: dict[int, str] = {}
    for line in adb("shell", "cat", "/proc/net/unix").splitlines():
        match = re.search(r"@(webview_devtools_remote_(\d+))", line)
        if match:
            found[int(match.group(2))] = match.group(1)
    return found


def main() -> int:
    if not any(line.strip() for line in adb("shell", "pm", "list", "packages", PACKAGE).splitlines()):
        print(f"{PACKAGE} is not installed")
        return 1

    # Relaunch: the socket only exists while a live WebView does, so sampling a
    # backgrounded app reports "off" whether or not the artifact enables it.
    adb("shell", "input", "keyevent", "KEYCODE_WAKEUP")
    adb("shell", "am", "force-stop", PACKAGE)
    adb("shell", "am", "start", "-n", f"{PACKAGE}/{ACTIVITY}")
    socket_name = None
    for _ in range(15):
        time.sleep(1)
        pid = adb("shell", "pidof", PACKAGE).strip().split()[0] if adb("shell", "pidof", PACKAGE).strip() else None
        if pid and int(pid) in sockets():
            socket_name = sockets()[int(pid)]
            break

    flags = adb("shell", "dumpsys", "package", PACKAGE)
    debuggable = bool(re.search(r"flags=\[[^\]]*DEBUGGABLE", flags))
    backup = bool(re.search(r"flags=\[[^\]]*ALLOW_BACKUP", flags))

    print(f"{PACKAGE}: pid {pid}")
    print(f"  devtools socket: {socket_name or 'none after a fresh launch'}")
    print(f"  debuggable: {debuggable}   allow-backup: {backup}")
    if socket_name:
        print(f"  attach with: adb forward tcp:9223 localabstract:{socket_name}")
        print("  -> fine for debugging, not for an artifact you hand to someone else")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
