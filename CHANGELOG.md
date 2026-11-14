# Changelog

Notable changes to rustypaper. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Earlier releases
predate this file; their story is in the tag messages and `docs/ARCHITECTURE.md`.

## [0.2.3] - 2026-11-14

### Added

- Unlabelled author-year bibliographies (Springer, APA, Harvard styles) are now split into
  entries. Each parenthesised year is walked back to the earliest boundary that still parses as
  nothing but an author list; labelled lists still outrank the new signal, an in-entry or
  in-text `(1995)` never fires it, and the stray DOI tail a column break strands joins the
  entry it ends instead of opening the next one. (#6)

### Fixed

- Superscript character runs keep their spans through paragraph assembly, so citation
  superscripts link even where a font change used to flatten them, and numeric superscripts in
  author bylines and correspondence notes are no longer linked as citations — they are
  affiliation markers. Title badges (journal flags set above the title) are stripped rather
  than emitted as stray headings. (#5)
- A bibliography heading fused into the first entry is recognised even when the heading ends in
  a digit, and zero-advance space marks no longer break words apart. (#5)
- A zero-padded numeral is no longer taken for an entry label, so the dates inside a
  spaced-out DOI (`2016. 01. 026`) cannot masquerade as the label sequence that outranks every
  other splitting signal. (#6)

### Changed

- The PDF backend is now rustium-pdf 0.1.3, which honours `/ActualText`: ligatures and digits
  that Chromium-printed PDFs carry only as replacement text are restored, instead of being read
  as the missing glyphs they are drawn with. Whitespace and control characters in a replacement
  become a plain space rather than swallowing the word boundary — publisher PDFs declare the
  space after a reference label as a tab, which 0.1.2 took literally and fused `1.Lahiry`,
  collapsing whole reference lists. Zero-width marks are dropped, and a non-breaking hyphen
  reads as `-`. A fixture test with ActualText-wrapped spaces now guards the reference-label
  case the corpus (all LaTeX PDFs, no ActualText) cannot see.

## [0.2.2] - 2026-10-11

Prebuilt standalone executables for five platforms, a Python CLI (`rustypaper` and
`python -m rustypaper` from the wheel), and the release pipeline that ships both.

## [0.2.1] - 2026-09-04

Publisher-template hardening across the widened sixteen-paper corpus.

## [0.2.0] - 2026-08-13

The `Document` model and its section map: Markdown, Typst and text become renderings of one
structure instead of the structure itself.

## [0.1.1] - 2026-08-02

First crates.io release of the pure-Rust pipeline.

[0.2.3]: https://github.com/pgarrett-scripps/rustypaper/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/pgarrett-scripps/rustypaper/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/pgarrett-scripps/rustypaper/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/pgarrett-scripps/rustypaper/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/pgarrett-scripps/rustypaper/releases/tag/v0.1.1
