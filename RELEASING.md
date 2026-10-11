# Releasing

One project, one version, one tag. Pushing `vX.Y.Z` publishes the crate to
crates.io, the wheels and source distribution to PyPI, and standalone executables
to GitHub Releases. All packages are named `rustypaper`.

The latest release before standalone executables and the Python CLI was 0.2.1.
Do not retag or republish that version to ship these additions.

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
   Refresh `Cargo.lock` with `cargo check`. The release workflow requires the Rust
   version, Python version, and publishing tag to agree.
2. Commit, then tag and push:

   ```sh
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

The workflow runs Rust tests and a crate packaging check, builds all five native
executables and all five wheels, and tests each executable after archive extraction.
Each wheel is installed on a matching runner, where both `rustypaper` and
`python -m rustypaper` must pass the CLI checks. All builds and checks must pass
before any publishing job starts.

Relevant pull requests run the same packaging jobs without publishing. To rehearse
manually, run the workflow from the Actions tab with **dry run** left on. A manual
run with publishing enabled must target the matching version tag, never a branch.

Native executables are built and exercised on matching Linux x86_64 and ARM64,
macOS Intel and Apple Silicon, and Windows x86_64 runners. Linux binaries use
Ubuntu 22.04 and require glibc 2.35 or newer. Wheel compatibility is handled
separately by maturin's manylinux build.

## What a release needs configured, once

| Where | What | Why |
|---|---|---|
| Repository secrets | `CARGO_REGISTRY_TOKEN` | A crates.io API token scoped to publish-update. crates.io also supports OIDC trusted publishing, which removes the stored secret — worth switching to after the first release. |
| pypi.org | A **trusted publisher** for `rustypaper` | PyPI accepts a short-lived token from this workflow rather than a stored API token. Owner `pgarrett-scripps`, repository `rustypaper`, workflow `release.yml`, environment `pypi`. All four claims must match exactly; one disagreeing claim fails as `invalid-publisher`, which reads like a missing publisher rather than a mismatched one. Before the first upload it has to be added as a *pending* publisher, since the project does not exist on PyPI until then; that step is done. |
| Repository environments | An environment named `pypi` | What the PyPI publisher binds to. Add required reviewers here if a release should need approval. |

## What ships

The wheel is the extension module and the Python package around it, and
includes a `rustypaper` console entry point and `python -m rustypaper`. Both
call the same Rust command implementation as the standalone executable, so no
second executable is bundled in the wheel. The PDF reader is pure Rust:
no native library beside it, nothing per-platform to keep in step, and no
environment variable for a user to discover.

One wheel per platform covers every supported Python, because the extension
is built against the stable ABI (`abi3-py39`).

Standalone executables are archived as
`rustypaper-X.Y.Z-TARGET.tar.gz`, or `.zip` on Windows. Each archive contains a
matching directory with the executable, README, and both license files. This
layout follows cargo-binstall's default discovery rules. GitHub Releases also
includes `SHA256SUMS`. The release job verifies the hashes before upload.

GitHub Actions keeps native binaries under `binary-*` artifacts, wheels under
`wheel-*`, and the source distribution under `sdist`. PyPI downloads only the
wheel and source artifacts. Dry runs leave all artifacts available for inspection.

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
cargo binstall rustypaper
rustypaper --version
pip install rustypaper
python -m rustypaper --version
python scripts/smoke_cli.py rustypaper
python scripts/smoke_cli.py python -m rustypaper
```

Use a fresh virtual environment for the Python installation and leave `PYTHONPATH`
unset. The smoke checks run outside the source tree using the committed synthetic
PDF fixtures. They exercise all output formats, file and batch output, partial
batch failures, compression, diagnostics, usage errors, and closed output pipes.
