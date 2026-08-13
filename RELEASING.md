# Releasing

One project, one version, one tag. Pushing `vX.Y.Z` publishes the crate to
crates.io and the wheels to PyPI, both named `rustypaper`.

Published so far: `rustypaper` 0.1.0, 0.1.1 and 0.2.0 on crates.io, 0.1.1 and
0.2.0 on PyPI as five platform wheels and an sdist. `rustium-pdf` 0.1.0 and
0.1.1 on crates.io.

## Order matters

`rustium-pdf` goes first. rustypaper resolves it from the registry as
`version = "0.1.1"`, so `cargo publish` fails outright if that version is not
on the index yet. Nothing here resolves it by path — a `[patch.crates-io]`
override during development must not be committed, or the publish will carry
a dependency the registry cannot see.

```
rustium-pdf  v0.1.0  →  crates.io
                            │
rustypaper   v0.1.1  ───────┴──→  crates.io  +  PyPI
```

`rustypaper-py` is marked `publish = false`: it is a cdylib built by maturin
into the wheel, not a crate anyone consumes.

## Cutting a release

1. Bump `version` in the workspace `Cargo.toml` and in `python/pyproject.toml`.
   They are separate files and nothing forces them to agree; the release
   workflow checks the tag against the first one only.
2. Commit, then tag and push:

   ```sh
   git tag v0.1.2
   git push origin v0.1.2
   ```

The workflow re-runs the tests, checks the tag against the manifest, does a
`cargo publish --dry-run`, then publishes. To rehearse without publishing,
run it from the Actions tab with **dry run** left on.

## What a release needs configured, once

| Where | What | Why |
|---|---|---|
| Repository secrets | `CARGO_REGISTRY_TOKEN` | A crates.io API token scoped to publish-update. crates.io also supports OIDC trusted publishing, which removes the stored secret — worth switching to after the first release. |
| pypi.org | A **trusted publisher** for `rustypaper` | PyPI accepts a short-lived token from this workflow rather than a stored API token. Owner `pgarrett-scripps`, repository `rustypaper`, workflow `release.yml`, environment `pypi`. All four claims must match exactly; one disagreeing claim fails as `invalid-publisher`, which reads like a missing publisher rather than a mismatched one. Before the first upload it has to be added as a *pending* publisher, since the project does not exist on PyPI until then; that step is done. |
| Repository environments | An environment named `pypi` | What the PyPI publisher binds to. Add required reviewers here if a release should need approval. |

## What ships

The wheel is the extension module and the Python package around it, and
nothing else. The PDF reader is pure Rust, so the wheel is self-contained:
no native library beside it, nothing per-platform to keep in step, and no
environment variable for a user to discover.

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
