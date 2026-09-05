"""The Python contract always runs against a committed fixture."""
from pathlib import Path
import sys

import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "python"))


@pytest.fixture(scope="module")
def paper() -> str:
    return str(Path(__file__).parent / "fixtures" / "single-column.pdf")
