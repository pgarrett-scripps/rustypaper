# Releasing

## Order matters

`rp2m` depends on `rustypdf`, so the library goes first and the CLI cannot be packaged until it
is on the index. `rustypdf-py` is marked `publish = false` — it is a cdylib built by maturin, not
a crate anyone consumes.

```sh
scripts/fetch-pdfium.sh
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
cd eval && PYTHONPATH=.:../python python3 -m rp2m_eval --baseline baseline.json && cd ..

cargo publish -p rustypdf
cargo publish -p rp2m          # only once rustypdf is live on the index
```

## Python wheel

```sh
maturin build -m rustypdf-py/Cargo.toml --release
```

## What is deliberately not in the package

`tests/corpus.rs` is excluded: it needs PDFs that are not ours to redistribute. Unit and
robustness tests ship and run anywhere; corpus tests skip when `corpus/` is absent.

pdfium is not bundled and cannot be — it is a native library. `Error::PdfiumUnavailable` tells
the user where to get it.
