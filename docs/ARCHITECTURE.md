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

## The reader boundary

Reading a PDF happens behind one trait, `backend::PageSource`, implemented over
[rustium-pdf](https://github.com/pgarrett-scripps/rustium-pdf), a pure-Rust interpreter. There is
no C library in the build at all: `ldd` on the CLI reports libc, libm and libgcc.

The trait is not there to hold a second implementation. It is there because what the pipeline is
promised has to be stated somewhere, and that statement belongs above the reader rather than
inside it. So the classification downstream passes depend on — `classify_path`, `clip_to_page`,
`resolve_size`, `expand_ligature` — lives in `backend/mod.rs`: table detection must not become a
function of how a particular reader represents a path or names a glyph. Rectangle detection is
the one part that has to sit below the boundary, since it inspects the reader's own path
segments, but the criterion it applies to promote a path to `PathKind::Box` is fixed here.

Three obligations the boundary carries, all settled at it rather than papered over inside the
reader:

- **Line-break hyphens.** The page is reported as written, so a soft hyphen at a break is usually
  present and `doc::rejoin_across_break` takes it as the better evidence it is. Where a document
  leaves none, the document's own vocabulary settles the join.
- **Word breaks.** Space marks are synthesised from advance widths, and `segment_words` treats
  them as authoritative on any line that has them. Mark completeness is therefore a real
  obligation on a `PageSource`: a line that is under-marked runs the rest of itself together.
- **Nested content.** LaTeX's `\includegraphics` lands in the content stream as a form XObject,
  so every rule and image inside a figure sits a level down. Forms are executed as the stream is
  walked, with the form matrix accumulating down the tree; a reader that yielded only top-level
  objects would report a figure as empty.

## Coordinates

Everything above `backend/` works in **PDF points, top-left origin, y-down**, with page rotation
already applied. PDF's native space is bottom-left/y-up, so the conversion happens once, at the
backend boundary, and nothing downstream converts again: `Page::page_matrix` gives exactly that
transform, applied to every point rather than case-analysed per rotation.
`glyphs_land_inside_the_page` in `tests/corpus.rs` is the regression test for getting it wrong.

## Findings from M0

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
per document across the corpus — but the reader *generates* 5 000 to 8 500 per document from the
font's advance widths. Measured against those, gap analysis mis-segments kerned pairs:
`learning framework` has a 0.18 em ink gap where a typical space on the same line is 0.30 em. Gap
analysis is now the fallback for documents that emit no whitespace at all, and inferring a
per-line threshold by Otsu may only *lower* it, never raise it — author lines have three gap
classes and splitting at the widest yields `KaimingHe`.

The gap heuristic that synthesises those marks had to drop to 0.12 em: justification compresses a
word space to about 0.19 em, and at 0.20 em whole lines were being left unmarked and running
together.

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

### Text on a pure rotation can report a font size of 0

Every arXiv preprint carries a sideways stamp down the left margin, and every one of its 70
glyphs arrived with a scaled size of 0. Size is load-bearing everywhere downstream — baseline
tolerance, word gaps, heading detection — so "positive and finite" is established once, in
`backend::resolve_size`, rather than defended in every consumer. The substitute is the ink
extent measured *across* the baseline; taking the larger dimension would report the advance and
make a margin stamp look like display type. Rotated glyphs are excluded from running text;
sideways table headers are excluded by the same rule and are not yet handled.

### De-hyphenation cannot key off the hyphen alone

A hyphen at a line break says the word continues, but not what it becomes: `learn-` + `ing` is
`learning`, while `state-` + `of-the-art` is a compound that keeps its hyphen. And a document
that leaves no hyphen at the break gives nothing to key off but the words themselves. Both cases
go through `doc::rejoin_across_break`, which asks the document's own vocabulary whether the
joined form appears elsewhere — no word list, and it knows the paper's jargon. A geometric guard
comes first either way: a line stopping short of its block's right edge was not broken to fit, so
its last word is whole.

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

| | |
|---|---|
| equation recall | **0.370** |
| equation fidelity | **0.547** |

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

Release build, one document per process, no figure rasterisation, best of three:

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

Converting all ten in one process, writing figures, takes 2.13 s and 64 MB.

**Ingest dominates wall time.** Extracting the whole corpus takes 0.70 s, and converting it paper
by paper without figures takes 0.70 s as well: everything after ingest runs across pages under
rayon and disappears into the same wall clock.

Two experiments that did not pay off, recorded so they are not repeated:

- Dropping `fill_color` from ingest saves 2%. Not worth losing the field.
- Reading only segment *types* rather than coordinates in the rectangle test saved nothing
  measurable and made classification worse — Adam went from 39 boxes to 148.

## Dependencies

The build is Rust throughout, and everything in it is MIT OR Apache-2.0.

- **rustium-pdf** (`rustium` in the manifest) reads the PDF, resolved from crates.io at `0.1`. It
  is deliberately not a path dependency: a path only resolves where a sibling checkout happens to
  exist, which is this machine and not a CI runner. To develop the two together, add a
  `[patch.crates-io]` override rather than committing one.
- **rayon** parallelises the stages above ingest across pages; **clap** and **anyhow** are used by
  the binary only, which cargo has no way to express for a `[[bin]]` inside a library crate.
