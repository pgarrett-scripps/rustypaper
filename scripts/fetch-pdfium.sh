#!/usr/bin/env bash
# Downloads a prebuilt pdfium into vendor/pdfium/.
#
# pdfium is BSD-3-Clause (Chromium). We pin a specific build so the library we test against is
# the library we run against; bump PDFIUM_RELEASE deliberately, not incidentally.
set -euo pipefail

PDFIUM_RELEASE="${PDFIUM_RELEASE:-chromium/7961}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$ROOT/vendor/pdfium"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)   ASSET=pdfium-linux-x64 ;;
  Linux-aarch64)  ASSET=pdfium-linux-arm64 ;;
  Darwin-x86_64)  ASSET=pdfium-mac-x64 ;;
  Darwin-arm64)   ASSET=pdfium-mac-arm64 ;;
  *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

URL="https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE//\//%2F}/${ASSET}.tgz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "fetching $ASSET from $PDFIUM_RELEASE"
curl -fsSL -o "$TMP/pdfium.tgz" "$URL"

mkdir -p "$DEST"
tar xzf "$TMP/pdfium.tgz" -C "$DEST" LICENSE VERSION
tar xzf "$TMP/pdfium.tgz" -C "$DEST" lib 2>/dev/null || tar xzf "$TMP/pdfium.tgz" -C "$DEST" bin

echo "pdfium $(tr '\n' ' ' < "$DEST/VERSION") installed to $DEST"
