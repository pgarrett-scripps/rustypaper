"""The rest of the Python surface: the section map, the emitters, one-run conversion.

What these are guarding is the *contract*, not the conversion. A consumer indexes into
``blocks`` with the ranges ``sections`` gives it, so a range that is off by one is a paragraph
attributed to the wrong section — silently, and in a shape that looks entirely plausible.

Run with the repo's own corpus:  pytest python/tests -q
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

rustypaper = pytest.importorskip("rustypaper")

CORPUS = sorted((ROOT / "corpus").glob("*.pdf"))
pytestmark = pytest.mark.skipif(
    not CORPUS, reason="no corpus; run scripts/fetch-corpus.sh"
)


@pytest.fixture(scope="module")
def paper() -> str:
    return str(CORPUS[0])


@pytest.fixture(scope="module")
def document(paper: str) -> dict:
    return rustypaper.to_document(paper)


def walk(sections: list[dict]):
    for section in sections:
        yield section
        yield from walk(section["children"])


def test_the_document_carries_a_section_map(document: dict) -> None:
    assert set(document) >= {"title", "blocks", "sections"}, "the old keys must stay"
    assert document["sections"], "no sections at all"


def test_section_ranges_index_the_blocks(document: dict) -> None:
    """Every range is valid, ordered, and names the heading it starts at."""
    blocks = document["blocks"]
    for section in walk(document["sections"]):
        assert 0 <= section["start"] < section["end"] <= len(blocks)
        assert section["first_page"] <= section["last_page"]
        if section["title"] is None:
            assert section["level"] == 0, "only the front matter goes untitled"
        else:
            assert blocks[section["start"]]["text"].strip() == section["title"].strip()
            assert blocks[section["start"]]["kind"]["type"] == "heading"

    # The top level tiles the document: every block belongs to exactly one of them.
    cursor = 0
    for section in document["sections"]:
        assert section["start"] == cursor
        cursor = section["end"]
    assert cursor == len(blocks)


def test_children_are_contained_by_their_parent(document: dict) -> None:
    for section in walk(document["sections"]):
        for child in section["children"]:
            assert child["start"] > section["start"]
            assert child["end"] <= section["end"]
            assert child["level"] > section["level"]


def test_one_conversion_gives_both_views(paper: str) -> None:
    """`convert` is `to_markdown` and `to_document` for the price of one pipeline run."""
    result = rustypaper.convert(paper)
    assert result.markdown == rustypaper.to_markdown(paper)
    assert result.document == rustypaper.to_document(paper)

    markdown, document = result  # and it unpacks
    assert markdown and document["blocks"]


def test_convert_takes_the_caveman_level(paper: str) -> None:
    assert len(rustypaper.convert(paper, "hard").markdown) < len(
        rustypaper.convert(paper).markdown
    )


def test_typst_and_text_render(paper: str) -> None:
    typst = rustypaper.to_typst(paper)
    text = rustypaper.to_text(paper)
    assert "#import" in typst, "Typst output should carry its preamble"
    assert text.strip(), "plain text should not be empty"
    # Plain text is the same document with the markup taken away.
    assert "#import" not in text
