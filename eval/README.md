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
bert.pdf           0.874  0.920    6   0.000   0.000      6/8   0.12
biology.pdf        0.915  0.963   57   0.035   0.570      3/4   0.34
gan.pdf            0.882  0.961   14   0.500   0.712      1/4   0.07
numbertheory.pdf   0.878  0.987  103   0.728   0.824      1/0   0.12
optics.pdf         0.855  0.934  215   0.205   0.729      1/0   0.14
resnet.pdf         0.909  0.968    2   1.000   0.618     8/17   0.10
statistics.pdf     0.912  0.952   18   0.444   0.623      0/2   0.11
transformer.pdf    0.852  0.921   19   0.421   0.851      4/7   0.28
unet.pdf           0.939  0.982    2   0.000   0.000      2/2   0.04
------------------------------------------------------------------------
mean               0.891               0.370   0.547

  skipped adam.pdf: arXiv source carries no LaTeX prose (converted to 5388 words, 230 blocks)
```

## Where the ground truth comes from

Where a paper was written in TeX and submitted as source, arXiv serves that source, so the prose
the PDF was rendered *from* is available without hand annotation.

**This is not every paper.** arXiv requires source only when a submission was prepared in TeX; a
PDF produced by Word, or by a tool the author did not submit source for, is accepted as a PDF.
Some authors also route a finished PDF through the TeX path by wrapping it in a one-line
`\includepdf` document, which yields an archive with no prose in it. `adam.pdf` — one of the ten
papers in this corpus, and the only one without usable source — is exactly that case.

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
  whether roughly the right number of tables was found. Two papers score `1/0`, which is
  over-detection, and one scores `8/17`.

## Regression checking

`baseline.json` pins current scores. In CI, or before a commit that touches layout:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Exits non-zero if any paper's bigram recall drops by more than 0.005; smaller movements are
noise. Only bigram recall is checked. Refresh the baseline deliberately, with
`--json > baseline.json`, when a change is an intended improvement.

`baseline.json` is pinned against what a plain `scripts/build.sh` produces, which is what CI
runs.
