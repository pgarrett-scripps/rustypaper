"""Structure-aware conversion of born-digital scientific PDFs.

The heavy lifting is a Rust extension module; this package is the ergonomic surface around it.

    import rustypdf
    print(rustypdf.to_markdown("paper.pdf"))
    doc = rustypdf.to_document("paper.pdf")
    for block in doc["blocks"]:
        print(block["kind"], block["text"][:60])
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

# Point pdfium at the copy shipped inside this package, before the extension
# loads and caches its binding. An installed wheel has no `vendor/` directory
# and the extension's other search paths are relative to the working
# directory, so without this `import rustypdf` works from a checkout and fails
# everywhere else. An explicit PDFIUM_DYNAMIC_LIB_PATH still wins — someone
# who set it meant it.
if "PDFIUM_DYNAMIC_LIB_PATH" not in os.environ:
    for _candidate in (
        Path(__file__).resolve().parent,                          # bundled in the wheel
        Path(__file__).resolve().parents[2] / "vendor/pdfium/lib",  # source checkout
    ):
        if list(_candidate.glob("*pdfium*")):
            os.environ["PDFIUM_DYNAMIC_LIB_PATH"] = str(_candidate)
            break

from _rustypdf import ScannedDocument, __version__, extract_json, to_json, to_markdown  # noqa: E402

__all__ = [
    "ScannedDocument",
    "__version__",
    "to_markdown",
    "to_document",
    "extract",
]


def to_document(path: str | Path, caveman: str | None = None) -> dict[str, Any]:
    """Convert a PDF to the document model.

    Returns a dict with ``title`` and ``blocks``; each block has ``kind``, ``text``, ``page``,
    ``bbox`` and ``size``. This is the real output of the pipeline — Markdown is one rendering
    of it.

    ``caveman`` is ``"off"`` (the default), ``"light"`` or ``"hard"`` — see
    :mod:`rustypdf.compress` in the Rust crate for what each level drops.
    Mathematics, tables and bibliography entries are exempt at every level.
    """
    return json.loads(to_json(str(path), caveman))


def extract(path: str | Path) -> dict[str, Any]:
    """Extract page primitives without interpreting them.

    Glyphs, vector paths and images per page, for diagnosing conversion problems.
    """
    return json.loads(extract_json(str(path)))
