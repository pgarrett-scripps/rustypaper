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

### The section map is a view, not a field

`Document::sections` derives the outline — title, level, a half-open block range including nested
subsections, and a page span — from the heading blocks, and `Document`'s hand-written `Serialize`
puts it in the JSON beside `title` and `blocks`. It is computed on the way out rather than stored
because **assembly is not the last pass that touches `blocks`**: the pipeline lifts tables and
display equations back in, splits the bibliography into entries, attaches figures, and `compress`
rewrites text. A tree of block indices frozen at assembly time would be quietly wrong by the time
a caller read it, and a stale index is worse than an absent one — it points confidently at the
wrong paragraph. The cost is one linear walk per serialisation, against a conversion that reads a
PDF.

Content before the first heading is reported as one section with `title: null` at level 0 rather
than left out of the tree. It is a real part of the document — the abstract is in it — and the
question "which section is block 3 in?" deserves an answer; what would be dishonest is giving it
a heading the paper never printed.

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

## Findings from widening it again, to publisher templates

arXiv skews towards conference styles, and the corpus inherited the skew: ten papers, but only
one of them typeset by a publisher rather than by a call for papers. Six more were added —
IEEEtran with a drop capital (`metasurface.pdf`), acmart (`pinsage.pdf`), REVTeX
(`topological.pdf`), Elsevier's elsarticle (`medimaging.pdf`), a Springer journal in svjour3
(`imagenet.pdf`) and JMLR (`sklearn.pdf`) — and unlike the last widening, these are not four bugs
with four fixes. They are four *shapes* of failure, all of them pinned in `tests/corpus.rs`. The
two that were about columns are fixed and are written up in the next section; of the other two,
the drop capital is fixed and written up below, and the one that remains is:

- **A title set on several lines stays several blocks.** Publishers break long titles across two
  or three centred lines; `doc.title` takes one of them. The IEEE paper's title is its middle
  line, `Array of Rectangular`.

One thing did *not* break, which is worth recording because it was expected to: four of the six
titles come out exactly right.

### A drop capital is placed where it sits

`\IEEEPARstart` sets the first letter of the introduction two lines deep in the margin, so
reading order put it two lines in: `Conformal antennas...` came out as `onformal antennas are
essential components in appli-Ccations`. The letter was not lost, it was inserted into a word
forty characters later, which is worse than losing it — a search for the word fails either way,
but nothing looks wrong.

Size alone cannot find it, because a figure label, an equation number and a section ornament are
outsized too. What is specific to a drop capital is the *paragraph set around it*: one outsized
letter at the column's left edge, with the lines it rises through indented to just past its right
edge. `text::lines::reunite_drop_capitals` looks for that shape and moves the letter to the first
of those lines before word segmentation, so it joins the word rather than becoming one. Across
the sixteen papers it fires exactly twice, on the two IEEEtran ones.

### Roman numerals have to be read as numbering

REVTeX's and acmart's sections were found all along, but by their size and their capitals rather
than by their numbering — and IEEEtran sets sections at body size in the body face, where the
numeral is the only evidence there is. `I.` happened to work, being the appendix label
`doc::numbered_heading` already knew; `II.` onwards were nothing to it, and `II. THEORY` at the
foot of a column was classified as a *footnote*. Reading `I`, `V` and `X` as a numeral —
canonically written, and carrying its full stop — finds all four IEEE sections. `C`, `D`, `L` and
`M` are deliberately not read: they buy a range no paper needs, and would turn `MD.` and `DC.`
into section labels.

### A neighbouring column can be taken for part of a line

Two failures on `medimaging.pdf`, which sets an 8pt bibliography beside the 10pt end of an
appendix. Both are a question about the smaller thing answered with the larger one's number:

- **Baseline tolerance was read off the arriving glyph.** A 10pt line 2.5pt below an 8pt one is
  inside a 10pt glyph's share of the tolerance and outside an 8pt one's, so it joined the 8pt
  line's cluster. Judged at the *smaller* of the two sizes, the two stay apart.
- **Any smaller cluster overlapping a line's ink was a script of it.** Once a cluster spanned
  both columns, the next bibliography line overlapped it and was within the size ratio a script
  is allowed, so it was absorbed — and sorting the result by x zipped two columns together a
  character at a time: `AnaEvnig, YM.,e Kd oBgiaonl`. A script is a *short run near its host's
  baseline*, and either property alone is ordinary in real maths, so both are now required.

