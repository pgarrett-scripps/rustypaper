# Releasing

One project, one version, one tag. `v0.1.0` publishes the crate to crates.io
and the wheels to PyPI, both named `rustypaper`.

## Order matters

`rustium-pdf` goes first. rustypaper depends on it, and while a local build
resolves that through `path = "../rustium-pdf"`, the registry has no such
path — it resolves `version = "0.1.0"` and fails if that version is not
there yet.

```
rustium-pdf  v0.1.0  →  crates.io
                            │
rustypaper   v0.1.0  ───────┴──→  crates.io  +  PyPI
```

`rustypaper-py` is marked `publish = false`: it is a cdylib built by maturin
into the wheel, not a crate anyone consumes.

## Cutting a release

1. Bump `version` in the workspace `Cargo.toml` and in `python/pyproject.toml`.
   They are separate files and nothing forces them to agree; the release
   workflow checks the tag against the first one only.
2. Update `CHANGELOG.md`.
3. Commit, then tag and push:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

The workflow re-runs the tests, checks the tag against the manifest, does a
`cargo publish --dry-run`, then publishes. To rehearse without publishing,
run it from the Actions tab with **dry run** left on.

## What a release needs configured, once

| Where | What | Why |
|---|---|---|
| Repository secrets | `CARGO_REGISTRY_TOKEN` | A crates.io API token scoped to publish-update. crates.io also supports OIDC trusted publishing, which removes the stored secret — worth switching to after the first release. |
| pypi.org | A **pending publisher** for `rustypaper` | Trusted publishing: PyPI accepts a short-lived token from this workflow rather than a stored API token. It has to be added as *pending* because the project does not exist there yet. Owner `pgarrett-scripps`, repository `rustypaper`, workflow `release.yml`, environment `release`. |
| Repository environments | An environment named `release` | What the PyPI publisher binds to. Add required reviewers here if a release should need approval. |

## What ships

The wheel is the extension module and the Python package around it, and
nothing else. The default build is pure Rust — there is no pdfium to bundle,
no per-platform C library to keep in step, and no `PDFIUM_DYNAMIC_LIB_PATH`
for a user to discover. A wheel built `--features pdfium` would need all of
that; we do not build those.

One wheel per platform covers every supported Python, because the extension
is built against the stable ABI (`abi3-py39`).

`tests/corpus.rs` is excluded from the crate: it needs PDFs that are not ours
to redistribute. Unit and robustness tests ship and run anywhere; corpus tests
skip when `corpus/` is absent, which is why CI can run them at all.

## Before tagging, locally

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

## Verifying a release

```sh
cargo install rustypaper && rustypaper --help
pip install rustypaper && python -c "import rustypaper; print(rustypaper.__version__)"
```

The second is the one worth actually running: a wheel that imports on the
build machine and not on a clean one is the failure this packaging is shaped
to avoid.
