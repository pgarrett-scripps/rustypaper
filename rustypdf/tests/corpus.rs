//! End-to-end checks against real papers.
//!
//! The corpus is not committed (the PDFs are not ours to redistribute). Run
//! `scripts/fetch-corpus.sh` to populate it; without it these tests skip rather than fail, so a
//! fresh clone still has a green `cargo test`.

use std::path::PathBuf;

use rustypdf::backend::pdfium::PdfiumBackend;
use rustypdf::backend::PageSource;
use rustypdf::ir::{FontTable, PathKind, Rect};
use rustypdf::text::lines::build_lines;

fn corpus(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("corpus")
        .join(name);
    path.exists().then_some(path)
}

macro_rules! paper {
    ($name:expr) => {
        match corpus($name) {
            Some(path) => path,
            None => {
                eprintln!(
                    "skipping: corpus/{} absent (run scripts/fetch-corpus.sh)",
                    $name
                );
                return;
            }
        }
    };
}

#[test]
fn extracts_a_two_column_paper() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    assert_eq!(doc.pages.len(), 12);
    let glyphs: usize = doc.pages.iter().map(|p| p.glyphs.len()).sum();
    assert!(
        glyphs > 20_000,
        "expected a dense paper, got {glyphs} glyphs"
    );

    // Letter portrait, unrotated.
    let first = &doc.pages[0];
    assert_eq!(first.rotation, 0);
    assert!(first.width < first.height, "expected portrait");
}

#[test]
fn glyphs_land_inside_the_page() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    // The single most valuable invariant for the coordinate transform: if the y-flip or the
    // rotation handling were wrong, glyphs would sit outside the page box.
    for page in &doc.pages {
        let bounds = page.bounds();
        let slack = 2.0;
        for glyph in &page.glyphs {
            let b = glyph.bbox;
            assert!(
                b.x0 >= bounds.x0 - slack
                    && b.y0 >= bounds.y0 - slack
                    && b.x1 <= bounds.x1 + slack
                    && b.y1 <= bounds.y1 + slack,
                "page {} glyph {:?} escapes page {:?}",
                page.index,
                b,
                bounds
            );
        }
    }
}

#[test]
fn reading_order_is_not_assumed_but_text_runs_top_down() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    // The title is the largest *horizontal* text on page 1 — the sideways arXiv stamp is not
    // running text and layout never sees it. This is the assumption the heading classifier will
    // rest on, so it is worth pinning now.
    let page = &doc.pages[0];
    let rotated: std::collections::HashSet<usize> = rustypdf::text::lines::rotated_glyphs(page)
        .into_iter()
        .collect();
    let upright = |i: &usize| !rotated.contains(i);
    let max_size = (0..page.glyphs.len())
        .filter(upright)
        .map(|i| page.glyphs[i].size)
        .fold(0.0f32, f32::max);
    let body_size = median_size(page);
    assert!(
        max_size > body_size * 1.4,
        "title ({max_size}) should stand well clear of body text ({body_size})"
    );

    let title_top = (0..page.glyphs.len())
        .filter(upright)
        .map(|i| &page.glyphs[i])
        .filter(|g| (g.size - max_size).abs() < 0.1)
        .map(|g| g.bbox.y0)
        .fold(f32::MAX, f32::min);
    assert!(
        title_top < page.height * 0.25,
        "title should sit in the top quarter of the page, found at y={title_top}"
    );
}

fn median_size(page: &rustypdf::ir::PageRaw) -> f32 {
    let mut sizes: Vec<f32> = page.glyphs.iter().map(|g| g.size).collect();
    assert!(!sizes.is_empty());
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sizes[sizes.len() / 2]
}

