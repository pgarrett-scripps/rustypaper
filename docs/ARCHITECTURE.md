# Architecture

## Shape

Every stage is a pass over an intermediate representation:

```
PDF --[backend]--> PageRaw   glyphs, paths, images
    --[text]-----> lines and words, Unicode repaired
    --[layout]---> columns, reading order, typed blocks
    --[math/table/refs]--> Document
    --[emit]-----> Markdown / Typst / JSON / text
```

`Document` is the real output. Markdown is one rendering of it, which is why Typst can be added
later as an emitter rather than a rewrite.

## Coordinates

Everything above `backend/` works in **PDF points, top-left origin, y-down**, with page rotation
already applied. PDF's native space is bottom-left/y-up. The conversion happens exactly once, in
`backend::pdfium::Transform`, and `glyphs_land_inside_the_page` in `tests/corpus.rs` is the
regression test for getting it wrong.

## Findings from M0

Three things were discovered by building it that are worth not rediscovering.

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
pure-Rust stages are what get parallelised.** Ingest measures 4-8 ms/page, so it is not the
bottleneck. Converting many documents at once should shard across processes, not threads.

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

### pdfium strips soft line-break hyphens

There is no hyphen left in the glyph stream at a hyphenated line break, because Chrome removes
them so that copy-paste rejoins words. The artifact is therefore `learn ing`, not `learn- ing`.
M2's de-hyphenation cannot key off a trailing hyphen and has to notice a line ending mid-word,
then consult the document's own vocabulary.

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
| word segmentation | measured gaps | pdfium's generated space glyphs |
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

## Measurements

Release build, whole corpus, best of three runs of ten:

| paper | pages | ms/page |
|---|---|---|
| adam | 15 | 3.8 |
| bert | 16 | 4.1 |
| resnet | 12 | 5.6 |
| transformer | 15 | 7.0 |

Peak memory is 17–31 MB per document. The budget was ≤100 ms/page end-to-end, so this runs at
4–7% of it.

**Ingest is 80–90% of the remaining time**, and most of that is pdfium's own work. Two rounds of
optimisation roughly halved total time: running the pure-Rust stages across pages under rayon,
and caching the font descriptor flags, which are per-font but were being read per-character and
accounted for ten of the eighteen FFI calls a glyph cost.

Two experiments that did not pay off, recorded so they are not repeated:

- Dropping `fill_color` from ingest saves 2%. Not worth losing the field.
- Reading only segment *types* rather than coordinates in the rectangle test saved nothing
  measurable and made classification worse — Adam went from 39 boxes to 148.

The largest remaining wrapper cost is `PdfPageTextChar::font_name()` at ~9% of ingest, which
allocates a `String` per character. Removing it needs the raw `FPDFText_GetFontInfo` binding and
a reusable buffer.

## Dependencies

- **pdfium** (BSD-3-Clause), pinned to a specific build by `scripts/fetch-pdfium.sh`. Loaded
  dynamically; `vendor/pdfium/lib` is searched first, then `PDFIUM_DYNAMIC_LIB_PATH`, then the
  system. Pinning matters because the build we test against should be the one we run against.
- **pdfium-render** is compiled against its `pdfium_latest` bindings (chromium/7881) while the
  vendored binary is chromium/7961. pdfium's public C API is append-mostly so the newer library
  is a superset; if a binding ever goes missing, this is the first place to look.
- **image** is pinned to the version `pdfium-render`'s `image_latest` feature resolves to, so
  `PdfBitmap::as_image()` returns the same `DynamicImage` type we crop and encode.
