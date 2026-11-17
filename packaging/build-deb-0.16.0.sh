#!/usr/bin/env bash
# build-deb-0.16.0.sh — build the Frost 0.16.0 .deb for Debian 13 (amd64).
#
# What it ships: the Round 16 tree (frost repo HEAD a6f00f2) as a STRIPPED
# DEBUG BUILD. A full release build of the Servo fork + workspace was not
# feasible in the sandbox (the debug link alone takes ~43 min on 1.5 shared
# CPUs; a release build would take many hours and VM replacements kill long
# builds), so this package contains the proven Round 16 debug binary with its
# symbols stripped. It is fully functional, just larger and slower than a
# release build would be. This is stated plainly in the package Description
# below and in README.md — do not remove that note until a real release
# build replaces the binary.
#
# The packaged binary is the gated Round 16 tree binary
# (target/debug/frost, BuildID 4aa99814295e647a3195fb4a60e85a4a8b0063dc) —
# no rebuild, no code changes. Note: the gate binary was linked with
# `-C link-arg=-Wl,--strip-debug` (517 MB, same code, no debuginfo)
# because the plain 2 GB debug link OOM-killed twice in the sandbox;
# `strip` below removes the remaining symbol table as in past rounds.
#
# Depends were RE-DERIVED for Round 16 from ldd + /proc maps of the actual
# running binary (PID maps, 2026-11-17). Result: IDENTICAL to the 0.14.0
# list — no changes. Two points worth recording:
# - libvulkan1 STAYS: the default path is now GL (the running binary maps
#   no libvulkan.so.1), but WGPU_BACKEND=vulkan is a supported user
#   override and that path loads the Vulkan loader + ICD. The package
#   must work with the override.
# - libGLESv2 was NOT added, despite the Round 16 research note saying
#   "the GLES backend needs libGLESv2". Verified before packaging:
#   libGLESv2.so is in neither ldd nor the running binary's /proc maps,
#   and wgpu-hal 28's EGL code (src/gles/egl.rs:576-590) binds desktop
#   GL (OPENGL_API) whenever the EGL client-APIs string contains
#   "OpenGL" — Mesa (llvmpipe here, crocus on Joel's box) always does —
#   falling back to GLES only when desktop GL is unavailable. The gate
#   log confirms it: adapter driver_info "4.5 (Core Profile) Mesa
#   25.0.7", backend Gl. So on any Mesa system libGLESv2 is never
#   loaded; adding libgles2 would be an inaccurate dependency.
#
# Usage (from the frost repo root):
#   ./packaging/build-deb-0.16.0.sh
# Output: ~/workspace/frost-0.16.0_amd64.deb (+ sha256sum printed at the end)
#
# Only dpkg-deb is used. This script never installs anything (no dpkg -i,
# no apt) — install on the target machine with:
#   sudo dpkg -i frost-0.16.0_amd64.deb
#   sudo apt-get install -f   # only if a dependency is missing
#
# The staging tree lives under ~/workspace/harness-tmp/frost/pkg16/deb-stage
# (never in the repo — it holds a ~300 MB binary). The .deb itself is not
# committed to git either.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_HEAD="a6f00f2"
SRC_BIN="$ROOT/target/debug/frost"
STAGE="$HOME/workspace/harness-tmp/frost/pkg16/deb-stage"
OUT="$HOME/workspace/frost-0.16.0_amd64.deb"

if [ "${FROST_DEB_ALLOW_OTHER_HEAD:-}" != "1" ]; then
    HEAD="$(git -C "$ROOT" rev-parse --short HEAD)"
    if [ "$HEAD" != "$EXPECTED_HEAD" ]; then
        echo "ERROR: frost repo HEAD is $HEAD, expected $EXPECTED_HEAD (Round 16)." >&2
        echo "This script packages the Round 16 tree. Set FROST_DEB_ALLOW_OTHER_HEAD=1 to override." >&2
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
Version: 0.16.0
Section: web
Priority: optional
Architecture: amd64
Depends: libc6, libgcc-s1, libstdc++6, zlib1g, libfontconfig1, libfreetype6, libexpat1, libbz2-1.0, libpng16-16t64, libbrotli1, libx11-6, libx11-xcb1, libxext6, libxi6, libxcb1, libxcb-xkb1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libwayland-server0, libegl1, libgl1, libgbm1, libdrm2, libvulkan1
Maintainer: Joel Horrell <infektedpc@gmail.com>
Description: Frost — a web browser built on Servo
 Frost is a web browser written in Rust, using Servo 0.8.0 as its
 rendering engine and libcosmic for its user interface.
 .
 NOTE: this 0.16.0 package ships the Round 16 tree as a STRIPPED DEBUG
 BUILD. A full release build was not feasible in the sandbox, so the
 binary is larger (~300 MB) and slower than a release build would be.
 It is fully functional — back/forward, merged reload/stop button
 (reload icon while idle, X while a load is in flight), round toolbar
 icons, address bar, command palette (Ctrl+K), split view (two pages
 side by side, toolbar acts on the active pane), Snowplow tracker/ad
 blocking (62-domain blocklist, Settings toggle, per-page blocked
 count), Deep Freeze offline page archives ("Freeze this page" saves
 the active pane as a self-contained HTML file; "Archived pages"
 lists them with Open/Delete), menu items in their own native windows
 (Settings, Print, Open file, Inspector, Exit), extensions, print
 preview. Round 16 makes Frost lightweight: the UI defaults to the
 hardware OpenGL backend (only if a GL adapter is probed first; the
 WGPU_BACKEND env var still overrides) and the page renderer tries a
 hardware adapter first (FROST_RENDERER=software forces the old
 software path). There is no internal web server: the Round 3–6
 frost:// pages are gone, replaced by the native windows.
EOF

echo "Building .deb..."
dpkg-deb --build "$STAGE" "$OUT"
echo "Contents:"
dpkg-deb -c "$OUT"
echo "SHA-256:"
sha256sum "$OUT"