Neither is only about that paper. With them, `topological.pdf` finds the `I. INTRODUCTION` that
its two-column table of contents used to swallow, `imagenet.pdf` stops fusing `1 Introduction`
onto the end of an author line, and `pinsage.pdf` finds all 31 of its references rather than 22.

## What the gutter was being measured against

Both column failures the publisher templates found were the same mistake made twice: a local
question answered with a page-wide number.

**The flanking text was measured against the page's peak ink.** A gutter has to have text beside
it, or the sparse interior of a figure reads as a column boundary. Requiring both sides to reach
half of the densest bin *on the page* assumes the two columns are equally dense, and they are
not: one of them holds the algorithm float, the ragged appendix list, the bibliography set two
points smaller. Worse, the densest bin is usually the left margin, where every line of the page
starts. So a corridor 20pt wide with *no ink in it at all* was rejected on 2 of `pinsage.pdf`'s
10 pages and 4 of `imagenet.pdf`'s, and those pages then interleaved line by line. Measured
against the page's *median* text bin instead, they are found — and so is the page of
`imagenet.pdf` whose right-hand column is a sparse table.

The same rule then had to be told what a collapse is. A single column's right edge is ragged
rather than square: coverage decays over the last inch of the measure, and a page-wide threshold
cuts that slope somewhere, so `adam.pdf` reported an 11pt gutter inside its own right margin —
in a band holding four fifths of the ink of the text beside it. A gutter is a *collapse*, so the
band must be empty by comparison with the text immediately flanking it, not merely with the
page's average.

**The gutter's edges were where coverage came back, not where the text did.** A bibliography
hangs its `[12]` labels in a column of their own at the inner edge of the text, and they appear
on only the first line of each entry — a fifth as often as running text, which is below any
threshold that also lets a title cross. So the reported band swallowed the label column and its
far edge landed *inside* the entries. `split_at_gutters` wants a pair of glyphs straddling the
whole band before it will cut, and the lines it therefore could not cut were exactly the ones
that open an entry: `metasurface.pdf` yielded 0 references from 41, `imagenet.pdf` 0 from about
100, both arriving as one paragraph of two columns zipped together (`...1990.[30] I. Yoo and...`).
Trimming each band to its own floor — the corridor that is actually empty — puts the edges back.
Where the floor is not zero, because a title crosses the gutter, every bin is at it and the band
is returned untouched, which is what has to happen for a full-width line to keep crossing.

With both fixed, `metasurface.pdf` gives all 41 of its entries in order.

### One page is not always enough to read

A wide table holds the corridor open on every baseline it occupies, so the columns of running
text above or below it leave no dip deep enough to see: four pages of `medimaging.pdf`, one of
`resnet.pdf`, one of `imagenet.pdf`. Looking for gutters inside horizontal slices of the page
finds them — and finds plenty of others that are not gutters. Measured across the corpus, an
unconstrained band search invents a column boundary in a table or an author block on eight pages
of *single-column* papers, which is precisely where splitting lines does the most damage.

So the search is anchored. A document that sets two columns sets them in the same place on every
page, so the gutters agreed by the pages that could be read become the document's spine, and a
band may only report a gutter that lands on it. `document_bands` is the whole-document pass that
does this, and a page that reads its own columns is still one band covering the page.

### A score can be an artifact of its reference

`pinsage.pdf` was recorded above as the worst paper in the corpus at 0.664 bigram recall, and
that was read as the cost of its two interleaved pages. It was not. Its arXiv source wraps 3 754
of its 10 686 reference words — a third of the paper — in `\cut{...}`, a macro that discards
them, so the eval's reference contains prose the PDF never printed. Scored against the prose the
PDF *does* print, that paper reads **0.920**, at the top of the corpus rather than the bottom.

Fixing the interleave moved it from 0.911 to 0.920 on that honest reference, and from 0.664 to
0.670 on the harness's. Both are the same repair: zipping two columns line by line breaks the
bigram only at each line join, so it costs about one bigram per line and no more. The lesson is
for the scoreboard rather than the converter — **a low score is a question, not a diagnosis**, and
this one had been attributed to the wrong cause. `detex` should treat `\cut` as discarding.

