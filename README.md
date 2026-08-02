# rustypaper

Structure-aware conversion of born-digital scientific PDFs to Markdown, Typst, JSON and plain
text. Headings, reading order, figures, tables, equations and references, on a CPU, with no
models and no native libraries.

The good open-source converters (Marker, MinerU, Docling, Nougat) are Python stacks that want a
GPU; GROBID is CPU-only and fast but is a JVM service that emits TEI and ignores maths. The Rust
crates that exist are generic text extractors with no notion of a paper. This aims at the gap:
**structure-aware, maths-aware, CPU-only, single binary.**

> Status: **feature complete**. Converts one- and two-column papers to Markdown, Typst, plain
> text or JSON, with reading order, figures, tables, mathematics and references.

## Install

```sh
cargo install rustypaper          # the command-line tool
```

```toml
rustypaper = "0.1"            # the library
```

```sh
pip install rustypaper        # the Python bindings
```

**Nothing else to install.** The default build reads PDFs with
[rustium-pdf](https://github.com/pgarrett-scripps/rustium-pdf), a pure-Rust interpreter, so
there is no native library to fetch, point an environment variable at, or match versions with.
`ldd` on the binary shows libc, libm and libgcc and nothing else; the wheel is the extension
module and the Python package around it, with no C library travelling beside it.

pdfium — the Chromium PDF engine, behind FFI — remains available as an opt-in feature. It is
the reference the pure-Rust backend is measured against, and an escape hatch for documents
rustium cannot yet read. Asking for it means supplying it:

```sh
cargo build --release --no-default-features --features pdfium
scripts/fetch-pdfium.sh     # or set PDFIUM_DYNAMIC_LIB_PATH yourself
```

## Getting started

```sh
scripts/fetch-corpus.sh     # evaluation corpus of arXiv papers, not committed
scripts/build.sh            # cargo build --release, plus installing the Python extension
```

```sh
# Convert.
./target/release/rustypaper convert corpus/resnet.pdf
./target/release/rustypaper convert corpus/resnet.pdf --format typst --assets figures/
./target/release/rustypaper convert corpus/*.pdf --out out/        # batch
./target/release/rustypaper convert paper.pdf --caveman=hard       # -24% words for LLM ingestion

# Diagnostics.
./target/release/rustypaper probe corpus/resnet.pdf --pages   # counts, fonts, detected gutters
./target/release/rustypaper text  corpus/resnet.pdf --geometry # reconstructed lines
./target/release/rustypaper dump  corpus/resnet.pdf --page 0 --pretty
```

`probe` prints per-page counts, the font histogram and the detected gutters. Gutters are the
first thing to check when a two-column paper comes out interleaved; the font histogram is the
first thing to check when text comes out wrong.

## Design

Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the pipeline, the coordinate convention,
the two backends behind the `PageSource` trait, and the things the corpus taught this converter
the hard way.

The short version: the backend produces a `PageRaw` of glyphs, paths and images, and every later
stage is a pass over an IR. The `Document` JSON is the contract; Markdown, Typst and plain text
are renderings of it.

| | |
|---|---|
| speed | 2.5–8.7 ms/page, single process |
| corpus | 10 papers: ML, pure maths, physics, biology, statistics |
| memory | 13–30 MB peak for a whole paper |
| footprint | a 3.2 MB binary, no native library, no models |

Unicode repair turned out not to be needed — glyph-name fallback already resolves TeX ligatures,
so the tables the plan budgeted for were dropped. De-hyphenation could not work the way the plan
assumed either: pdfium removes soft line-break hyphens before the text reaches the pipeline, so
on that backend there is no hyphen to key off. Words split across a break are rejoined using the
document's own vocabulary instead, which needs no word list and knows the paper's jargon. The
default backend does leave the hyphen in, and it is preferred when it is there.

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
import rustypaper
print(rustypaper.to_markdown('corpus/resnet.pdf')[:80])
doc = rustypaper.to_document('corpus/resnet.pdf')   # the document model as a dict
"
```

`ScannedDocument` is raised for image-only PDFs, so callers can route those to an OCR pipeline
instead. Conversion releases the GIL, and on the default backend several threads convert in
parallel.

## Evaluation

Quality is measured, not eyeballed. Papers submitted to arXiv as TeX source come with the prose
their PDF was rendered from, which is free ground truth for exactly this document class — for
the subset of papers that have it. PDF-only submissions have none, and are reported as skipped
rather than scored.

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval
```

Current scores on the default backend, across the nine scorable papers:

| metric | value | what it says |
|---|---|---|
| prose bigram recall | **0.891** | prose comes out right, in the right order |
| equation recall | **0.370** | most display equations are *not* found |
| equation fidelity | **0.547** | those that are found are roughly half right |
| tables | 26 found / 44 in source | badly distributed: 8 of ResNet's 17, none of statistics' 2, two invented on papers with none |

The maths numbers are the honest state of the differentiator, and they are the project's
weakest point — see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md). See
[`eval/README.md`](eval/README.md) for what the metrics mean and why plain edit distance is the
wrong primary measure here.

## Testing

```sh
cargo test                                    # unit, robustness and corpus tests; corpus tests
                                              # skip if corpus/ is empty
python3 -m unittest discover -s eval/tests    # the eval harness's own tests
PYTHONPATH=python pytest python/tests -q      # the Python surface, against the corpus
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Integration tests live in `rustypaper/tests/corpus.rs` and run against real papers. They skip
rather than fail when the corpus is absent, so a fresh clone is green.

## Licence

MIT OR Apache-2.0.

That covers everything a default build contains, the published crate and the published wheels
included: the dependency tree is Rust, and rustium-pdf is MIT OR Apache-2.0 as well.

The `pdfium` feature is the exception, and only for whoever turns it on. pdfium is BSD-3-Clause
and vendors further third-party code (FreeType, libjpeg-turbo, zlib, ICU and others) under
their own terms. After running `scripts/fetch-pdfium.sh` those notices are in
`vendor/pdfium/licenses/`, and `vendor/pdfium/LICENSE-packaging` is the MIT licence of
[pdfium-binaries](https://github.com/bblanchon/pdfium-binaries), which builds and packages it.

**If you redistribute a binary that bundles `libpdfium.so`, ship `vendor/pdfium/licenses/` with
it.** Nothing released from this repository does.
