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
paper             bigram  cover   eq  eq rec  eq fid   tables    sec
------------------------------------------------------------------------
bert.pdf           0.877  0.920    6   0.000   0.000      6/8   0.12
biology.pdf        0.917  0.962   57   0.053   0.553      2/4   0.35
gan.pdf            0.882  0.961   14   0.500   0.712      1/4   0.07
imagenet.pdf       0.828  0.911    7   0.429   0.607     9/26   0.31
medimaging.pdf     0.911  0.940   11   0.091   0.897     7/11   0.30
metasurface.pdf    0.922  0.987   23   0.217   0.624      1/0   0.08
numbertheory.pdf   0.878  0.987  103   0.728   0.824      0/0   0.12
optics.pdf         0.855  0.934  215   0.205   0.729      0/0   0.14
pinsage.pdf        0.664  0.839    2   0.000   0.000      2/6   0.15
resnet.pdf         0.909  0.968    2   1.000   0.618     9/17   0.11
sklearn.pdf        0.938  0.969    0   0.000   0.000      0/1   0.04
statistics.pdf     0.912  0.952   18   0.444   0.623      0/2   0.12
topological.pdf    0.881  0.902   21   0.619   0.789      3/2   0.31
transformer.pdf    0.852  0.921   19   0.421   0.851      4/7   0.29
unet.pdf           0.939  0.982    2   0.000   0.000      2/2   0.04
------------------------------------------------------------------------
mean               0.878               0.336   0.559

  skipped adam.pdf: arXiv source carries no LaTeX prose (converted to 5474 words, 226 blocks)
```

The two worst rows are the two most recently added, which is the corpus doing its job: `pinsage.pdf`
loses two of ten pages to columns that interleave where a display equation overhangs the gutter,
and `imagenet.pdf` loses its bibliography pages the same way.

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
  whether roughly the right number of tables was found. `metasurface.pdf` scores `1/0` and
  `biology.pdf` `8/4`, both of which are over-detection; `imagenet.pdf` scores `10/26`.

## Regression checking

`baseline.json` pins current scores. In CI, or before a commit that touches layout:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Exits non-zero, naming the paper and the metric, if any paper's bigram recall, equation recall
or equation fidelity drops by more than 0.005 — smaller movements are noise — or if the number
of tables it found drops at all, that being a count rather than a score. A paper the baseline
scored and this run does not, because it stopped converting or lost its ground truth, also
fails: the gate reads the baseline's list of papers, not only this run's. A metric an older
baseline does not record is skipped rather than failed. Refresh the baseline deliberately, with
`--json > baseline.json`, when a change is an intended improvement.

A `--only` run does not check for missing papers, since it deliberately converts a subset.

`baseline.json` is pinned against what a plain `scripts/build.sh` produces, which is what CI
runs.