#[test]
fn math_papers_use_tex_math_fonts() {
    let path = paper!("adam.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    // Math detection seeds on these font families, so their presence is a precondition for the
    // whole geometric reconstruction approach.
    let names: Vec<String> = doc
        .fonts
        .iter()
        .map(|(_, n)| n.to_ascii_uppercase())
        .collect();
    let has_math_font = names
        .iter()
        .any(|n| n.contains("CMMI") || n.contains("CMSY") || n.contains("CMEX"));
    assert!(has_math_font, "no TeX math font among {names:?}");
}

#[test]
fn booktabs_rules_are_detected() {
    let path = paper!("bert.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    let rules = doc
        .pages
        .iter()
        .flat_map(|p| &p.paths)
        .filter(|p| p.kind == PathKind::HorizontalRule)
        .count();
    assert!(rules > 10, "expected table rules, found {rules}");
}

#[test]
fn renders_a_crop_at_the_requested_size() {
    let path = paper!("resnet.pdf");
    let backend = PdfiumBackend::open(&path).expect("open failed");

    let mut fonts = FontTable::new();
    let page = backend.page(0, &mut fonts).expect("page failed");

    // A 144pt x 72pt window rendered at 144 dpi is exactly 288 x 144 pixels.
    let region = Rect::from_corners(72.0, 72.0, 216.0, 144.0);
    let png = backend
        .render_region(0, region, 144.0)
        .expect("render failed");

    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");

    // PNG IHDR carries width and height as big-endian u32 at bytes 16..24.
    let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
    assert_eq!((width, height), (288, 144), "crop geometry is wrong");

    // And the crop must stay inside a page that we know the size of.
    assert!(region.x1 <= page.width && region.y1 <= page.height);
}

/// Regression test for the heap corruption that concurrent pdfium access causes.
///
/// pdfium-render's `thread_safe` feature only adds `Send`/`Sync` impls; it does no locking, so
/// without our own lock this aborts with `free(): corrupted unsorted chunks`. It reliably
/// reproduced within a handful of iterations before the lock was added.
#[test]
fn concurrent_extraction_does_not_corrupt_pdfium() {
    let path = paper!("resnet.pdf");

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let backend = PdfiumBackend::open(&path).expect("open failed");
                let mut fonts = FontTable::new();
                let mut total = 0;
                for round in 0..4 {
                    let page = backend
                        .page(round % backend.page_count(), &mut fonts)
                        .expect("page failed");
                    total += page.glyphs.len();
                }
                total
            })
        })
        .collect();

    for handle in handles {
        let glyphs = handle.join().expect("worker panicked");
        assert!(glyphs > 0);
    }
}

#[test]
fn crops_are_clamped_to_the_page() {
    let path = paper!("resnet.pdf");
    let backend = PdfiumBackend::open(&path).expect("open failed");

    // Deliberately oversized: math bounding boxes can spill past the page box, and this must
    // clamp rather than panic.
    let region = Rect::from_corners(-50.0, -50.0, 10_000.0, 10_000.0);
    let png = backend
        .render_region(0, region, 72.0)
        .expect("oversized crop should clamp, not fail");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
}

/// Word segmentation must come out right on prose, including the cases that a fixed gap
/// threshold gets wrong.
#[test]
fn words_are_segmented_correctly_on_body_text() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");

    let text: String = build_lines(&doc.pages[0])
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n");

    // `learning framework` is kerned to a 0.18 em ink gap against a 0.30 em typical space, so
    // any fixed threshold safe for kerning merges it. pdfium's generated space marks it.
    assert!(
        text.contains("residual learning framework"),
        "kerned space was swallowed"
    );
    // An author line has three gap classes (kerning, spaces, author separators); inferring the
    // threshold from the line's own distribution alone merges first and last names.
    assert!(
        text.contains("Kaiming He") && text.contains("Xiangyu Zhang"),
        "author names were merged"
    );
    assert!(text.contains("Deep Residual Learning for Image Recognition"));
}

/// The sideways arXiv stamp down the left margin must not be interleaved into body text.
#[test]
fn the_rotated_arxiv_stamp_is_excluded() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::extract(&path).expect("extraction failed");
    let page = &doc.pages[0];

    let rotated = rustypdf::text::lines::rotated_glyphs(page);
    assert!(!rotated.is_empty(), "resnet has a sideways arXiv stamp");
    // pdfium reports a scaled size of 0 for purely rotated text; the backend must have
    // substituted a usable size rather than letting a zero reach layout.
    for &i in &rotated {
        assert!(page.glyphs[i].size > 0.0, "glyph size must be positive");
    }

    let text: String = build_lines(page).iter().map(|l| l.text()).collect();
    assert!(
        !text.contains("arXiv"),
        "the sideways stamp leaked into running text"
    );
}

