# rustypdf2markdown

Structure-aware conversion of born-digital scientific PDFs to Markdown, Typst and JSON. Fast,
CPU-only, no models.

The good open-source converters (Marker, MinerU, Docling, Nougat) are Python stacks that want a
GPU; GROBID is CPU-only and fast but is a JVM service that emits TEI and ignores maths. The Rust
crates that exist are generic text extractors with no notion of a paper. This aims at the gap:
**structure-aware, maths-aware, CPU-only, single binary.**

> Status: **M1**. Converts to Markdown with correct reading order on one- and two-column
> papers. Tables, figures and maths are not handled yet — see [milestones](#milestones).

## Getting started

```sh
scripts/fetch-pdfium.sh     # vendored pdfium (BSD-3-Clause), pinned build
scripts/fetch-corpus.sh     # evaluation corpus of arXiv papers, not committed
cargo build --release
```

```sh
# Convert.
./target/release/rp2m convert corpus/resnet.pdf
./target/release/rp2m convert corpus/resnet.pdf --format json

# Diagnostics.
./target/release/rp2m probe corpus/resnet.pdf --pages   # counts, fonts, detected gutters
./target/release/rp2m text  corpus/resnet.pdf --geometry # reconstructed lines
./target/release/rp2m dump  corpus/resnet.pdf --page 0 --pretty
```

`probe` prints per-page glyph/path/image counts and the font histogram. The font histogram is
the first thing to look at when text comes out wrong: TeX maths fonts (`CMMI`, `CMSY`, `CMEX`)
where prose is expected means the Unicode repair pass has work to do.

## Design

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the pipeline, the coordinate convention,
and the three things M0 discovered the hard way — most importantly that pdfium-render's
`thread_safe` feature does no locking at all.

The short version: the backend produces a `PageRaw` of glyphs, paths and images, and every later
stage is a pass over an IR. The `Document` JSON is the contract; Markdown and Typst are
renderings of it.

Unicode repair turned out not to be needed — pdfium's glyph-name fallback already resolves TeX
ligatures, so the tables the plan budgeted for were dropped. De-hyphenation is still outstanding
and is harder than expected: pdfium *removes* soft line-break hyphens, so the artifact is
`learn ing` rather than `learn- ing` and there is no hyphen left to key off.

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
| **M2** | Figures, captions, footnotes, lists, de-hyphenation | next |
| **M3** | Tables (booktabs → lattice → stream) | |
| **M4** | Maths detection and reconstruction | |
| **M5** | References, citation linking, CSL-JSON | |
| **M6** | Typst emitter, performance pass, batch mode | |

## Testing

```sh
cargo test            # unit tests always run; corpus tests skip if corpus/ is empty
```

Integration tests live in `rustypdf/tests/corpus.rs` and run against real papers. They skip
rather than fail when the corpus is absent, so a fresh clone is green.

## Licence

MIT OR Apache-2.0. Bundled pdfium is BSD-3-Clause (see `vendor/pdfium/LICENSE` after fetching).
