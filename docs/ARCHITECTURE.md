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

## Measurements

Extraction only (M0 scope), release build, corpus of four arXiv papers:

| paper | pages | glyphs | ms/page |
|---|---|---|---|
| adam | 15 | 40 851 | 4.3 |
| bert | 16 | 62 370 | 4.9 |
| resnet | 12 | 57 917 | 6.4 |
| transformer | 15 | 38 753 | 7.5 |

Budget is ≤100 ms/page end-to-end single-threaded, so ingest currently costs under 10% of it.

Known cost to revisit if ingest ever shows up in a profile: `PdfPageTextChar::font_name()`
allocates a `String` per character inside pdfium-render. The backend already avoids re-interning
by remembering the previous name, but the allocation itself needs the raw
`FPDFText_GetFontInfo` binding and a reusable buffer to remove.

## Dependencies

- **pdfium** (BSD-3-Clause), pinned to a specific build by `scripts/fetch-pdfium.sh`. Loaded
  dynamically; `vendor/pdfium/lib` is searched first, then `PDFIUM_DYNAMIC_LIB_PATH`, then the
  system. Pinning matters because the build we test against should be the one we run against.
- **pdfium-render** is compiled against its `pdfium_latest` bindings (chromium/7881) while the
  vendored binary is chromium/7961. pdfium's public C API is append-mostly so the newer library
  is a superset; if a binding ever goes missing, this is the first place to look.
- **image** is pinned to the version `pdfium-render`'s `image_latest` feature resolves to, so
  `PdfBitmap::as_image()` returns the same `DynamicImage` type we crop and encode.
