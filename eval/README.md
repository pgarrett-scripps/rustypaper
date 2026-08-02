# Evaluation harness

Measures conversion quality against ground truth, so that quality changes are observed rather
than eyeballed.

```sh
cargo build --release                        # the converter, and optionally the bindings
python3 -m unittest discover -s eval/tests   # the harness's own tests
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval
```

```
converter=extension  scorer=difflib
paper               bigram  cover    sim         words  blocks    sec
----------------------------------------------------------------------
bert.pdf             0.851  0.924  0.672    10837/7305     333   0.17
resnet.pdf           0.895  0.970  0.644    10496/6021     300   0.17
transformer.pdf      0.839  0.923  0.709     6185/4369     186   0.22
----------------------------------------------------------------------
mean bigram recall   0.862
```

## Where the ground truth comes from

Where a paper was written in TeX and submitted as source, arXiv serves that source, so the prose
the PDF was rendered *from* is available without hand annotation.

**This is not every paper.** arXiv requires source only when a submission was prepared in TeX; a
PDF produced by Word, or by a tool the author did not submit source for, is accepted as a PDF.
Some authors also route a finished PDF through the TeX path by wrapping it in a one-line
`\includepdf` document, which yields an archive with no prose in it. `adam.pdf` — one of the
four papers in this corpus — is exactly that case.

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
  for why it sits well below the other two.

## Regression checking

`baseline.json` pins current scores. In CI, or before a commit that touches layout:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Exits non-zero if any paper drops by more than 0.5 points. Refresh it deliberately, with
`--json > baseline.json`, when a change is an intended improvement.

## Papers without ground truth

Some arXiv submissions are a PDF with a one-line LaTeX wrapper around `\includepdf` —
`adam.pdf` is one. There is no prose to score against, so it is reported as skipped rather than
scored near zero. It still exercises the converter.
