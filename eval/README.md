# Evaluation harness

Measures conversion quality against ground truth, so that quality changes are observed rather
than eyeballed.

```sh
scripts/build.sh                             # the converter and the Python extension it loads
python3 -m unittest discover -s eval/tests   # the harness's own tests
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval
```

Use `scripts/build.sh`, not a bare `cargo build`: the harness imports the compiled extension, and
a bare build leaves that stale.

```
converter=extension  scorer=difflib
paper             bigram  cover   eq  eq rec  eq fid   tables     refs  sections    sec
-------------------------------------------------------------------------------------------
bert.pdf           0.878  0.922    6   0.000   0.000      8/8    47/56     16/29   0.06
biology.pdf        0.916  0.962   57   0.632   0.691      4/4     33/?     27/30   0.18
gan.pdf            0.883  0.961   14   0.643   0.694      1/4    31/31       9/9   0.04
imagenet.pdf       0.856  0.930    7   0.429   0.641     9/26   97/102     27/34   0.18
medimaging.pdf     0.924  0.941   11   0.909   0.776    11/11  341/350     17/33   0.16
metasurface.pdf    0.927  0.986   23   0.261   0.657      1/0    41/41       5/8   0.04
numbertheory.pdf   0.878  0.987  103   0.845   0.865      0/0    21/21       8/9   0.07
optics.pdf         0.857  0.932  215   0.353   0.716      0/0    27/27      9/18   0.08
pinsage.pdf        0.933  0.987    2   0.500   0.671      4/6    31/31     15/17   0.08
resnet.pdf         0.918  0.969    2   1.000   0.659    15/17    50/50     13/14   0.06
sklearn.pdf        0.939  0.969    0   0.000   0.000      0/1     8/16       6/6   0.02
statistics.pdf     0.912  0.952   18   0.500   0.662      0/2    27/27     10/10   0.06
topological.pdf    0.882  0.903   21   0.809   0.847      3/2  163/368     15/18   0.16
transformer.pdf    0.852  0.921   19   0.526   0.868      4/7    40/40     22/22   0.14
unet.pdf           0.939  0.982    2   0.500   0.733      2/2    14/14       7/7   0.02
-------------------------------------------------------------------------------------------
mean               0.900               0.565   0.677
total                                                   62/90 971/1174   206/264

  skipped adam.pdf: arXiv source carries no LaTeX prose (converted to 5474 words, 226 blocks)
```

`sections` is a found/wanted count like `tables` and `refs`; `sec` is seconds, as it always was.
Every paper is converted **once** — `rustypaper.convert` returns the Markdown and the document
model from a single pipeline run, where scoring both used to mean reading each PDF twice.

The worst rows are publisher templates the corpus added late, which is the corpus doing its job:
`imagenet.pdf` still loses most of its tables, and `topological.pdf`'s bibliography is capped at
163 of 368 because the remainder never reaches the reference parser — a column-extraction gap,
not a parsing one.

## Where the ground truth comes from

Where a paper was written in TeX and submitted as source, arXiv serves that source, so the prose
the PDF was rendered *from* is available without hand annotation.

**This is not every paper.** arXiv requires source only when a submission was prepared in TeX; a
PDF produced by Word, or by a tool the author did not submit source for, is accepted as a PDF.
Some authors also route a finished PDF through the TeX path by wrapping it in a one-line
`\includepdf` document, which yields an archive with no prose in it. `adam.pdf` — one of the
sixteen papers in this corpus, and the only one without usable source — is exactly that case.

So the harness treats ground truth as *available for some papers*, not all: anything with fewer
than `MIN_REFERENCE_WORDS` of reference prose is reported as skipped rather than scored. Growing
the corpus means sampling papers and keeping the ones that have usable source, not assuming
every id will.

`detex.py` reduces that source to prose. It is deliberately not a LaTeX implementation — no
macro expansion, no package semantics. **Scores are therefore relative.** Whatever the reducer
gets wrong it gets wrong identically for every version of the converter, so regressions still
show; an absolute number means little.

## The metrics

- **bigram recall** (primary) — fraction of the reference's word bigrams that survive in the
  output. Plain edit distance is unfair here: the reducer drops tables, figures and the
  bibliography, so a *correct* conversion has far more words than the reference and gets
  punished for it. Recall over the reference's own bigrams asks only whether the prose that is
  there came out right and in the right local order. A scrambled reading order breaks a bigram
  at every column boundary; extra content costs nothing.
