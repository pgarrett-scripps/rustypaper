"""Generate original, deterministic PDFs for the conversion contract tests.

Requires reportlab 4.4.9 only when regenerating the committed fixtures.
The fixture text is synthetic and released under CC0-1.0.
"""
import json
from pathlib import Path
import textwrap

from reportlab.pdfgen import canvas
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont

ROOT = Path(__file__).resolve().parent
FONTS = Path("/usr/share/fonts/truetype/dejavu")
pdfmetrics.registerFont(TTFont("FixtureSerif", str(FONTS / "DejaVuSerif.ttf")))
pdfmetrics.registerFont(TTFont("FixtureBold", str(FONTS / "DejaVuSerif-Bold.ttf")))
TITLE = "Reliable Widget Measurements"
PARAGRAPHS = {
    "Abstract": (
        "We describe a simple study of colored widgets on a wooden bench. "
        "The measurements were repeated under the same conditions on several days. "
        "This document contains original synthetic prose for software testing. "
        "It does not report an actual experiment or cite actual publications."
    ),
    "1 Introduction": (
        "The purpose of this study is to compare measurements of small widgets. "
        "A reliable instrument should give similar readings when the object is unchanged. "
        "Earlier observations suggested that careful placement was useful for repeatability. "
        "We therefore kept the position of each widget fixed throughout the observations. "
        "The study provides an example of ordinary prose with clear word boundaries. "
        "Every section should remain readable after conversion from the original document."
    ),
    "2 Methods": (
        "We placed the widgets on a level bench and recorded the width of each object. "
        "The same instrument was used for all measurements in the comparison. "
        "Before each reading the observer checked the scale against a reference block. "
        "The procedure was repeated three times for each widget on each day. "
        "The values were entered into a notebook in the order they were observed. "
        "The notebook remained available to the observer during the entire study."
    ),
    "3 Results": (
        "The measurements of the blue widgets were consistent across the repeated observations. "
        "The green widgets showed a small difference between the first and final readings. "
        "The difference was smaller than the smallest division marked on the instrument. "
        "We did not interpret that difference as evidence of a change in the objects. "
        "The observations illustrate why a clear account of measurement precision is useful. "
        "All of the reported observations belong to this synthetic example document."
    ),
    "4 Discussion": (
        "An observation is useful only when its context is clear to the reader. "
        "For that reason we describe the bench, the instrument, and the order of the readings. "
        "These details help distinguish an actual change from a difference in placement. "
        "The example does not attempt to establish a general rule for every kind of widget. "
        "A larger collection would be needed before drawing such a conclusion. "
        "A second observer could repeat the procedure with an independent instrument. "
        "That comparison would help identify any effect associated with the original observer. "
        "The records should retain their order so that gradual changes remain visible. "
        "A summary that keeps only an average could conceal those changes from the reader. "
        "Likewise, a document converter must retain the order of sentences within each column. "
        "The first column ends before the reader begins the second column on the same page. "
        "A heading identifies the paragraphs that follow it and should not absorb nearby prose. "
        "The bibliography contains three invented entries with stable labels and dates. "
        "Those entries allow a test to check the separation between prose and reference metadata. "
        "The names are examples and do not identify authors of actual published work. "
        "The original text and its generated PDF are committed together for reproducibility. "
        "Neither a remote archive nor a network connection is needed to run these tests. "
        "Both page layouts use the same words, so differences in their output expose layout errors. "
        "A test compares complete sentences as well as the sequence of section headings. "
        "This makes it possible to detect missing words and accidental joins between columns."
    ),
    "5 Conclusion": (
        "In this study, we found that fixed placement supported consistent widget measurements. "
        "The example is intended to exercise titles, headings, paragraphs, and references. "
        "A reader should encounter the introduction before the methods and the results. "
        "No content from separate columns should be fused into the same sentence. "
        "The conclusion should end before the bibliography begins on the printed page."
    ),
    "References": (
        "[1] Ada Example. Reliable widget placement. Synthetic Journal, 2020.\n"
        "[2] Ben Sample. Reading a measurement scale. Synthetic Journal, 2021.\n"
        "[3] Clara Fixture. Comparing repeated observations. Synthetic Journal, 2022."
    ),
}


def generate(name: str, columns: int) -> None:
    pdf = canvas.Canvas(str(ROOT / name), pagesize=(612, 792), invariant=1,
                        pageCompression=0)
    pdf.setTitle(TITLE)
    pdf.setAuthor("Example Author")
    pdf.setFont("FixtureBold", 20)
    pdf.drawCentredString(306, 750, TITLE)
    pdf.setFont("FixtureSerif", 10)
    pdf.drawCentredString(306, 729, "Example Author")
    x, y = 48, 695
    width = 516 if columns == 1 else 246
    column = 0

    def next_column():
        nonlocal x, y, column
        column += 1
        if column == columns:
            pdf.showPage()
            column = 0
        x = 48 + column * 270
        y = 695

    for heading, paragraph in PARAGRAPHS.items():
        lines = []
        for part in paragraph.splitlines():
            lines.extend(textwrap.wrap(part, width=90 if columns == 1 else 42))
        if y - (len(lines) * 14 + 40) < 55:
            next_column()
        pdf.setFont("FixtureBold", 14)
        pdf.drawString(x, y, heading)
        y -= 23
        pdf.setFont("FixtureSerif", 10)
        for line in lines:
            assert pdf.stringWidth(line, "FixtureSerif", 10) <= width
            pdf.drawString(x, y, line)
            y -= 14
        y -= 18
    pdf.save()


if __name__ == "__main__":
    generate("single-column.pdf", 1)
    generate("two-column.pdf", 2)
    (ROOT / "expected-text.json").write_text(
        json.dumps({"title": TITLE, "paragraphs": PARAGRAPHS}, indent=2) + "\n"
    )