/// pdfium's glyph-name fallback already resolves TeX ligatures, so the Unicode repair tables the
/// plan budgeted for are not needed. This pins that: if a backend change ever regresses it, the
/// classic dropped-ligature spellings reappear.
#[test]
fn tex_ligatures_survive_extraction() {
    for name in ["resnet.pdf", "adam.pdf", "bert.pdf", "transformer.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::extract(&path).expect("extraction failed");
        let text: String = doc
            .pages
            .iter()
            .flat_map(build_lines)
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join(" ");

        for broken in ["dierent", "eective", "signicant", "dicult", "specic"] {
            assert!(
                !text.contains(broken),
                "{name}: ligature dropped, found {broken:?}"
            );
        }
        // Raw ligature codepoints must have been decomposed, not passed through.
        assert!(
            !text.chars().any(|c| ('\u{FB00}'..='\u{FB06}').contains(&c)),
            "{name}: un-expanded ligature codepoint"
        );
    }
}

/// End-to-end conversion of a two-column paper.
#[test]
fn converts_a_two_column_paper_in_reading_order() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");
    let md = rustypdf::emit::markdown::render(&doc);

    assert_eq!(
        doc.title.as_deref(),
        Some("Deep Residual Learning for Image Recognition")
    );
    assert!(md.starts_with("# Deep Residual Learning for Image Recognition"));

    // The abstract must read as continuous prose. Before the column pass it was interleaved
    // line-by-line with the tick labels of the figure in the opposite column.
    assert!(
        md.contains("Deeper neural networks are more difficult to train. We present a residual"),
        "abstract is not continuous"
    );

    // Sections must be found and ordered.
    let heading_positions: Vec<usize> = ["## 1. Introduction", "## 2. Related Work"]
        .iter()
        .map(|h| md.find(h).unwrap_or_else(|| panic!("missing heading {h}")))
        .collect();
    assert!(
        heading_positions.windows(2).all(|w| w[0] < w[1]),
        "headings are out of order"
    );
}

/// Single-column papers must not have phantom columns invented for them.
#[test]
fn single_column_papers_are_not_split_into_columns() {
    for name in ["adam.pdf", "transformer.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::extract(&path).expect("extraction failed");
        for page in &doc.pages {
            let lines = build_lines(page);
            let found = rustypdf::layout::columns::page_gutters(page, &lines);
            assert!(found.is_empty(), "{name} page {}: {found:?}", page.index);
        }
    }
}

/// Words split across a line break are rejoined.
///
/// pdfium removes soft line-break hyphens entirely — Chrome does this so copy-paste rejoins
/// hyphenated words — so there is no hyphen to key off and the break is invisible except in the
/// words themselves. The document's own vocabulary supplies the evidence.
#[test]
fn hyphenated_line_breaks_are_rejoined() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");
    let md = rustypdf::emit::markdown::render(&doc);

    assert!(
        md.contains("learning residual functions"),
        "hyphenated break was not rejoined"
    );
    assert!(!md.contains("learn ing"), "a split word survived");

    // And it must not over-merge: ordinary word pairs stay separate.
    assert!(md.contains("residual learning framework"));
    assert!(!md.contains("residuallearning"));
}

