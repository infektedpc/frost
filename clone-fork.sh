#!/usr/bin/env bash
# clone-fork.sh — recreate the Frost Servo fork from scratch.
#
# What it does:
#   1. Shallow-clones servo/servo at tag v0.8.0 into ./servo/
#   2. Applies the thirteen proven 0.8.0 engine patches from ./patches/,
#      one commit per patch (the three servo-de patches + Frost's own
#      F-001 stop patch, which the engine's stop() depends on, the
#      F-005 viewport-refresh patch, which the engine's refresh_viewport()
#      depends on, the Round 4 patches: F-006 reload notification,
#      F-007 full present, F-008 stop-aborts-pending, the Round 16
#      patch: F-009 hardware rendering context, the Round 17
#      patch: F-010 HTTP/2 max header list size, and the Round 18
#      patch: F-011 MSE foundation, and the Round 19 patch: F-012
#      MSE appendBuffer → media thread, and the Round 20 patch: F-013
#      MSE remuxer).
#
# Usage (from the frost repo root):
#   ./clone-fork.sh
#
# Requirements: git, network access to github.com. No remote is added;
# the fork is yours — point it at your own GitHub repo when you are ready
# (see README.md, "Pushing to GitHub").
#
# Notes:
# * The fork is a NESTED repo (its own .git). It is gitignored by the
#   frost repo and never committed into it.
# * The clone is shallow (--depth 1). Full history later with:
#     git -C servo fetch --unshallow
# * If the initial checkout ever fails with an empty index (seen once in
#   the sandbox: clone reported "unable to checkout working tree"), the
#   script recovers with `git read-tree HEAD && git checkout -- .`.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FORK_DIR="$ROOT/servo"
PATCHES_DIR="$ROOT/patches"

if [ -d "$FORK_DIR/.git" ]; then
    echo "servo/ already exists — refusing to overwrite. Delete it first if you want a fresh fork."
    exit 1
fi

echo "Cloning servo/servo at v0.8.0 (shallow)..."
git clone --depth 1 --branch v0.8.0 https://github.com/servo/servo "$FORK_DIR" || true

cd "$FORK_DIR"

# Recovery for the rare failed-checkout case: rebuild the index and check out.
if [ "$(git ls-files | wc -l)" -eq 0 ]; then
    echo "Checkout looks empty — recovering with read-tree + checkout..."
    git read-tree HEAD
    git checkout -- .
fi

git config user.name "Joel Horrell"
git config user.email "infektedpc@gmail.com"

echo "Fork point: $(git rev-parse HEAD) ($(git log --oneline -1))"

for patch in servo-keypress-charcode-0.8 servo-damage-rects-0.8 servo-dmabuf-0.8 servo-stop-0.8 servo-refresh-viewport-0.8 servo-reload-notify-0.8 servo-full-present-0.8 servo-stop-abort-pending-0.8 servo-hardware-rendering-context-0.8 servo-http2-header-list-size-0.8 servo-mediasource-foundation-0.8 servo-mediasource-append-0.8 servo-mediasource-remux-0.8; do
    echo "Applying $patch..."
    git apply --check "$PATCHES_DIR/$patch.patch"
    git apply "$PATCHES_DIR/$patch.patch"
    git add -A
    case "$patch" in
        servo-keypress-charcode-0.8)
            git commit -qm "fork: apply keypress charcode patch (0.8)"
            ;;
        servo-damage-rects-0.8)
            git commit -qm "fork: apply damage-rects patch (0.8)"
            ;;
        servo-dmabuf-0.8)
            git commit -qm "fork: apply dmabuf patch (0.8)"
            ;;
        servo-stop-0.8)
            git commit -qm "fork: F-001 — add WebView::stop() (Round 2)"
            ;;
        servo-refresh-viewport-0.8)
            git commit -qm "fork: F-005 — add WebView::refresh_viewport() (Round 3)"
            ;;
        servo-reload-notify-0.8)
            git commit -qm "F-006: notify the delegate on reload (set_load_status)"
            ;;
        servo-full-present-0.8)
            git commit -qm "F-007: full present instead of partial present (fix blank tiny pages)"
            ;;
        servo-stop-abort-pending-0.8)
            git commit -qm "F-008: Stop also aborts a pending navigation"
            ;;
        servo-hardware-rendering-context-0.8)
            git commit -qm "F-009: Add HardwareRenderingContext (Round 16)"
            ;;
        servo-http2-header-list-size-0.8)
            git commit -qm "F-010: Raise HTTP/2 max header list size to 256 KiB (Round 17)"
            ;;
        servo-mediasource-foundation-0.8)
            git commit -qm "F-011: MSE foundation — MediaSource/SourceBuffer/SourceBufferList (Round 18)"
            ;;
        servo-mediasource-append-0.8)
            git commit -qm "F-012: MSE slice 2 — appendBuffer → media thread (Round 19)"
            ;;
        servo-mediasource-remux-0.8)
            git commit -qm "F-013: MSE slice 3 — remux per-track DASH streams into one player stream (Round 20)"
            ;;
    esac
done

echo "Fork ready:"
git log --oneline | head -5
echo "Full history later with: git -C servo fetch --unshallow"
