These original synthetic test documents are released under CC0-1.0.
Their embedded DejaVu font license is included in FONT-LICENSE.txt.

Regenerate with `python generate.py` using ReportLab 4.4.9 and the DejaVu Serif fonts
from `fonts-dejavu-core`. The output uses fixed metadata and no random values.
The tests consume the committed PDFs and do not require ReportLab or a network.

The single-column document also serves as PeerReviewAgents' ingestion fixture.
Copy the regenerated file to its `tests/fixtures/` directory when updating it.
The two-column document exercises ragged column edges with aligned baselines.
The expected text is generated alongside the PDFs. Tests require every complete
paragraph to survive conversion, including paragraphs crossing the sparse gutter.

Two limitations remain visible in the two-column output: the author joins the
title, and the first page's Results heading precedes Methods. The paragraph
regression covers column separation. Title and section ordering need a separate
layout change measured against the full corpus.