/// Figures are found and bound to their captions.
#[test]
fn figures_are_detected_and_captioned() {
    let path = paper!("transformer.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let figures: Vec<&rustypdf::doc::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypdf::doc::BlockKind::Figure)
        .collect();

    assert!(
        figures.len() >= 4,
        "expected several figures, got {}",
        figures.len()
    );
    assert!(
        figures.iter().any(|f| f.text.starts_with("Figure 1")),
        "Figure 1's caption was not bound to its graphic"
    );
}

/// A figure region must never cover the page. One did — a clipped path reporting bounds tens of
/// thousands of points off-page — and suppressing text inside it removed a page of prose.
#[test]
fn figure_regions_stay_within_the_page() {
    for name in ["resnet.pdf", "bert.pdf", "transformer.pdf", "adam.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::extract(&path).expect("extraction failed");
        for page in &doc.pages {
            let page_area = page.width * page.height;
            for region in rustypdf::figure::regions(page) {
                assert!(
                    page.bounds().contains(&region.bbox),
                    "{name} page {}: region {:?} escapes the page",
                    page.index,
                    region.bbox
                );
                let share = region.bbox.width() * region.bbox.height() / page_area;
                assert!(
                    share <= 0.85,
                    "{name} page {}: region covers {:.0}% of it",
                    page.index,
                    share * 100.0
                );
            }
        }
    }
}

/// Figures are written out and referenced when an assets directory is given.
#[test]
fn figures_are_extracted_to_disk() {
    let path = paper!("resnet.pdf");
    let dir = std::env::temp_dir().join("rustypdf-test-assets");
    let _ = std::fs::remove_dir_all(&dir);

    let options = rustypdf::Options {
        assets: Some(dir.clone()),
        figure_dpi: 72.0,
    };
    let doc = rustypdf::convert_with(&path, &options).expect("conversion failed");
    let md = rustypdf::emit::markdown::render(&doc);

    let written: Vec<_> = std::fs::read_dir(&dir)
        .expect("assets directory")
        .filter_map(|e| e.ok())
        .collect();
    assert!(!written.is_empty(), "no figures written");
    assert!(md.contains("!["), "no image reference in the Markdown");

    for entry in &written {
        let bytes = std::fs::read(entry.path()).expect("read figure");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bulleted lists survive as lists, and isolated markers do not become phantom ones.
#[test]
fn lists_and_footnotes_are_classified() {
    let path = paper!("bert.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let lists = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, rustypdf::doc::BlockKind::ListItem { .. }))
        .count();
    assert!(
        lists >= 3,
        "expected BERT's contribution bullets, got {lists}"
    );

    let footnotes = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypdf::doc::BlockKind::Footnote)
        .count();
    assert!(footnotes >= 5, "expected footnotes, got {footnotes}");
}

/// Tables are reconstructed as tables rather than left as loose text.
#[test]
fn tables_are_reconstructed() {
    let path = paper!("transformer.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let tables: Vec<&rustypdf::doc::TableData> =
        doc.blocks.iter().filter_map(|b| b.table.as_ref()).collect();
    assert!(
        tables.len() >= 3,
        "expected several tables, got {}",
        tables.len()
    );

    // Table 1 compares layer types; its shape and contents are unambiguous.
    let layers = tables
        .iter()
        .find(|t| {
            t.rows
                .iter()
                .flatten()
                .any(|c| c.contains("Self-Attention"))
        })
        .expect("the layer-type table was not found");
    // Its header is two lines deep: `Sequential` / `Operations` wraps. The emitter flattens
    // that into one GFM header row, which is the only shape GFM can express.
    assert_eq!(layers.header_rows, 2);
    assert!(layers.rows[0].iter().any(|c| c.contains("Layer Type")));
    assert!(
        layers.rows.iter().any(|r| r[0].starts_with("Recurrent")),
        "row labels were lost"
    );
    // Every row must have the same number of cells for the emitter to be able to render it.
    let width = layers.rows[0].len();
    assert!(width >= 3);
    assert!(layers.rows.iter().all(|r| r.len() == width), "ragged rows");

    let md = rustypdf::emit::markdown::render(&doc);
    assert!(
        md.contains("| Sequential Operations |"),
        "the wrapped header was not flattened into one row"
    );
}

/// Table cells must not also appear as paragraphs.
#[test]
fn table_content_is_removed_from_the_prose() {
    let path = paper!("transformer.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let prose: String = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypdf::doc::BlockKind::Paragraph)
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        !prose.contains("Maximum Path Length"),
        "a table heading leaked into the prose"
    );
}

/// GFM tables must be well formed: every row the same width as the separator.
#[test]
fn emitted_tables_are_well_formed_gfm() {
    for name in ["transformer.pdf", "resnet.pdf", "bert.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::convert(&path).expect("conversion failed");
        let md = rustypdf::emit::markdown::render(&doc);

        let mut expected: Option<usize> = None;
        for line in md.lines() {
            if !line.starts_with('|') {
                expected = None;
                continue;
            }
            let width = line.matches('|').count();
            match expected {
                None => expected = Some(width),
                Some(w) => assert_eq!(w, width, "{name}: ragged table row {line:?}"),
            }
        }
    }
}

/// Display equations are reconstructed as LaTeX rather than left as scrambled characters.
#[test]
fn display_equations_are_reconstructed() {
    let path = paper!("adam.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let equations: Vec<&rustypdf::doc::MathData> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypdf::doc::BlockKind::Equation)
        .filter_map(|b| b.math.as_ref())
        .collect();

    assert!(
        (5..120).contains(&equations.len()),
        "expected tens of equations, got {} — detection has drifted",
        equations.len()
    );

    let latex: String = equations
        .iter()
        .map(|m| m.latex.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // Greek, scripts and operators are the constructs the geometric pass exists to recover.
    assert!(latex.contains(r"\beta"), "Greek was not named");
    assert!(
        latex.contains('^') || latex.contains('_'),
        "no scripts recovered"
    );

    // Numbered equations keep their number.
    assert!(
        equations.iter().any(|m| m.number.is_some()),
        "no equation numbers were found"
    );

    // Confidence must be a real signal, not a constant.
    assert!(equations
        .iter()
        .all(|m| (0.0..=1.0).contains(&m.confidence)));
}

/// Inline mathematics must stay inside the formula, not swallow the sentence around it.
#[test]
fn inline_maths_does_not_swallow_prose() {
    let path = paper!("adam.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    // Length is not the measure — a real formula can be long. What must not happen is English
    // getting pulled in, which is what an unbounded span looks like.
    let mut worst: (usize, String) = (0, String::new());
    for block in &doc.blocks {
        let mut rest = block.text.as_str();
        while let Some(open) = rest.find('$') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find('$') else { break };
            let span = &rest[..close];
            let prose = span
                .split_whitespace()
                .filter(|w| {
                    !w.starts_with('\\')
                        && w.chars().count() >= 4
                        && w.chars().all(char::is_alphabetic)
                })
                .count();
            if prose > worst.0 {
                worst = (prose, span.to_owned());
            }
            rest = &rest[close + 1..];
        }
    }
    assert!(
        worst.0 < 3,
        "an inline formula swallowed {} English words: {:?}",
        worst.0,
        worst.1
    );
}

/// Reconstruction never emits an empty construct: `\sqrt{}` is worse than the bare symbol.
#[test]
fn no_degenerate_latex_is_emitted() {
    for name in ["adam.pdf", "transformer.pdf", "resnet.pdf", "bert.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::convert(&path).expect("conversion failed");
        let md = rustypdf::emit::markdown::render(&doc);
        for bad in [r"\sqrt{}", r"\frac{}{}", "^{}", "_{}"] {
            assert!(!md.contains(bad), "{name} emitted {bad}");
        }
    }
}

/// The bibliography becomes structured entries, not a wall of text.
#[test]
fn references_are_parsed() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");

    let refs: Vec<&rustypdf::refs::Reference> = doc
        .blocks
        .iter()
        .filter_map(|b| b.reference.as_ref())
        .collect();

    assert!(refs.len() >= 40, "expected ~50 entries, got {}", refs.len());

    // The pattern-bound fields are the ones that must be reliable.
    let years = refs.iter().filter(|r| r.year.is_some()).count();
    assert!(
        years * 10 >= refs.len() * 9,
        "only {years}/{} have a year",
        refs.len()
    );
    assert!(refs
        .iter()
        .all(|r| r.year.is_none_or(|y| (1900..=2030).contains(&y))));

    let arxiv = refs.iter().filter(|r| r.arxiv.is_some()).count();
    assert!(arxiv >= 5, "expected arXiv identifiers, found {arxiv}");

    // Initials must not be mistaken for the end of the author list.
    let authors = refs.iter().filter(|r| r.authors.len() >= 2).count();
    assert!(
        authors * 2 >= refs.len(),
        "only {authors} entries parsed an author list"
    );
    assert!(
        refs.iter().flat_map(|r| &r.authors).all(|a| a.len() > 1),
        "a single-letter 'author' means initials were treated as a sentence break"
    );
}

/// Every paper's bibliography must be found, whatever shape its heading takes.
#[test]
fn every_paper_yields_references() {
    for name in ["resnet.pdf", "bert.pdf", "transformer.pdf", "adam.pdf"] {
        let path = paper!(name);
        let doc = rustypdf::convert(&path).expect("conversion failed");
        let count = doc.blocks.iter().filter(|b| b.reference.is_some()).count();
        assert!(count > 0, "{name}: no bibliography found");
    }
}

/// Numeric citations link to the entries they name.
#[test]
fn citations_link_to_their_entries() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");
    let md = rustypdf::emit::markdown::render(&doc);

    assert!(md.contains("(#ref-"), "no citation links were emitted");
    assert!(md.contains("<a id=\"ref-"), "no anchors were emitted");

    // Every link must have a matching anchor, or the document has dangling references.
    let labels: Vec<String> = doc
        .blocks
        .iter()
        .filter_map(|b| b.reference.as_ref()?.label.clone())
        .collect();
    let mut rest = md.as_str();
    while let Some(i) = rest.find("](#ref-") {
        rest = &rest[i + 7..];
        let end = rest.find(')').unwrap_or(0);
        let label = &rest[..end];
        assert!(
            labels.iter().any(|l| l == label),
            "dangling citation [{label}]"
        );
    }
}
