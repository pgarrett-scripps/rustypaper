"""Structure-aware conversion of born-digital scientific PDFs.

The heavy lifting is a Rust extension module; this package is the ergonomic surface around it.

    import rustypaper
    print(rustypaper.to_markdown("paper.pdf"))
    doc = rustypaper.to_document("paper.pdf")
    for block in doc["blocks"]:
        print(block["kind"], block["text"][:60])
    for section in doc["sections"]:
        print(section["level"], section["title"], section["start"], section["end"])

Wanting both the prose and the structure is the common case, and converting twice to get them
is a whole second pipeline run:

    result = rustypaper.convert("paper.pdf")
    result.markdown, result.document
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, NamedTuple

from ._rustypaper import (  # noqa: E402
    ScannedDocument,
    __version__,
    convert_json,
    extract_json,
    to_json,
    to_markdown,
    to_text,
    to_typst,
)

__all__ = [
    "ScannedDocument",
    "__version__",
    "to_markdown",
    "to_typst",
    "to_text",
    "to_document",
    "convert",
    "Conversion",
    "extract",
]


class Conversion(NamedTuple):
    """Both renderings of one conversion."""

    markdown: str
    document: dict[str, Any]


def to_document(path: str | Path, caveman: str | None = None) -> dict[str, Any]:
    """Convert a PDF to the document model.

    Returns a dict with ``title``, ``blocks`` and ``sections``. Each block has ``kind``, ``text``,
    ``page``, ``bbox`` and ``size``. This is the real output of the pipeline — Markdown is one
    rendering of it.

    ``sections`` is the document's section tree, derived from the headings the pipeline typed.
    Each section carries:

    * ``title`` — the heading as printed, numbering and all, or ``None`` for the front matter
      (title, authors, abstract), which no heading introduces;
    * ``level`` — 1 for a top-level section, 0 for the front matter;
    * ``start`` and ``end`` — a half-open range into ``blocks``. ``start`` is the heading block
      itself, so ``blocks[start:end]`` is the section as a reader sees it and ``blocks[start +
      1:end]`` is its body. Nested subsections are included in the range;
    * ``first_page`` and ``last_page`` — zero-based, inclusive;
    * ``children`` — subsections, in reading order.

    Sibling ranges never overlap, and a child's range lies inside its parent's.

    ``caveman`` is ``"off"`` (the default), ``"light"`` or ``"hard"``. ``light`` drops articles,
    copulas and stock phrases; ``hard`` also drops prepositions, pronouns and filler, for about
    a quarter fewer words. Mathematics, tables and bibliography entries are exempt at every
    level.
    """
    return json.loads(to_json(str(path), caveman))


def convert(path: str | Path, caveman: str | None = None) -> Conversion:
    """Convert a PDF once, returning its Markdown and its document model.

    ``rustypaper.to_markdown(p)`` followed by ``rustypaper.to_document(p)`` reads and interprets
    the PDF twice for two views of the same result. This does the work once:

        markdown, document = rustypaper.convert("paper.pdf")

    ``caveman`` is as for :func:`to_document`, and applies to both renderings.
    """
    markdown, document = convert_json(str(path), caveman)
    return Conversion(markdown, json.loads(document))


def extract(path: str | Path) -> dict[str, Any]:
    """Extract page primitives without interpreting them.

    Glyphs, vector paths and images per page, for diagnosing conversion problems.
    """
    return json.loads(extract_json(str(path)))
