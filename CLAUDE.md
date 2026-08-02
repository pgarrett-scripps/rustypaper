# Working on this repository

Notes for AI coding agents. Human-facing docs are `README.md` (what it does) and
`docs/ARCHITECTURE.md` (how, and what was learned the hard way).

## Build and test

```sh
scripts/fetch-pdfium.sh    # vendored pdfium, pinned; required before anything builds
scripts/fetch-corpus.sh    # 10 arXiv PDFs, not committed; corpus tests skip without it
scripts/build.sh           # cargo build --release AND install the Python extension
cargo test --release
python3 -m unittest discover -s eval/tests
```

**Use `scripts/build.sh`, not bare `cargo build`.** The eval harness loads a compiled extension
module; a bare cargo build leaves it stale and the harness then reports "no change" for a change
that worked. It warns when this happens, but the warning is easy to miss.

## Measure, do not assume

Quality is a number here, and every change to a pass should be justified by it:

```sh
cd eval && PYTHONPATH=.:../python python3 -m rp2m_eval --baseline baseline.json
```

Exits non-zero if any paper regresses by more than 0.005. Refresh `baseline.json` deliberately,
with `--json > baseline.json`, only when a change is an intended improvement.

Current: prose bigram recall **0.894**, equation recall **0.375**, equation fidelity **0.557**.
The maths numbers are the project's weak point and the honest place to work next.

## Rules that have earned their place

- **Edit code with the Edit tool, never a scripted string replace.** `cargo fmt` reflows code,
  and an exact-match replace then fails *silently*. This cost three separate debugging sessions.
- **Never commit with failing tests.** It happened twice; both had to be unpicked.
- **The corpus is the specification.** Ten papers across six template families. A converter tuned
  on one family passes its own tests and fails on everything else — see the findings section of
  `docs/ARCHITECTURE.md` for six real bugs that only appeared when the corpus widened.
- **Prefer an absent field to a wrong one.** Reference parsing omits authors it cannot parse;
  maths falls back to a rendered crop rather than emitting confident-looking wrong LaTeX.

## Shape

`PageRaw` (glyphs, paths, images) → lines → layout → `Document` → emitters. Each stage is a pass
over an IR and only knows the stage before it. `Document` is the contract; Markdown, Typst and
text are renderings of it, which is why the model was never allowed to become "whatever Markdown
can express".

pdfium is confined to `backend/pdfium.rs` behind the `PageSource` trait. It is **not thread-safe**
and pdfium-render's `thread_safe` feature does no locking — ingest is serialised behind a lock and
the pure-Rust stages are what run in parallel.
