"""Structure-aware conversion of born-digital scientific PDFs.

The heavy lifting is a Rust extension module; this package is the ergonomic surface around it.

    import rustypaper
    print(rustypaper.to_markdown("paper.pdf"))
    doc = rustypaper.to_document("paper.pdf")
    for block in doc["blocks"]:
        print(block["kind"], block["text"][:60])
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from ._rustypaper import ScannedDocument, __version__, extract_json, to_json, to_markdown  # noqa: E402

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

    ``caveman`` is ``"off"`` (the default), ``"light"`` or ``"hard"``. ``light`` drops articles,
    copulas and stock phrases; ``hard`` also drops prepositions, pronouns and filler, for about
    a quarter fewer words. Mathematics, tables and bibliography entries are exempt at every
    level.
    """
    return json.loads(to_json(str(path), caveman))


def extract(path: str | Path) -> dict[str, Any]:
    """Extract page primitives without interpreting them.

    Glyphs, vector paths and images per page, for diagnosing conversion problems.
    """
    return json.loads(extract_json(str(path)))
