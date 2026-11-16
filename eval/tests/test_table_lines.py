"""A tripwire against prose being dealt into tables.

Release 0.2.4 rendered the exosome pilot paper with 32 Markdown table lines; the
rule-coalescing that shipped after it welded a figure's frame to the table below and dealt
the prose between them into phantom columns, and the same paper printed 105. A table-line
count is a blunt instrument, but it is exactly the right bluntness for that failure: prose
swallowed into a table inflates it by tens of lines at a stroke, while honest changes to
table detection move it by a handful.

`eval/table_lines.json` pins, for every corpus and pilot paper, the count at 0.2.4 (context
for the reader, never compared against) and the verified-good count for the current
converter. This test converts each paper that is present and fails when the live count
leaves a narrow band around the pinned one — too high reads as prose-into-tables, too low
as lost tables. Refresh the pinned counts deliberately, the way `eval/baseline.json` is
refreshed, after verifying the new output by eye:

    PYTHONPATH=eval:python python3 eval/tests/test_table_lines.py --regen

Corpus papers live in `corpus/` (fetched by scripts/fetch-corpus.sh); pilot papers in the
directory named by RUSTYPAPER_PILOT_DIR, defaulting to /home/ty/pra-spot/papers. Papers
whose PDF is absent are skipped, so the test degrades to whatever is present.
"""

import json
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from rustypaper_eval import convert  # noqa: E402

REPO = Path(__file__).resolve().parents[2]
REFERENCE = REPO / "eval" / "table_lines.json"
CORPUS_DIR = REPO / "corpus"
PILOT_DIR = Path(os.environ.get("RUSTYPAPER_PILOT_DIR", "/home/ty/pra-spot/papers"))


def table_lines(pdf: Path) -> int:
    markdown = convert.to_markdown(pdf)
    return sum(1 for line in markdown.splitlines() if line.startswith("|"))


def slack(pinned: int) -> int:
    """How far a count may drift before it is a failure.

    Absolute 3 covers borderline tables on small papers; 15% covers papers whose hundreds
    of table lines shift by a row or two when an unrelated pass moves a boundary. Prose
    swallowed into a table blows through either within a single paragraph.
    """
    return max(3, round(pinned * 0.15))


class TestTableLines(unittest.TestCase):
    def check(self, directory: Path, pinned: dict[str, dict[str, int]]) -> None:
        if not directory.is_dir():
            self.skipTest(f"{directory} is absent")
        missing = [name for name in pinned if not (directory / name).exists()]
        failures = []
        for name, counts in pinned.items():
            pdf = directory / name
            if not pdf.exists():
                continue
            count = table_lines(pdf)
            good = counts["table_lines"]
            if count > good + slack(good):
                failures.append(
                    f"{name}: {count} table lines against {good} pinned "
                    f"(0.2.4 gave {counts['v024']}) — prose is being dealt into tables"
                )
            elif count < good - slack(good):
                failures.append(
                    f"{name}: {count} table lines against {good} pinned "
                    f"(0.2.4 gave {counts['v024']}) — tables were lost"
                )
        self.assertEqual(failures, [], "\n" + "\n".join(failures))
        if missing == list(pinned):
            self.skipTest(f"no pinned papers present in {directory}")

    def test_corpus_papers(self):
        pinned = json.loads(REFERENCE.read_text())
        self.check(CORPUS_DIR, pinned["corpus"])

    def test_pilot_papers(self):
        pinned = json.loads(REFERENCE.read_text())
        self.check(PILOT_DIR, pinned["pilot"])


def _regen() -> None:
    reference = json.loads(REFERENCE.read_text())
    for directory, key in [(CORPUS_DIR, "corpus"), (PILOT_DIR, "pilot")]:
        for name, counts in reference[key].items():
            pdf = directory / name
            if pdf.exists():
                counts["table_lines"] = table_lines(pdf)
    REFERENCE.write_text(json.dumps(reference, indent=2) + "\n")
    print(f"refreshed {REFERENCE}")


if __name__ == "__main__":
    if "--regen" in sys.argv:
        _regen()
    else:
        unittest.main()
