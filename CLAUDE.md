# Working on this repository

Notes for AI coding agents. Human-facing docs are `README.md` (what it does) and
`docs/ARCHITECTURE.md` (how, and what was learned the hard way).

## Build and test

```sh
scripts/fetch-corpus.sh    # 10 arXiv PDFs, not committed; corpus tests skip without it
scripts/build.sh           # cargo build --release AND install the Python extension
cargo test --release
python3 -m unittest discover -s eval/tests

scripts/fetch-pdfium.sh    # only for the optional pdfium backend, below
```

**Use `scripts/build.sh`, not bare `cargo build`.** The eval harness loads a compiled extension
module; a bare cargo build leaves it stale and the harness then reports "no change" for a change
that worked. It warns when this happens, but the warning is easy to miss.

## Backends

Two implement `PageSource`, chosen by cargo feature:

| feature | backend | notes |
| --- | --- | --- |
| `rustium` (default) | pure Rust, no FFI | `Send + Sync`, no global state, nothing to install |
| `pdfium` | Chromium's engine via FFI | needs `libpdfium.so`; the accuracy reference |

```sh
cargo test --release                                            # rustium
cargo test --release -p rustypdf --no-default-features --features pdfium
```

pdfium currently scores better — see the table below — so measure any backend change against
both before concluding anything.

## Measure, do not assume

Quality is a number here, and every change to a pass should be justified by it:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rp2m_eval --baseline baseline.json
```

Exits non-zero if any paper regresses by more than 0.005. Refresh `baseline.json` deliberately,
with `--json > baseline.json`, only when a change is an intended improvement.

Current, by backend:

| backend | prose bigram | equation recall | equation fidelity | corpus tests |
| --- | --- | --- | --- | --- |
| pdfium | 0.894 | 0.375 | 0.557 | 31/31 |
| rustium (default) | 0.891 | 0.375 | 0.549 | 31/31 |

`baseline.json` holds the pdfium numbers. The maths numbers are the project's weak point on
either backend and are the honest place to work next; the two backends are otherwise within
0.003 of each other and both pass the whole corpus.

rustium converts the corpus in 2.06 s to pdfium's 1.94 s, in 63 MB of resident memory to
pdfium's 95 MB.

## Rules that have earned their place

- **Edit code with the Edit tool, never a scripted string replace.** `cargo fmt` reflows code,
  and an exact-match replace then fails *silently*. This cost three separate debugging sessions.
- **Never commit with failing tests.** It happened twice; both had to be unpicked.
- **The corpus is the specification.** Ten papers across six template families. A converter tuned
  on one family passes its own tests and fails on everything else — see the findings section of
  `docs/ARCHITECTURE.md` for six real bugs that only appeared when the corpus widened.
- **Prefer an absent field to a wrong one.** Reference parsing omits authors it cannot parse;
  maths falls back to a rendered crop rather than emitting confident-looking wrong LaTeX.
- **Check which extension Python actually imported**, with `rustypdf._rustypdf.__file__`. The
  module is built `abi3`, and CPython prefers `_rustypdf.abi3.so` over a plain `_rustypdf.so`
  when both exist. `build.sh` used to install the plain name, so an old abi3 build beside it was
  loaded instead — silently, for every eval run. A whole backend comparison was measured against
  the wrong binary before this surfaced. `build.sh` now writes the abi3 name and deletes the
  other; do not reintroduce a second name.
- **`default-features = false` belongs on the workspace dependency**, not on the member that
  inherits it. Cargo ignores it on the member, so `--no-default-features` on a member silently
  compiled *both* backends in and the feature-selected one won regardless of the flag.

## Shape

`PageRaw` (glyphs, paths, images) → lines → layout → `Document` → emitters. Each stage is a pass
over an IR and only knows the stage before it. `Document` is the contract; Markdown, Typst and
text are renderings of it, which is why the model was never allowed to become "whatever Markdown
can express".

Each backend is confined to one module behind the `PageSource` trait, and the shape classification
they must agree on (`classify_path`, `clip_to_page`, `resolve_size`, `expand_ligature`) lives in
`backend/mod.rs` — downstream passes must not be able to tell which backend built the `PageRaw`.

pdfium is **not thread-safe** and pdfium-render's `thread_safe` feature does no locking, so that
backend serialises ingest behind a lock and the pure-Rust stages are what run in parallel. rustium
has no global state and is `Send + Sync`, so ingest could be parallelised on it — the pipeline does
not yet do so.

Two places where the backends genuinely differ, both handled above the trait rather than papered
over inside it:

- **Line-break hyphens.** pdfium deletes them from the text page; rustium reports the page as
  written. `doc::rejoin_across_break` handles both, and prefers the hyphen when it is there —
  it is better evidence than the vocabulary heuristic pdfium forces us into.
- **Word breaks.** Both synthesise space marks on their own gap heuristic. `segment_words` treats
  the marks as authoritative on any line that has them; inferring gaps instead splits *inside*
  words (`I mage`), because ink gaps after a narrow letter look exactly like spaces. The cost is
  that a backend which under-marks a line runs the rest of it together, so mark completeness is a
  real obligation on a `PageSource`. rustium's threshold had to drop to 0.12 em to meet it —
  justification compresses a word space to about 0.19 em, and at 0.20 em whole lines were being
  missed.

Two `PageSource` obligations that only became visible through this pipeline, both now met by
rustium and worth re-checking in any future backend:

- **Glyph boxes must be the document's**, not a substitute face's. When a font embeds no usable
  program the substitute's letters are another typeface's, so its ink widths make inter-glyph
  gaps depend on which fonts are installed — enough to invent a word break in a letterspaced
  heading. Horizontal extent has to come from the advance.
- **CID-keyed CFF needs its charset inverted.** A CID is not a glyph id there, and `/CIDToGIDMap`
  is a CIDFontType2 mechanism that does not apply. Getting this wrong is quiet: the text is
  correct, because text comes from the encoding, and only the reported boxes are another
  letter's.
