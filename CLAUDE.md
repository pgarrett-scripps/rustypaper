# Working on this repository

Notes for AI coding agents. Human-facing docs are `README.md` (what it does), `docs/ARCHITECTURE.md`
(how, and what was learned the hard way), `eval/README.md` (what the quality numbers mean) and
`RELEASING.md` (cutting a version).

## Build and test

```sh
scripts/fetch-corpus.sh    # 16 arXiv PDFs, not committed; corpus tests skip without it
scripts/build.sh           # cargo build --release AND install the Python extension
cargo test --release
python3 -m unittest discover -s eval/tests
PYTHONPATH=python pytest python/tests -q   # the Python surface; needs the corpus
```

**Use `scripts/build.sh`, not bare `cargo build`.** The eval harness loads a compiled extension
module; a bare cargo build leaves it stale and the harness then reports "no change" for a change
that worked. It warns when this happens, but the warning is easy to miss.

## Measure, do not assume

Quality is a number here, and every change to a pass should be justified by it:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rustypaper_eval --baseline baseline.json
```

Exits non-zero if any paper regresses by more than 0.005 in bigram recall, equation recall or
equation fidelity, if it finds fewer tables, or if a paper the baseline scored is missing or no
longer scorable. Refresh `baseline.json` deliberately, with `--json > baseline.json`, only when a
change is an intended improvement.

Current, over the fifteen scorable papers:

| prose bigram | equation recall | equation fidelity | tables | corpus tests |
| --- | --- | --- | --- | --- |
| 0.880 | 0.336 | 0.559 | 46/90 | 44/44 |

Maths and tables are the project's weak point, and the honest place to work next. The corpus
converts in 1.4 s and 66 MB of resident memory.

`pinsage.pdf` scores 0.670 and is *not* the worst-converted paper in the corpus: a third of its
arXiv source sits inside `\cut{...}`, which discards it, so the reference holds prose the PDF
never printed. Against what the PDF prints it scores 0.920. Fix `detex` before reading that row.

## Rules that have earned their place

- **Edit code with the Edit tool, never a scripted string replace.** `cargo fmt` reflows code,
  and an exact-match replace then fails *silently*. This cost three separate debugging sessions.
- **Never commit with failing tests.** It happened twice; both had to be unpicked.
- **The corpus is the specification.** Sixteen papers across ten template families, six of them
  publisher journal classes (IEEEtran, acmart, REVTeX, elsarticle, svjour3, JMLR). A converter
  tuned on one family passes its own tests and fails on everything else — see the two findings
  sections of `docs/ARCHITECTURE.md` for the bugs that only appeared when the corpus widened, and
  for the four still open that the publisher templates found.
- **Prefer an absent field to a wrong one.** Reference parsing omits authors it cannot parse;
  maths falls back to a rendered crop rather than emitting confident-looking wrong LaTeX.
- **Check which extension Python actually imported**, with `rustypaper._rustypaper.__file__`. The
  module is built `abi3`, and CPython prefers `_rustypaper.abi3.so` over a plain `_rustypaper.so`
  when both exist. `build.sh` used to install the plain name, so an old abi3 build beside it was
  loaded instead — silently, for every eval run. A whole round of eval numbers was measured
  against the wrong binary before this surfaced. `build.sh` now writes the abi3 name and deletes
  the other; do not reintroduce a second name.

## Shape

`PageRaw` (glyphs, paths, images) → lines → layout → `Document` → emitters. Each stage is a pass
over an IR and only knows the stage before it. `Document` is the contract; Markdown, Typst and
text are renderings of it, which is why the model was never allowed to become "whatever Markdown
can express".

Reading a PDF is confined to one module behind the `PageSource` trait, and the classification
downstream passes depend on (`classify_path`, `clip_to_page`, `resolve_size`, `expand_ligature`)
lives in `backend/mod.rs` rather than in the reader — what the pipeline is promised does not move
if the reader underneath it does. rustium has no global state and is `Send + Sync`, so ingest
could be parallelised; the pipeline does not yet do so, because ingest is a small share of the
total.

Two things the trait boundary has to get right, both handled above it rather than papered over
inside it:

- **Line-break hyphens.** The page is reported as written, so the hyphen is usually there and
  `doc::rejoin_across_break` takes it as the evidence it is. Where a document leaves none, the
  document's own vocabulary settles the join instead.
- **Word breaks.** The reader synthesises space marks on a gap heuristic, and `segment_words`
  treats those marks as authoritative on any line that has them; inferring gaps instead splits
  *inside* words (`I mage`), because ink gaps after a narrow letter look exactly like spaces. The
  cost is that a line that is under-marked runs the rest of it together, so mark completeness is a
  real obligation on a `PageSource`. rustium's threshold had to drop to 0.12 em to meet it —
  justification compresses a word space to about 0.19 em, and at 0.20 em whole lines were being
  missed.

Two further `PageSource` obligations that only became visible through this pipeline, both now met
and worth re-checking against any future reader:

- **Glyph boxes must be the document's**, not a substitute face's. When a font embeds no usable
  program the substitute's letters are another typeface's, so its ink widths make inter-glyph
  gaps depend on which fonts are installed — enough to invent a word break in a letterspaced
  heading. Horizontal extent has to come from the advance.
- **CID-keyed CFF needs its charset inverted.** A CID is not a glyph id there, and `/CIDToGIDMap`
  is a CIDFontType2 mechanism that does not apply. Getting this wrong is quiet: the text is
  correct, because text comes from the encoding, and only the reported boxes are another
  letter's.
