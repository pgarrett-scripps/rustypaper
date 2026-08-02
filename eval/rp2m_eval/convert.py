"""Running the converter.

Prefers the compiled extension module and falls back to the CLI, so the harness works whether or
not the Python bindings have been built.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

try:  # pragma: no cover - depends on the environment
    import rustypdf as _module

    BACKEND = "extension"
except ImportError:  # pragma: no cover
    _module = None
    BACKEND = "cli"

REPO = Path(__file__).resolve().parents[2]
CLI = REPO / "target" / "release" / "rp2m"


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
            f"neither the rustypdf extension nor {CLI} is available; "
            "run `cargo build --release`"
        )
    result = subprocess.run(
        [str(CLI), *args], capture_output=True, text=True, cwd=REPO, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(f"rp2m failed: {result.stderr.strip()}")
    return result.stdout
