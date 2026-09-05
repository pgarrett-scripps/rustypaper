"""Committed layout fixtures must preserve complete prose across column gutters."""

import json
from pathlib import Path

import pytest
import rustypaper

FIXTURES = Path(__file__).parent / "fixtures"
EXPECTED = json.loads((FIXTURES / "expected-text.json").read_text())


@pytest.mark.parametrize("name", ["single-column.pdf", "two-column.pdf"])
def test_complete_paragraphs_survive_column_separation(name: str) -> None:
    markdown = rustypaper.to_markdown(str(FIXTURES / name))
    for heading, paragraph in EXPECTED["paragraphs"].items():
        for text in paragraph.splitlines():
            assert text in markdown, f"{name}: missing or interleaved prose in {heading}"
