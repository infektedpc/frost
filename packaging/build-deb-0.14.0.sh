#!/usr/bin/env bash
# build-deb-0.14.0.sh — build the Frost 0.14.0 .deb for Debian 13 (amd64).
#
# What it ships: the Round 14 tree (frost repo HEAD 42e2a96) as a STRIPPED
# DEBUG BUILD. A full release build of the Servo fork + workspace was not
# feasible in the sandbox (the debug link alone takes ~43 min on 1.5 shared
# CPUs; a release build would take many hours and VM replacements kill long
# builds), so this package contains the proven Round 14 debug binary with its
# symbols stripped. It is fully functional, just larger and slower than a
# release build would be. This is stated plainly in the package Description
# below and in README.md — do not remove that note until a real release
# build replaces the binary.
#
# The packaged binary is the gated Round 14 tree binary
# (target/debug/frost, BuildID 6a28b417d94ea00a1fcb4d75bfa3823d6413ffe8) —
# no rebuild, no code changes. Note: the gate binary was linked with
# `-C link-arg=-Wl,--strip-debug` (517 MB, same code, no debuginfo)
# because the plain 2 GB debug link OOM-killed twice in the sandbox;
# `strip` below removes the remaining symbol table as in past rounds.
#
# Depends were RE-DERIVED for Round 14 from ldd + /proc maps of the actual
# running binary (PID maps, 2026-11-16). Result: IDENTICAL to the 0.11.0
# list — no changes. In particular libvulkan1 stays (the running binary
# maps system libvulkan.so.1 and the lavapipe ICD; wgpu takes the Vulkan
# path here, as in Rounds 7–13) and libwayland-server0 stays
# (libwayland-server.so.0 is mapped by the running process alongside
# libwayland-client). Everything else re-confirmed in the same maps.
#
# Usage (from the frost repo root):
#   ./packaging/build-deb-0.14.0.sh
# Output: ~/workspace/frost-0.14.0_amd64.deb (+ sha256sum printed at the end)
#
# Only dpkg-deb is used. This script never installs anything (no dpkg -i,
# no apt) — install on the target machine with:
#   sudo dpkg -i frost-0.14.0_amd64.deb
#   sudo apt-get install -f   # only if a dependency is missing
#
# The staging tree lives under ~/workspace/harness-tmp/frost/pkg14/deb-stage
# (never in the repo — it holds a ~300 MB binary). The .deb itself is not
# committed to git either.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_HEAD="42e2a96"
SRC_BIN="$ROOT/target/debug/frost"
STAGE="$HOME/workspace/harness-tmp/frost/pkg14/deb-stage"
OUT="$HOME/workspace/frost-0.14.0_amd64.deb"

if [ "${FROST_DEB_ALLOW_OTHER_HEAD:-}" != "1" ]; then
    HEAD="$(git -C "$ROOT" rev-parse --short HEAD)"
    if [ "$HEAD" != "$EXPECTED_HEAD" ]; then
        echo "ERROR: frost repo HEAD is $HEAD, expected $EXPECTED_HEAD (Round 14)." >&2
        echo "This script packages the Round 14 tree. Set FROST_DEB_ALLOW_OTHER_HEAD=1 to override." >&2
        exit 1
    fi
    if [ -n "$(git -C "$ROOT" status --short)" ]; then
        echo "ERROR: frost repo has uncommitted changes — commit them first." >&2
        exit 1
    fi
fi

if [ ! -x "$SRC_BIN" ]; then
    echo "ERROR: $SRC_BIN not found — build it first (cargo build -p frost)." >&2
    exit 1
fi

echo "Source binary: $SRC_BIN ($(stat -c%s "$SRC_BIN") bytes, debug, strip-debug linked)"
rm -rf "$STAGE"
mkdir -p "$STAGE/usr/bin"
mkdir -p "$STAGE/usr/share/applications"
mkdir -p "$STAGE/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$STAGE/DEBIAN"

echo "Copying + stripping the binary..."
cp "$SRC_BIN" "$STAGE/usr/bin/frost"
strip "$STAGE/usr/bin/frost"
chmod 755 "$STAGE/usr/bin/frost"
echo "Stripped binary: $(stat -c%s "$STAGE/usr/bin/frost") bytes"

cp "$ROOT/packaging/frost.desktop" "$STAGE/usr/share/applications/frost.desktop"
cp "$ROOT/packaging/frost.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/frost.svg"
chmod 644 "$STAGE/usr/share/applications/frost.desktop" "$STAGE/usr/share/icons/hicolor/scalable/apps/frost.svg"

# The workspace umask leaves new dirs at 2770; dpkg-deb requires the
# control directory (and installed dirs should be) 0755.
find "$STAGE" -type d -exec chmod 755 {} +

cat > "$STAGE/DEBIAN/control" <<'EOF'
Package: frost
Version: 0.14.0
Section: web
Priority: optional
Architecture: amd64
Depends: libc6, libgcc-s1, libstdc++6, zlib1g, libfontconfig1, libfreetype6, libexpat1, libbz2-1.0, libpng16-16t64, libbrotli1, libx11-6, libx11-xcb1, libxext6, libxi6, libxcb1, libxcb-xkb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-server0, libegl1, libgl1, libgbm1, libdrm2, libvulkan1
Maintainer: Joel Horrell <infektedpc@gmail.com>
Description: Frost — a web browser built on Servo
 Frost is a web browser written in Rust, using Servo 0.8.0 as its
 rendering engine and libcosmic for its user interface.
 .
 NOTE: this 0.14.0 package ships the Round 14 tree as a STRIPPED DEBUG
 BUILD. A full release build was not feasible in the sandbox, so the
 binary is larger (~300 MB) and slower than a release build would be.
 It is fully functional — back/forward, merged reload/stop button
 (reload icon while idle, X while a load is in flight), round toolbar
 icons, address bar, command palette (Ctrl+K), split view (two pages
 side by side, toolbar acts on the active pane), Snowplow tracker/ad
 blocking (62-domain blocklist, Settings toggle, per-page blocked
 count), menu items in their own native windows (Settings, Print, Open
 file, Inspector, Exit), extensions, print preview. There is no
 internal web server: the Round 3–6 frost:// pages are gone, replaced
 by the native windows.
EOF

echo "Building .deb..."
dpkg-deb --build "$STAGE" "$OUT"
echo "Contents:"
dpkg-deb -c "$OUT"
echo "SHA-256:"
sha256sum "$OUT"
