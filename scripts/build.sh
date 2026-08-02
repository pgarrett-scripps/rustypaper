#!/usr/bin/env bash
# Builds the CLI, the library and the Python extension, and installs the extension where the
# `rustypdf` Python package expects it. Use this rather than a bare `cargo build`, or the eval
# harness will measure a stale extension.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --release
cp target/release/lib_rustypdf.so python/rustypdf/_rustypdf.so

# pdfium is loaded at runtime and is not linked in, so the Python package has
# to carry its own copy — that is what makes an installed wheel work off a
# checkout, where there is no vendor/ directory to fall back to.
if [ -f vendor/pdfium/lib/libpdfium.so ]; then
  cp vendor/pdfium/lib/libpdfium.so python/rustypdf/libpdfium.so
fi

echo "built target/release/rp2m and python/rustypdf/_rustypdf.so"
