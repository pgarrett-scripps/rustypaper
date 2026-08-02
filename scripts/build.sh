#!/usr/bin/env bash
# Builds the CLI, the library and the Python extension, and installs the extension where the
# `rustypdf` Python package expects it. Use this rather than a bare `cargo build`, or the eval
# harness will measure a stale extension.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build --release
cp target/release/lib_rustypdf.so python/_rustypdf.so

echo "built target/release/rp2m and python/_rustypdf.so"
