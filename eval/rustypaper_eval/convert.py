"""Running the converter.

Prefers the compiled extension module and falls back to the CLI, so the harness works whether or
not the Python bindings have been built.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

try:  # pragma: no cover - depends on the environment
    import rustypaper as _module

    BACKEND = "extension"
except ImportError:  # pragma: no cover
    _module = None
    BACKEND = "cli"

REPO = Path(__file__).resolve().parents[2]
CLI = REPO / "target" / "release" / "rustypaper"
BUILT_EXTENSION = REPO / "target" / "release" / "lib_rustypaper.so"


def loaded_extension() -> Path | None:
    """The file Python actually loaded the extension from, or None if it did not.

    Ask the module rather than guessing a path. A hardcoded location is wrong the moment
    build.sh moves or renames what it installs — which is exactly what happened: this file
    named `python/_rustypaper.so` while build.sh writes `python/rustypaper/_rustypaper.abi3.so`,
    so the staleness check below never once ran. The imported module is the only source of
    truth for which binary is being measured.
    """
    extension = getattr(_module, "_rustypaper", None)
    path = getattr(extension, "__file__", None)
    return Path(path) if path else None


def _warn_if_stale() -> None:
    """Shout if the loaded extension is older than the last Rust build.

    Worth the code: measuring a stale binary silently reports "no change" for a change that
    worked, which is the most expensive kind of wrong answer a harness can give. It hid a real
    +0.013 improvement once already.
    """
    import sys

    installed = loaded_extension()
    if installed is None or not (BUILT_EXTENSION.exists() and installed.exists()):
        return
    if BUILT_EXTENSION.stat().st_mtime > installed.stat().st_mtime + 1:
        print(
            f"warning: {installed} is older than the last cargo build; "
            "scores describe stale code. Run scripts/build.sh.",
            file=sys.stderr,
        )


if _module is not None:
    _warn_if_stale()


def to_markdown(pdf: Path) -> str:
    if _module is not None:
        return _module.to_markdown(str(pdf))
    return _run(["convert", str(pdf), "--format", "md"])


def to_document(pdf: Path) -> dict:
    if _module is not None:
        return _module.to_document(str(pdf))
    return json.loads(_run(["convert", str(pdf), "--format", "json"]))


def _run(args: list[str]) -> str:
    if not CLI.exists():
        raise RuntimeError(
            f"neither the rustypaper extension nor {CLI} is available; "
            "run `cargo build --release`"
        )
    result = subprocess.run(
        [str(CLI), *args], capture_output=True, text=True, cwd=REPO, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(f"rustypaper failed: {result.stderr.strip()}")
    return result.stdout
