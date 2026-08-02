"""The caveman argument on the Python surface.

Compression is what makes this binding usable for feeding papers to a model
that charges by the token, and it is the one argument whose failure is silent:
a level the extension does not recognise, or one that quietly does nothing,
produces a perfectly good conversion that is simply larger than the caller
asked for. Nobody notices until the bill.

Run with the repo's own corpus:  pytest python/tests -q
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))

rustypdf = pytest.importorskip("rustypdf")

CORPUS = sorted((ROOT / "corpus").glob("*.pdf"))
pytestmark = pytest.mark.skipif(
    not CORPUS, reason="no corpus; run scripts/fetch-corpus.sh"
)


@pytest.fixture(scope="module")
def paper() -> str:
    return str(CORPUS[0])


def test_levels_shrink_monotonically(paper: str) -> None:
    """off >= light >= hard, and each level actually does something.

    Asserted as a chain rather than against fixed sizes so it keeps working as
    the word lists change — what must hold is that a stronger level is never
    larger, and that light and hard are not silently no-ops.
    """
    off = rustypdf.to_markdown(paper)
    light = rustypdf.to_markdown(paper, "light")
    hard = rustypdf.to_markdown(paper, "hard")

    assert len(off) > len(light) > len(hard), (len(off), len(light), len(hard))
    assert rustypdf.to_markdown(paper, "off") == off
    assert rustypdf.to_markdown(paper, "none") == off


def test_unknown_level_raises(paper: str) -> None:
    """A typo must fail, not fall through to no compression."""
    with pytest.raises(ValueError, match="ligth"):
        rustypdf.to_markdown(paper, "ligth")


def test_document_model_takes_the_same_levels(paper: str) -> None:
    """`to_document` compresses too, so the two views cannot disagree."""
    full = rustypdf.to_document(paper)
    light = rustypdf.to_document(paper, "light")

    assert len(full["blocks"]) == len(light["blocks"]), "compression dropped blocks"
    joined = lambda d: "".join(b.get("text") or "" for b in d["blocks"])  # noqa: E731
    assert len(joined(light)) < len(joined(full))


def test_content_words_survive_light(paper: str) -> None:
    """Light drops only closed-class words.

    The title is the cheapest thing to check that on: it is content words
    almost end to end, so if compression is reaching them it shows up here.
    """
    full = rustypdf.to_document(paper)["title"] or ""
    light = rustypdf.to_document(paper, "light")["title"] or ""
    if not full:
        pytest.skip("this corpus paper has no detected title")

    dropped = [w for w in full.split() if w not in light.split()]
    assert all(len(w) <= 4 for w in dropped), f"light dropped content words: {dropped}"
