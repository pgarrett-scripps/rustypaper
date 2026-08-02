# rustypdf2markdown

Structure-aware conversion of born-digital scientific PDFs to Markdown, Typst and JSON. Fast,
CPU-only, no models.

The good open-source converters (Marker, MinerU, Docling, Nougat) are Python stacks that want a
GPU; GROBID is CPU-only and fast but is a JVM service that emits TEI and ignores maths. The Rust
crates that exist are generic text extractors with no notion of a paper. This aims at the gap:
**structure-aware, maths-aware, CPU-only, single binary.**

> Status: **feature complete**. Converts one- and two-column papers to Markdown, Typst, plain
> text or JSON, with reading order, figures, tables, mathematics and references.

## Getting started

```sh
scripts/fetch-pdfium.sh     # vendored pdfium (BSD-3-Clause), pinned build
scripts/fetch-corpus.sh     # evaluation corpus of arXiv papers, not committed
scripts/build.sh            # cargo build --release, plus installing the Python extension
```

```sh
# Convert.
./target/release/rp2m convert corpus/resnet.pdf
./target/release/rp2m convert corpus/resnet.pdf --format typst --assets figures/
./target/release/rp2m convert corpus/*.pdf --out out/        # batch

# Diagnostics.
./target/release/rp2m probe corpus/resnet.pdf --pages   # counts, fonts, detected gutters
./target/release/rp2m text  corpus/resnet.pdf --geometry # reconstructed lines
./target/release/rp2m dump  corpus/resnet.pdf --page 0 --pretty
```

`probe` prints per-page counts, the font histogram and the detected gutters. Gutters are the
first thing to check when a two-column paper comes out interleaved; the font histogram is the
first thing to check when text comes out wrong.

## Design

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the pipeline, the coordinate convention,
and the eight things M0 and M1 discovered the hard way — most importantly that pdfium-render's
`thread_safe` feature does no locking at all.

The short version: the backend produces a `PageRaw` of glyphs, paths and images, and every later
stage is a pass over an IR. The `Document` JSON is the contract; Markdown, Typst and plain text
are renderings of it.

| | |
|---|---|
| speed | 3.8–7.0 ms/page, single process |
| corpus | 10 papers: ML, pure maths, physics, biology, statistics |
| memory | 17–31 MB peak for a whole paper |
| footprint | 2.3 MB binary + 7.3 MB pdfium, no models |

Unicode repair turned out not to be needed — pdfium's glyph-name fallback already resolves TeX
ligatures, so the tables the plan budgeted for were dropped. De-hyphenation could not work the
way the plan assumed either: pdfium *removes* soft line-break hyphens, so there is no hyphen to
key off. Words split across a break are rejoined using the document's own vocabulary instead,
which needs no word list and knows the paper's jargon.

Maths is reconstructed geometrically, MaxTract-style, from exact glyph identities and positions
rather than by OCR — a born-digital PDF hands you perfect character information, so image-to-
LaTeX models are solving a problem this pipeline does not have. Equations carry a confidence
score and fall back to a rendered crop rather than emitting confident-looking nonsense.

Scanned documents are explicitly out of scope: `extract` fails with `Error::Scanned` rather than
pretending.

## Milestones

| | | status |
|---|---|---|
| **M0** | Backend, `PageRaw`, CLI, corpus, thread-safety spike | done |
| **M1** | Lines/words, furniture removal, columns, reading order → Markdown | done |
| **M2** | Figures, captions, footnotes, lists, de-hyphenation | done |
| **M3** | Tables | done |
| **M4** | Maths detection and reconstruction | done |
| **M5** | References and citation linking | done |
| **M6** | Typst emitter, performance pass, batch mode | done |

## Python

The core is Rust; the tooling around it is Python, because evaluation, corpus management and
comparison against other converters are scripting jobs.

```sh
scripts/build.sh
PYTHONPATH=python python3 -c "
import rustypdf
print(rustypdf.to_markdown('corpus/resnet.pdf')[:80])
doc = rustypdf.to_document('corpus/resnet.pdf')   # the document model as a dict
"
```

`ScannedDocument` is raised for image-only PDFs, so callers can route those to an OCR pipeline
instead. Conversion releases the GIL.

## Evaluation

Quality is measured, not eyeballed. Papers submitted to arXiv as TeX source come with the prose
their PDF was rendered from, which is free ground truth for exactly this document class — for
the subset of papers that have it. PDF-only submissions have none, and are reported as skipped
rather than scored.

```sh
cd eval && PYTHONPATH=.:../python python3 -m rp2m_eval
```

Current mean bigram recall is **0.894** across the nine scorable papers. See
[`eval/README.md`](eval/README.md) for what the metrics mean and why plain edit distance is the
wrong primary measure here.

## Testing

```sh
cargo test                                    # unit tests always run; corpus tests skip if
                                              # corpus/ is empty
python3 -m unittest discover -s eval/tests    # the eval harness's own tests
cd eval && PYTHONPATH=.:../python python3 -m rp2m_eval --baseline baseline.json
```

Integration tests live in `rustypdf/tests/corpus.rs` and run against real papers. They skip
rather than fail when the corpus is absent, so a fresh clone is green.

## Licence

MIT OR Apache-2.0. Bundled pdfium is BSD-3-Clause (see `vendor/pdfium/LICENSE` after fetching).
