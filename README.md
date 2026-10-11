# rustypaper

**Turn research PDFs into readable text and structured data.**

Rustypaper converts scientific papers to Markdown, plain text, JSON, or Typst. Use it to
save a paper as an editable document, prepare text for search or an LLM, or extract
sections for your own analysis tools. Run it from the terminal, Python, or Rust.

It runs locally on your CPU. No GPU, AI model, API key, or document upload is required.
It is designed for PDFs with selectable text, including one-column and two-column papers.
It does not perform OCR on scans or image-only PDFs.

[Quick start](#quick-start) · [Downloads](#install-without-python) ·
[Python API](#python-api) · [Limitations](#conversion-quality-and-limitations)

## Quick start

With Python 3.9 or newer:

```sh
pip install rustypaper
rustypaper convert paper.pdf --out paper.md
```

Replace `paper.pdf` with your PDF's path. This saves the converted Markdown to `paper.md`.
Omit `--out` to print it in the terminal instead.

You can also run the same command through Python:

```sh
python -m rustypaper convert paper.pdf --out paper.md
```

The [PyPI package](https://pypi.org/project/rustypaper/) includes both the terminal command
and the Python library. Precompiled packages are available for the platforms below, so
supported installations do not need a Rust compiler.

## What does the output look like?

Rustypaper recognizes headings, paragraphs, figures, tables, equations, and references,
and reconstructs the reading order of the paper. For example, the start of the Markdown
from the repository's [synthetic sample PDF](python/tests/fixtures/single-column.pdf) is:

```markdown
# Reliable Widget Measurements

Example Author

## Abstract

We describe a simple study of colored widgets on a wooden bench.
```

This is a shortened excerpt. Conversion quality varies with the paper's layout, especially
for equations and complex tables. See [limitations](#conversion-quality-and-limitations).

Choose the output that suits your workflow:

| Format | Useful for | CLI option |
|---|---|---|
| Markdown | Reading, editing, and use in notes or LLM workflows | `--format md` (default) |
| Plain text | Search and text processing | `--format text` |
| JSON | Working with sections, content blocks, and their page locations | `--format json` |
| Typst | Further editing and typesetting in Typst | `--format typst` |

## Common tasks

Save plain text, structured JSON, or a Typst document:

```sh
rustypaper convert paper.pdf --format text --out paper.txt
rustypaper convert paper.pdf --format json --out paper.json
rustypaper convert paper.pdf --format typst --out paper.typ
```

Save figures alongside the Markdown, with links to the image files:

```sh
rustypaper convert paper.pdf --out paper.md --assets figures
```

Convert several papers into an output folder:

```sh
rustypaper convert first.pdf second.pdf --out converted
```

Each PDF gets its own output file. For multiple inputs, `--out` must be a directory.

For all options, run `rustypaper convert --help`. An optional `--caveman light` or
`--caveman hard` setting shortens prose by removing selected words and phrases. It is
off by default and changes the wording, so use the default when you want the original prose.

## Install without Python

Download an executable from the [latest release](https://github.com/pgarrett-scripps/rustypaper/releases/latest).
These downloads do not require Python or a Rust compiler.

| Your computer | Archive name contains |
|---|---|
| Linux on Intel or AMD, 64-bit | `x86_64-unknown-linux-gnu` |
| Linux on ARM64 | `aarch64-unknown-linux-gnu` |
| Mac with an Intel processor | `x86_64-apple-darwin` |
| Mac with Apple Silicon | `aarch64-apple-darwin` |
| Windows on Intel or AMD, 64-bit | `x86_64-pc-windows-msvc` |

Extract the archive, then run `./rustypaper --help` on Linux or macOS, or
`.\rustypaper.exe --help` in Windows PowerShell. Put the executable in a folder on your
`PATH` to use `rustypaper` from any directory. The release includes `SHA256SUMS` for
verifying downloads. Standalone Linux executables require glibc 2.35 or newer.

If you use [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), it can download
the executable for you:

```sh
cargo binstall rustypaper
```

With a Rust toolchain installed, you can also compile from source:

```sh
cargo install rustypaper --locked
```

## Python API

After `pip install rustypaper`, convert a PDF directly in your Python code:

```python
from pathlib import Path
import rustypaper

markdown = rustypaper.to_markdown("paper.pdf")
Path("paper.md").write_text(markdown, encoding="utf-8")
```

To get readable text and structured data from a single conversion:

```python
import rustypaper

result = rustypaper.convert("paper.pdf")
print(result.markdown)

document = result.document
for section in document["sections"]:
    print(section["title"], section["first_page"], section["last_page"])
```

The document contains a title, typed content blocks, and a section outline. Blocks carry
text and page positions. Sections include nested children and `start` / `end` indices,
so `document["blocks"][section["start"]:section["end"]]` selects a section's contents.
Page numbers are zero-based. Content before the first heading has a section title of `None`.

Other helpers include `to_text()`, `to_typst()`, and `to_document()`.
Image-only documents raise `rustypaper.ScannedDocument`.

## Conversion quality and limitations

Rustypaper uses the text and geometry already present in a PDF to recover its structure.
It is designed for scientific papers. Page layout can change during conversion.

- Scanned or image-only documents need a separate OCR tool.
- Equations, complex tables, and unusual layouts can be missed or reconstructed incorrectly.
- Titles split over several lines, subtle subsection headings, and long bibliographies remain
  challenging. Check the converted output when those details matter to your workflow.

We evaluate conversion on 16 papers across 10 template families. Fifteen have usable source
text for comparison. This small corpus helps us detect regressions, and results on other
papers will depend on their layout. See the [evaluation results and methodology](eval/README.md)
for measurements of text recovery, equations, tables, references, and sections.

If a conversion looks wrong, [open an issue](https://github.com/pgarrett-scripps/rustypaper/issues).
Include the Rustypaper version, the command you ran, what you expected, and a public PDF link
or a small sample you can share. `rustypaper --version` prints the installed version.

## For developers

Rustypaper is also a [Rust crate](https://crates.io/crates/rustypaper). Add it to `Cargo.toml`:

```toml
[dependencies]
rustypaper = "0.2"
```

The Rust API exposes `convert()`, `convert_with()`, and `extract()`.
PDF reading is provided by [rustium-pdf](https://github.com/pgarrett-scripps/rustium-pdf).
See the [architecture notes](docs/ARCHITECTURE.md) for how conversion works, the
[evaluation guide](eval/README.md) for quality checks, and the [release guide](RELEASING.md)
for packaging and publishing.

For local development on Linux, with Rust 1.94 or newer, Python 3.9 or newer, and pytest
installed, run these commands from a clone of this repository:

```sh
scripts/build.sh
cargo test --release
PYTHONPATH=python pytest python/tests -q
python3 -m unittest discover -s eval/tests
```

`scripts/build.sh` builds both the CLI and the Python extension used by the tests. The
Python tests use committed synthetic PDFs. To run the optional real-paper corpus tests,
fetch the papers with `scripts/fetch-corpus.sh` before running the tests. See the evaluation
guide for downloading reference sources and comparing results with the quality baseline.

For conversion diagnostics, `rustypaper probe paper.pdf --pages` summarizes the PDF,
and `rustypaper dump paper.pdf --page 0 --pretty` shows the extracted page data.

## License

Licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