## What the maths scorer found

Scoring emitted LaTeX against the equations in each paper's own source — the measurement the
original plan called for and did not get built until much later — gives the honest state of the
project's headline claim:

| | |
|---|---|
| equation recall | **0.336** |
| equation fidelity | **0.559** |

Recall ranges from 1.000 on ResNet, which has two equations, to 0.000 on BERT and unet and 0.053
on a biology paper with 57. **Detection, not reconstruction, is the weaker half**: most display
equations are never identified as such, and the ones that are come out around half right.

The confidence score is still the weaker signal of the two it reports. Across the corpus the
converter emits 357 display equations at a mean confidence of 0.85, of which 42 fall below the
0.55 fallback threshold — so the image fallback does now fire, where at first it never did.
Confidence still reflects two specific snags, a guessed radical extent and an unnameable glyph,
and none of the ways detection goes wrong: an equation that was never recognised as one has no
confidence to be low. **A score that is always high is worse than no score**, because it
silently disables the safety mechanism built on it.

The fix has to start with detection: `math::display` requires a line to be centred in its column
or to carry an equation number, and templates that indent display maths without centring it are
invisible to it.

## What the sections scorer found

Scoring the section map's titles against the `\section` and `\subsection` titles of each paper's
own source — the first measurement of an outline this project had ever taken — gives **206 of
264**, or 0.78, across the fifteen scorable papers. Unlike tables and references this is a match
rather than a count: finding the right *number* of headings and the wrong ones scores badly, which
is the point, since the map is what a consumer navigates by.

**Most of what is lost is the level below the one the corpus has been checking.** `bert.pdf`
scores 16/29 with every top-level section found and almost every subsection missing — `2.1
Unsupervised Feature-based Approaches`, `4.1 GLUE`, `SQuAD v1.1`, `SWAG`. `medimaging.pdf` at
17/33 is the same picture: `Introduction`, `Discussion` and `Overview of deep learning methods`
are there, while `brain`, `eye`, `chest` and `cardiac` are not. Those headings are set bold at
body size, which `is_heading` accepts only when *every* line of the group is bold and the block
is short — and existing corpus tests all assert top-level headings, so nothing was watching.

`optics.pdf` at 9/18 is a different failure and the one worth chasing: it loses its own
`Introduction` and `Conclusion`, emits seven headings for eighteen, and one of the seven is
`tinmcid oern tt feield` — two columns zipped together. A paper whose headings are being built
out of interleaved text is not a heading-detection problem.

Nothing here says the tree is built wrongly. Every paper's ranges are ordered, non-overlapping
and valid — `section_ranges_are_valid_across_the_corpus` checks all sixteen — so what the number
measures is heading detection, which is where the work is.

## Measurements

Release build, one document per process, no figure rasterisation, best of three:

| paper | pages | ms/page |
|---|---|---|
| unet | 8 | 2.5 |
| numbertheory | 19 | 3.2 |
| gan | 9 | 3.3 |
| statistics | 14 | 3.6 |
| sklearn | 6 | 3.7 |
| imagenet | 43 | 3.7 |
| bert | 16 | 3.8 |
| medimaging | 38 | 3.9 |
| adam | 15 | 4.0 |
| resnet | 12 | 4.2 |
| optics | 12 | 5.8 |
| topological | 23 | 6.6 |
| biology | 23 | 7.4 |
| metasurface | 6 | 7.3 |
| pinsage | 10 | 8.1 |
| transformer | 15 | 8.7 |

Peak memory is 13–30 MB per document. The budget was ≤100 ms/page end-to-end, so this runs at
3–9% of it. The longest papers in the corpus are among the *cheapest* per page: pages of survey
prose cost less than pages of derivation.

Converting all sixteen in one process, writing figures, takes 4.2 s and 86 MB.

**Ingest dominates wall time.** Extracting the whole corpus takes 0.71 s against 0.76 s to
convert it — 93% — because everything after ingest runs across pages under rayon and disappears
into the same wall clock. Reproduce with `cargo run --release -p rustypaper --example
ingest_share`. Ingest itself is serial and the reader is `Sync`, so that is where the remaining
headroom is.

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