- **coverage** — fraction of distinct reference words present, order-insensitive. Read it
  against bigram recall: high coverage with low bigram recall means the words were all found
  but put in the wrong order, which is a reading-order bug rather than an extraction bug.
- **similarity** — whole-document sequence similarity. Kept for continuity; see the caveat above
  for why it sits well below the other two. Reported in `--json`, not in the printed table.
- **equation recall and fidelity** — the display equations in the paper's own TeX source, matched
  against the LaTeX the converter emitted. Recall is how many were found at all; fidelity is how
  close the found ones are, after both sides are normalised. `eq` in the table is how many the
  source has, and the means are taken over the papers that have any.
- **tables** — table blocks emitted against `\begin{tabular}` environments in the source. A count,
  not a match: it says nothing about whether the right cells ended up in the right places, only
  whether roughly the right number of tables was found. `metasurface.pdf` scores `1/0`, which
  is over-detection; `imagenet.pdf` scores `9/26`.
- **refs** — reference blocks emitted against the `\bibitem` entries the source declares. A count,
  like tables, and for the same reason: it says how much of the bibliography was recognised as a
  bibliography, not whether each entry's fields were parsed correctly. Downstream consumers audit
  citations, so a bibliography that never becomes typed blocks is a whole feature missing rather
  than a cosmetic loss — `bert.pdf` at `15/56` is one, and the two-column bibliography pages are
  where it goes wrong.

  Wanted is counted from **one** file of the source tree, never the sum of all of them: entries
  are counted per file and the largest list wins. arXiv archives carry the same `.bbl` twice
  under two names often enough that summing would double every count, and a stub
  `thebibliography` left in the main `.tex` beside a generated `.bbl` would inflate others.
  Both the plain `\bibitem{key}` and natbib's `\bibitem[label]{key}` count; commented-out
  entries do not.

  A source that declares no entries anywhere — a paper that cited with BibTeX and shipped only
  its `.bib`, which is `biology.pdf` here — prints `?` for wanted rather than `0`, and stores
  `null` in the JSON. Nothing was measured, and a zero would read as "this paper cites nothing".
- **sections** — the titles in the converter's section map, matched against the `\section` and
  `\subsection` titles the source declares. Unlike tables and refs this *is* a match rather than
  a count: a converter that finds the right number of headings and the wrong ones scores badly,
  which is the point, since the map is what a consumer navigates by.

  Both sides are reduced before comparing, because the differences between them are not the
  converter's doing: case, LaTeX markup, punctuation, and the section number the PDF prints and
  the source does not (`\section{Model Architecture}` against `3 Model Architecture`). A
  lettered number has to carry its full stop to be stripped, or `A Survey of Methods` loses its
  first word. A title then counts as found when some heading equals it, contains it, or is
  contained by it — containment in either direction because a heading may fuse a running head
  onto the title, or carry only one line of a title that broke across two — with a floor of
  `MIN_SECTION_TITLE` characters on the shorter side, without which a one-word heading matches
  half an outline.

  Wanted titles are deduplicated, for the same reason the bibliography is taken from one file:
  an archive that ships the paper twice does not have twice the sections. `\subsubsection` is
  not counted; the map goes deeper, but the deepest level is where heading detection is least
  reliable and the number would say more about the corpus than about the converter.

  At **206/264** this is the first honest measurement of section quality. `bert.pdf` (16/29) and
  `medimaging.pdf` (17/33) have every top-level section and lose the level below it, which is
  set bold at body size; `optics.pdf` (9/18) loses its own `Introduction`, and one of the seven
  headings it does emit is two columns zipped together. See the sections scorer's findings in
  `docs/ARCHITECTURE.md`.

## Regression checking

`baseline.json` pins current scores. In CI, or before a commit that touches layout:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Exits non-zero, naming the paper and the metric, if any paper's bigram recall, equation recall
or equation fidelity drops by more than 0.005 — smaller movements are noise — or if the number
of tables, references or sections it found drops at all, those being counts rather than scores:
one entry fewer is one entry lost. A paper the baseline scored and this run does not, because it
stopped converting or lost its ground truth, also fails: the gate reads the baseline's list of
papers, not only this run's. A metric an older baseline does not record is skipped rather than
failed — so a baseline from before the references or sections columns keeps passing. Refresh the baseline deliberately, with
`--json > baseline.json`, when a change is an intended improvement.

A `--only` run does not check for missing papers, since it deliberately converts a subset.

`baseline.json` is pinned against what a plain `scripts/build.sh` produces, which is what CI
runs.
