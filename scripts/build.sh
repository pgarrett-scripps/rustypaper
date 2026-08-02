#!/usr/bin/env bash
# Builds the CLI, the library and the Python extension, and installs the extension where the
# `rustypaper` Python package expects it. Use this rather than a bare `cargo build`, or the eval
# harness will measure a stale extension.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --release "$@"

# The extension is built with `abi3-py39`, so maturin names it `_rustypaper.abi3.so` — and
# CPython's import machinery prefers that name over a plain `_rustypaper.so` when both exist.
# Writing the plain name and leaving an old abi3 build beside it means every `import rustypaper`
# silently loads the *stale* module: the eval harness then scores a build nobody is editing,
# reports "no change", and looks like a correct measurement. That happened. Install under the
# name Python actually resolves, and clear the other so the ambiguity cannot come back.
cp target/release/lib_rustypaper.so python/rustypaper/_rustypaper.abi3.so
rm -f python/rustypaper/_rustypaper.so

# pdfium is loaded at runtime and is not linked in, so a pdfium-backed build of the Python
# package has to carry its own copy — that is what makes an installed wheel work off a
# checkout, where there is no vendor/ directory to fall back to. The default build is pure
# Rust and ignores it.
if [ -f vendor/pdfium/lib/libpdfium.so ]; then
  cp vendor/pdfium/lib/libpdfium.so python/rustypaper/libpdfium.so
fi

echo "built target/release/rustypaper and python/rustypaper/_rustypaper.abi3.so"
