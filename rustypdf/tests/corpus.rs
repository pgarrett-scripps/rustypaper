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

/// Known gap, pinned so the fix is visible when it lands.
///
/// pdfium removes soft line-break hyphens from the text page entirely — Chrome does this so
/// copy-paste rejoins hyphenated words — so the artifact is `learn ing`, not `learn- ing`. That
/// means M2's de-hyphenation cannot key off a trailing hyphen; it has to notice a line ending
/// mid-word and consult the document's own vocabulary.
#[test]
fn hyphenated_line_breaks_are_a_known_gap() {
    let path = paper!("resnet.pdf");
    let doc = rustypdf::convert(&path).expect("conversion failed");
    let md = rustypdf::emit::markdown::render(&doc);

    assert!(
        md.contains("learn ing residual functions"),
        "the split-word artifact changed shape; revisit de-hyphenation"
    );
    assert!(
        !md.contains("learn- ing"),
        "a trailing hyphen survived, so pdfium's behaviour changed"
    );
}
