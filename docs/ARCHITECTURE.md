# Architecture

## Shape

Every stage is a pass over an intermediate representation:

```
PDF --[backend]--> PageRaw   glyphs, paths, images
    --[text]-----> lines and words
    --[layout]---> columns, reading order, typed blocks
    --[math/table/refs]--> Document
    --[emit]-----> Markdown / Typst / JSON / text
```

`Document` is the real output. Markdown is one rendering of it, which is why Typst can be added
later as an emitter rather than a rewrite.

## Backends

Reading a PDF is behind one trait, `backend::PageSource`, with two implementations selected by
cargo feature:

| feature | backend | |
|---|---|---|
| `rustium` (default) | [rustium-pdf](https://github.com/pgarrett-scripps/rustium-pdf), a pure-Rust interpreter | no FFI, no global state, `Send + Sync`, nothing to install |
| `pdfium` | Chromium's PDF engine, behind FFI via `pdfium-render` | needs `libpdfium.so` at runtime; the accuracy reference |

The default build therefore has no C library in it at all: `ldd` on the CLI reports libc, libm
and libgcc. pdfium is asked for deliberately —

```sh
cargo build --release --no-default-features --features pdfium
scripts/fetch-pdfium.sh
```

— and is kept as an escape hatch for documents rustium cannot yet read, and as the reference the
pure-Rust backend is measured against. `backend::Backend` resolves to rustium when both features
are on, so a build only gets pdfium by giving up the default.

Nothing above `backend/` can tell which one ran. The classification downstream passes depend on
— `classify_path`, `clip_to_page`, `resolve_size`, `expand_ligature` — lives in `backend/mod.rs`
rather than in either backend, because two backends that classified rules differently would make
table detection depend on which was compiled in. Only rectangle detection is unavoidably
per-backend, since each has its own representation of a path's segments; both promote a path to
`PathKind::Box` on the same criterion.

Two places where they genuinely differ, both reconciled above the trait rather than papered over
inside it:

- **Line-break hyphens.** pdfium deletes them from the text page; rustium reports the page as
  written. `doc::rejoin_across_break` handles both and prefers the hyphen when it is there — it
  is better evidence than the vocabulary heuristic pdfium forces on us.
- **Word breaks.** Both synthesise space marks on their own gap heuristic, and `segment_words`
  treats the marks as authoritative on any line that has them. Mark completeness is a real
  obligation on a `PageSource`: a backend that under-marks a line runs the rest of it together.

## Coordinates

Everything above `backend/` works in **PDF points, top-left origin, y-down**, with page rotation
already applied. PDF's native space is bottom-left/y-up, so each backend converts at its own
boundary and nothing downstream converts again — `backend::pdfium::Transform` for pdfium, the
matrix from `Page::page_matrix` for rustium. `glyphs_land_inside_the_page` in `tests/corpus.rs`
is the regression test for getting it wrong.

## Findings from M0

Three things were discovered by building it that are worth not rediscovering. pdfium was the
only backend at the time, so the first two are about pdfium specifically; they are kept because
that backend still ships as a feature.

### pdfium-render's `thread_safe` feature does not make anything thread-safe

It is a default feature and its entire effect is:

```rust
#[cfg(feature = "thread_safe")]
unsafe impl<'a> Send for PdfDocument<'a> {}
#[cfg(feature = "thread_safe")]
unsafe impl<'a> Sync for PdfDocument<'a> {}
```

No locking. It only lets pdfium handles cross thread boundaries; keeping concurrent calls out of
pdfium is the caller's job. Running the integration tests on the default multi-threaded test
harness aborted with `free(): corrupted unsorted chunks` — and separate documents on separate
threads are enough to trigger it, because pdfium's global state is shared.

`PDFIUM_LOCK` in `backend/pdfium.rs` serialises every entry point, including `Drop` (closing a
document calls into pdfium, so the document is held in an `Option` and taken under the guard).
`concurrent_extraction_does_not_corrupt_pdfium` is the regression test.

The consequence for the pipeline is the one the plan assumed: **ingest is serialised, and the
pure-Rust stages are what get parallelised.** rustium has no global state and is `Send + Sync`,
so ingest could be parallelised on the default backend; the pipeline does not yet do so, because
the shape has to hold for both. Converting many documents at once should shard across processes,
not threads.

### Figures live inside Form XObjects

`page.objects()` only yields top-level objects. LaTeX's `\includegraphics` lands in the content
stream as a Form XObject, so treating forms as opaque loses every rule and image inside every
figure. Before the fix, `adam.pdf` reported **0** paths across 15 pages; after, 1 825.

Child objects report bounds in their form's coordinate space, so the form matrix accumulates
down the tree. `PdfMatrix::apply_to_points` uses the row-vector convention (`p · M`), so nesting
composes as `form.multiply(parent)`.

Text needs no equivalent handling: `FPDFText_LoadPage` already flattens the whole page.

### Path shape has to come from segments, not the bounding box

Classifying any painted path with area as a rectangle makes `PathKind::Box` swallow everything —
bezier artwork included — and leaves `PathKind::Other` permanently empty. Both signals matter
downstream: rectangles are cell shading and frames for table detection, while a dense cluster of
`Other` is how a vector figure gets recognised. `is_axis_aligned_rect` inspects the actual
segments. On the corpus this moved `transformer.pdf` from `2602 boxes / 0 other` to
`2255 / 347`, and `adam.pdf` from `676 / 0` to `39 / 637`.

## Findings from M1

### Word breaks come from whitespace glyphs, not from measuring gaps

The plan had this backwards. LaTeX output contains almost no real space characters — 9 to 236
per document across the corpus — but pdfium *generates* 5 000 to 8 500 per document from the
font's advance widths, which the public API does not otherwise expose. Measured against those,
gap analysis mis-segments kerned pairs: `learning framework` has a 0.18 em ink gap where a
typical space on the same line is 0.30 em. Gap analysis is now the fallback for documents that
emit no whitespace at all, and inferring a per-line threshold by Otsu may only *lower* it, never
raise it — author lines have three gap classes and splitting at the widest yields `KaimingHe`.

rustium synthesises the same marks on its own gap heuristic, so this holds on either backend. Its
threshold had to drop to 0.12 em to match: justification compresses a word space to about 0.19 em,
and at 0.20 em whole lines were being left unmarked and running together.

### The column profile must be built from glyphs, not lines

Both columns of a paper usually sit on the same baseline grid, so before the gutter is known a
"line" is often one row of *both* columns — and therefore spans the gutter it is meant to
reveal. Profiling by line found the gutter on 2 of 12 pages of a plainly two-column paper;
profiling by glyph finds it on 11. Two guards keep it honest: a band must have dense text within
24pt on both sides, and a page reporting more than two gutters is treated as having none, since
four or more columns means a wide table rather than columns.

### XY-cut must cut at the widest gap, not at every gap

Ordinary leading between two body lines is a gap. Cutting at all of them shreds the page into
one band per line before the column structure can be seen, and the two columns then interleave
line by line. Cutting only at the widest gap — and any within 15% of it, so uniformly-leaded
lines stay together — lets the dominant structure win at each level and the rest be found by
recursion. Full-width elements block vertical cuts by construction, which is why a title above
two columns, and a wide figure dropped between them, both come out in the right order without a
special case.

### pdfium reports a font size of 0 for rotated text

Every arXiv preprint carries a sideways stamp down the left margin, and pdfium gives all 70 of
its glyphs a scaled size of 0. Size is load-bearing everywhere downstream, so the backend now
substitutes the ink extent measured *across* the baseline — taking the larger dimension would
report the advance and make a margin stamp look like display type. Rotated glyphs are excluded
from running text; sideways table headers are excluded by the same rule and are not yet handled.

The substitution is `backend::resolve_size`, shared rather than pdfium's, because "size is
positive and finite" is an invariant downstream depends on and every backend has to meet it.

### pdfium strips soft line-break hyphens

There is no hyphen left in the glyph stream at a hyphenated line break, because Chrome removes
them so that copy-paste rejoins words. The artifact is therefore `learn ing`, not `learn- ing`.
M2's de-hyphenation cannot key off a trailing hyphen and has to notice a line ending mid-word,
then consult the document's own vocabulary.

rustium reports the page as written, so on the default backend the hyphen *is* there.
`doc::rejoin_across_break` takes it when it is and falls back to the vocabulary when it is not;
neither backend is normalised to look like the other, because the hyphen is the better evidence
and discarding it would be losing information on purpose.

## Findings from the later milestones

### A clipped path reports the bounds of its unclipped geometry

One path in BERT ran from y=-38870 to y=39239 on an 842pt page. Figure regions merge nearby
graphics, so that single object dragged a region to 6012% of the page, and suppressing text
inside figures then removed every block on it. Object bounds are clipped to the page at the
backend boundary, and a region covering more than 85% of a page is rejected outright.

### Word breaks, column profiles and cut selection all needed the *other* primitive

Three separate passes were first written against the intuitive input and had to be moved:

| pass | wrong input | right input |
|---|---|---|
| word segmentation | measured gaps | the backend's generated space glyphs |
| column profile | line extents | glyph extents |
| XY-cut | every whitespace gap | only the widest |

The common thread is that the intuitive primitive is downstream of the thing being decided. A
line cannot reveal a gutter it spans; a gap cannot reveal a word break the font already knows.

### Confidence has to cost something

Maths reconstruction reported confidence 1.0 for every formula until a guessed radical extent
and an unnameable glyph were made to lower it. A score that is always 1.0 is worse than no score,
because the image fallback it gates never fires.

## Findings from widening the corpus

The corpus began as four machine-learning preprints in two conference templates. Adding pure
maths, physics, biology and statistics papers, a Springer LNCS submission and a 2014 NeurIPS one
found four bugs in a converter that passed every test it had:

- **Any short run of capitals parsed as a section label**, so `ON DIOPHANTINE SETS...` was
  section `ON`, and any title starting `A ...` was appendix `A`. A lettered label is a single
  capital and must carry its full stop.
- **Titles are not always larger than the body.** `amsart` sets them in capitals at body size, so
  a pure-maths paper had no title at all.
- **Bibliography numbering runs across column blocks.** ResNet's first column ends at `[27]` and
  its second begins at `[28]`; restarting the sequence per block lost half its entries.
- **A derivation's row spans three baselines at three sizes**, so line building saw three lines
  and assembly split on every size change. A quarter of one paper's blocks held three words or
  fewer.

The pattern is worth stating plainly: **a converter tested on one family of templates learns
that family, not the general shape of a paper.** Every one of these was invisible on the
original four, and the new papers score *higher* once fixed — the two-column ML templates were
the harder cases all along.

## What the maths scorer found

Scoring emitted LaTeX against the equations in each paper's own source — the measurement the
original plan called for and did not get built until much later — gives the honest state of the
project's headline claim:

| | default backend | pdfium |
|---|---|---|
| equation recall | **0.370** | 0.384 |
| equation fidelity | **0.547** | 0.557 |

Recall ranges from 1.000 on ResNet, which has two equations, to 0.000 on BERT and unet and 0.035
on a biology paper with 57. **Detection, not reconstruction, is the weaker half**: most display
equations are never identified as such, and the ones that are come out around half right.

The confidence score is still the weaker signal of the two it reports. Across the corpus the
converter emits 297 display equations at a mean confidence of 0.83, of which 40 fall below the
0.55 fallback threshold — so the image fallback does now fire, where at first it never did.
Confidence still reflects two specific snags, a guessed radical extent and an unnameable glyph,
and none of the ways detection goes wrong: an equation that was never recognised as one has no
confidence to be low. **A score that is always high is worse than no score**, because it
silently disables the safety mechanism built on it.

The fix has to start with detection: `math::display` requires a line to be centred in its column
or to carry an equation number, and templates that indent display maths without centring it are
invisible to it.

## Measurements

Release build on the default backend, one document per process, no figure rasterisation, best of
three:

| paper | pages | ms/page |
|---|---|---|
| unet | 8 | 2.5 |
| numbertheory | 19 | 3.2 |
| gan | 9 | 3.3 |
| statistics | 14 | 3.6 |
| bert | 16 | 3.8 |
| adam | 15 | 4.0 |
| resnet | 12 | 4.2 |
| optics | 12 | 5.8 |
| biology | 23 | 7.4 |
| transformer | 15 | 8.7 |

Peak memory is 13–30 MB per document. The budget was ≤100 ms/page end-to-end, so this runs at
3–9% of it.

Converting all ten in one process, writing figures, takes 2.13 s and 64 MB on rustium against
1.98 s and 93 MB on pdfium. The pure-Rust backend costs a few per cent of wall time and saves a
third of the memory.

**Ingest dominates wall time on either backend.** Extracting the whole corpus takes 0.70 s, and
converting it paper by paper without figures takes 0.70 s as well: everything after ingest runs
across pages under rayon and disappears into the same wall clock.

Two rounds of optimisation on the pdfium backend roughly halved its total time: running the
pure-Rust stages across pages under rayon, and caching the font descriptor flags, which are
per-font but were being read per-character and accounted for ten of the eighteen FFI calls a
glyph cost.

Two experiments that did not pay off, recorded so they are not repeated:

- Dropping `fill_color` from ingest saves 2%. Not worth losing the field.
- Reading only segment *types* rather than coordinates in the rectangle test saved nothing
  measurable and made classification worse — Adam went from 39 boxes to 148.

The largest remaining wrapper cost on pdfium is `PdfPageTextChar::font_name()` at ~9% of ingest,
which allocates a `String` per character. Removing it needs the raw `FPDFText_GetFontInfo`
binding and a reusable buffer.

## Dependencies

The default build is Rust throughout, and everything in it is MIT OR Apache-2.0.

- **rustium-pdf** (`rustium` in the manifest) is the default backend, resolved from crates.io at
  `0.1`. It is deliberately not a path dependency: a path only resolves where a sibling checkout
  happens to exist, which is this machine and not a CI runner. To develop the two together, add
  a `[patch.crates-io]` override rather than committing one.
- **rayon** parallelises the pure-Rust stages across pages; **clap** and **anyhow** are used by
  the binary only, which cargo has no way to express for a `[[bin]]` inside a library crate.

The `pdfium` feature pulls in the rest, and only for whoever turns it on:

- **pdfium** (BSD-3-Clause), pinned to a specific build by `scripts/fetch-pdfium.sh`. Loaded
  dynamically, not linked: `PDFIUM_DYNAMIC_LIB_PATH` is tried first, then `vendor/pdfium/lib`
  relative to the executable, then relative to the working directory, then the system library.
  Pinning matters because the build we test against should be the one we run against. pdfium
  vendors further third-party code under its own terms — see the licence note in the README
  before redistributing a binary that bundles it.
- **pdfium-render** is compiled against its `pdfium_latest` bindings (chromium/7881) while the
  vendored binary is chromium/7961. pdfium's public C API is append-mostly so the newer library
  is a superset; if a binding ever goes missing, this is the first place to look.
- **image** is used only by that backend, to crop and PNG-encode what pdfium rasterises — rustium
  encodes its own. It is pinned to the version `pdfium-render`'s `image_latest` feature resolves
  to, so `PdfBitmap::as_image()` returns the same `DynamicImage` type. It is declared
  unconditionally in the manifest rather than behind the feature, so a default build compiles it
  without using it.
