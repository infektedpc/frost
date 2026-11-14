#!/usr/bin/env bash
# build-flatpak.sh — build the Frost Flatpak on a machine that has Flatpak.
#
# UNBUILT and UNVERIFIED in the sandbox: the sandbox has no flatpak,
# flatpak-builder, bwrap, or ostree binaries at all, and system packages
# may not be installed there — so this was never run. Run it on Joel's
# Debian 13 box (Flatpak already installed there).
#
# Prerequisites on that machine:
#   - flatpak + flatpak-builder installed
#   - the Flathub remote (for the org.freedesktop Platform/SDK):
#       flatpak remote-add --if-not-exists flathub \
#           https://flathub.org/repo/flathub.flatpakrepo
#   - the GitHub repo https://github.com/infektedpc/frost.git exists
#     (the manifest clones branch `main` from it)
#   - network access during the build (clone-fork.sh clones servo/servo;
#     cargo fetches crates + the libcosmic git dep) — hence --share=network
#   - patience: a full Servo release build takes hours, not minutes
#
# Usage (from the frost repo root):
#   ./packaging/build-flatpak.sh
# Output: a .flatpak bundle in the current directory.

set -euo pipefail

if ! command -v flatpak-builder >/dev/null 2>&1; then
    echo "ERROR: flatpak-builder is not installed on this machine." >&2
    echo "Install it first (Debian: sudo apt install flatpak-builder), then re-run." >&2
    exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

flatpak-builder --install-deps-from=flathub --share=network \
    --force-clean build-dir packaging/org.frost.Frost.yml

flatpak-builder --repo=repo --force-clean build-dir packaging/org.frost.Frost.yml

flatpak build-bundle repo frost-0.4.0-x86_64.flatpak org.frost.Frost

echo "Built: $ROOT/frost-0.4.0-x86_64.flatpak"
echo "Install with: flatpak install frost-0.4.0-x86_64.flatpak"
