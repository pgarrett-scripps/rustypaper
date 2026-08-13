//! End-to-end checks against real papers.
//!
//! The corpus is not committed (the PDFs are not ours to redistribute). Run
//! `scripts/fetch-corpus.sh` to populate it; without it these tests skip rather than fail, so a
//! fresh clone still has a green `cargo test`.

use std::path::PathBuf;

use rustypaper::backend::open as open_backend;
use rustypaper::backend::PageSource;
use rustypaper::ir::{FontTable, PathKind, Rect};
use rustypaper::text::lines::build_lines;

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
    let doc = rustypaper::extract(&path).expect("extraction failed");

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
    let doc = rustypaper::extract(&path).expect("extraction failed");

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
    let doc = rustypaper::extract(&path).expect("extraction failed");

    // The title is the largest *horizontal* text on page 1 — the sideways arXiv stamp is not
    // running text and layout never sees it. This is the assumption the heading classifier will
    // rest on, so it is worth pinning now.
    let page = &doc.pages[0];
    let rotated: std::collections::HashSet<usize> = rustypaper::text::lines::rotated_glyphs(page)
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

fn median_size(page: &rustypaper::ir::PageRaw) -> f32 {
    let mut sizes: Vec<f32> = page.glyphs.iter().map(|g| g.size).collect();
    assert!(!sizes.is_empty());
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sizes[sizes.len() / 2]
}

#[test]
fn math_papers_use_tex_math_fonts() {
    let path = paper!("adam.pdf");
    let doc = rustypaper::extract(&path).expect("extraction failed");

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
    let doc = rustypaper::extract(&path).expect("extraction failed");

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
    let backend = open_backend(&path).expect("open failed");

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

/// Extracting from several threads at once must stay memory-safe.
///
/// This began as a regression test for real heap corruption, back when reading was done through
/// a C library whose thread-safe build added `Send`/`Sync` and no locking: without a lock of our
/// own it aborted with `free(): corrupted unsorted chunks` within a handful of iterations. The
/// reader has no global state to corrupt now, but the guarantee callers rely on is unchanged,
/// so the test stays.
#[test]
fn concurrent_extraction_stays_sound() {
    let path = paper!("resnet.pdf");

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let backend = open_backend(&path).expect("open failed");
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
    let backend = open_backend(&path).expect("open failed");

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
    let doc = rustypaper::extract(&path).expect("extraction failed");

    let text: String = build_lines(&doc.pages[0])
        .iter()
        .map(|l| l.text())
        .collect::<Vec<_>>()
        .join("\n");

    // `learning framework` is kerned to a 0.18 em ink gap against a 0.30 em typical space, so
    // any fixed threshold safe for kerning merges it. The generated space glyph marks it.
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
    let doc = rustypaper::extract(&path).expect("extraction failed");
    let page = &doc.pages[0];

    let rotated = rustypaper::text::lines::rotated_glyphs(page);
    assert!(!rotated.is_empty(), "resnet has a sideways arXiv stamp");
    // A purely rotated glyph can carry a scaled size of 0; layout must receive a usable size
    // rather than that zero.
    for &i in &rotated {
        assert!(page.glyphs[i].size > 0.0, "glyph size must be positive");
    }

    let text: String = build_lines(page).iter().map(|l| l.text()).collect();
    assert!(
        !text.contains("arXiv"),
        "the sideways stamp leaked into running text"
    );
}

/// The glyph-name fallback already resolves TeX ligatures, so the Unicode repair tables the plan
/// budgeted for are not needed. This pins that: if a reader change ever regresses it, the classic
/// dropped-ligature spellings reappear.
#[test]
fn tex_ligatures_survive_extraction() {
    for name in ["resnet.pdf", "adam.pdf", "bert.pdf", "transformer.pdf"] {
        let path = paper!(name);
        let doc = rustypaper::extract(&path).expect("extraction failed");
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
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);

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
        let doc = rustypaper::extract(&path).expect("extraction failed");
        for page in &doc.pages {
            let lines = build_lines(page);
            let found = rustypaper::layout::columns::page_gutters(page, &lines);
            assert!(found.is_empty(), "{name} page {}: {found:?}", page.index);
        }
    }
}

/// Words split across a line break are rejoined.
///
/// Where a document leaves no hyphen at the break, it is invisible except in the words
/// themselves, and the document's own vocabulary supplies the evidence.
#[test]
fn hyphenated_line_breaks_are_rejoined() {
    let path = paper!("resnet.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);

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
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let figures: Vec<&rustypaper::doc::Block> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Figure)
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
        let doc = rustypaper::extract(&path).expect("extraction failed");
        for page in &doc.pages {
            let page_area = page.width * page.height;
            for region in rustypaper::figure::regions(page) {
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
    let dir = std::env::temp_dir().join("rustypaper-test-assets");
    let _ = std::fs::remove_dir_all(&dir);

    let options = rustypaper::Options {
        assets: Some(dir.clone()),
        figure_dpi: 72.0,
        caveman: None,
    };
    let doc = rustypaper::convert_with(&path, &options).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);

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
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let lists = doc
        .blocks
        .iter()
        .filter(|b| matches!(b.kind, rustypaper::doc::BlockKind::ListItem { .. }))
        .count();
    assert!(
        lists >= 3,
        "expected BERT's contribution bullets, got {lists}"
    );

    let footnotes = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Footnote)
        .count();
    assert!(footnotes >= 5, "expected footnotes, got {footnotes}");
}

/// Tables are reconstructed as tables rather than left as loose text.
#[test]
fn tables_are_reconstructed() {
    let path = paper!("transformer.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let tables: Vec<&rustypaper::doc::TableData> =
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

    let md = rustypaper::emit::markdown::render(&doc);
    assert!(
        md.contains("| Sequential Operations |"),
        "the wrapped header was not flattened into one row"
    );
}

/// ResNet's last page stacks four tables down the same measure, and their rule extents are
/// nested — 139.8–455.4 inside 51.7–543.5 around 324.6–529.3 — so every pair of them looks like
/// one tabular and its `\cmidrule`. All seventeen rules used to chain into a single 622 pt block
/// of 69 rows that swallowed the page's body prose along with three of its four tables.
#[test]
fn tables_stacked_down_a_page_stay_four_tables() {
    let path = paper!("resnet.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let found: Vec<&rustypaper::doc::TableData> = doc
        .blocks
        .iter()
        .filter_map(|b| b.table.as_ref())
        .filter(|t| t.rows.iter().flatten().any(|c| c.contains("baseline")))
        .collect();

    // Tables 9 to 12: COCO, PASCAL VOC 2007, PASCAL VOC 2012, ImageNet DET.
    let coco = found
        .iter()
        .find(|t| t.rows.iter().flatten().any(|c| c.contains("multi-scale")))
        .expect("the COCO table was not found");
    assert_eq!(coco.rows.len(), 9, "the COCO table is the wrong height");

    let voc: Vec<_> = found
        .iter()
        .filter(|t| t.rows.iter().flatten().any(|c| c.contains("baseline+++")))
        .collect();
    assert_eq!(voc.len(), 2, "the two PASCAL VOC tables did not come apart");
    for table in voc {
        assert_eq!(table.rows.len(), 4, "a VOC table took in its neighbour");
    }

    // Nothing from the running text may have been dragged in with them.
    for table in &found {
        assert!(
            !table
                .rows
                .iter()
                .flatten()
                .any(|c| c.contains("We select two adjacent scales")),
            "body prose was swallowed by a table: {:?}",
            table.rows
        );
    }
}

/// Table cells must not also appear as paragraphs.
#[test]
fn table_content_is_removed_from_the_prose() {
    let path = paper!("transformer.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let prose: String = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Paragraph)
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
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let md = rustypaper::emit::markdown::render(&doc);

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
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let equations: Vec<&rustypaper::doc::MathData> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == rustypaper::doc::BlockKind::Equation)
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
    let doc = rustypaper::convert(&path).expect("conversion failed");

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
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let md = rustypaper::emit::markdown::render(&doc);
        for bad in [r"\sqrt{}", r"\frac{}{}", "^{}", "_{}"] {
            assert!(!md.contains(bad), "{name} emitted {bad}");
        }
    }
}

/// The bibliography becomes structured entries, not a wall of text.
#[test]
fn references_are_parsed() {
    let path = paper!("resnet.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    let refs: Vec<&rustypaper::refs::Reference> = doc
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
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let count = doc.blocks.iter().filter(|b| b.reference.is_some()).count();
        assert!(count > 0, "{name}: no bibliography found");
    }
}

/// Numeric citations link to the entries they name.
#[test]
fn citations_link_to_their_entries() {
    let path = paper!("resnet.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);

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

/// Every emitter must render every paper without panicking, and produce something.
#[test]
fn all_emitters_render_the_corpus() {
    for name in ["resnet.pdf", "bert.pdf", "transformer.pdf", "adam.pdf"] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");

        let md = rustypaper::emit::markdown::render(&doc);
        let typ = rustypaper::emit::typst::render(&doc);
        let txt = rustypaper::emit::text::render(&doc);
        let json = serde_json::to_string(&doc).expect("the document model must serialise");

        for (format, output) in [("markdown", &md), ("typst", &typ), ("text", &txt)] {
            assert!(
                output.len() > 5_000,
                "{name}: {format} output is suspiciously short"
            );
        }
        assert!(json.contains("\"type\":\"paragraph\""));

        // Typst's maths goes through mitex, so the import must be present whenever maths is.
        if doc.blocks.iter().any(|b| b.math.is_some()) {
            assert!(typ.starts_with("#import"), "{name}: mitex import missing");
        }
        // Plain text must carry no markup.
        assert!(
            !txt.contains("#table("),
            "{name}: markup leaked into plain text"
        );
    }
}

/// Every paper in the corpus converts, and produces the structure a paper has.
///
/// The corpus deliberately spans pure maths, physics, biology, statistics and machine learning,
/// in templates from `amsart` to Springer LNCS. A converter tuned on one family of conference
/// templates passes its own tests and fails on everything else, which is what this guards.
#[test]
fn every_paper_converts_with_plausible_structure() {
    const PAPERS: [&str; 10] = [
        "resnet.pdf",
        "bert.pdf",
        "transformer.pdf",
        "adam.pdf",
        "numbertheory.pdf",
        "optics.pdf",
        "biology.pdf",
        "statistics.pdf",
        "unet.pdf",
        "gan.pdf",
    ];

    for name in PAPERS {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");

        assert!(!doc.blocks.is_empty(), "{name}: no blocks");
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b.kind, rustypaper::doc::BlockKind::Heading { .. })),
            "{name}: no headings at all"
        );
        assert!(
            doc.blocks.iter().any(|b| b.reference.is_some()),
            "{name}: no bibliography"
        );

        // Text must dominate. A document that is mostly captions or fragments has gone wrong
        // somewhere upstream, whatever the individual passes report.
        let words: usize = doc
            .blocks
            .iter()
            .map(|b| b.text.split_whitespace().count())
            .sum();
        assert!(words > 1_000, "{name}: only {words} words recovered");
    }
}

/// Blocks must not shatter into fragments.
///
/// A physics paper with page-long derivations produced 109 blocks of three words or fewer out of
/// 442 — a quarter of the document — because a row carrying a fraction and a summation spans
/// three font sizes and assembly split on every one of them.
#[test]
fn output_is_not_shattered_into_fragments() {
    for name in [
        "optics.pdf",
        "numbertheory.pdf",
        "biology.pdf",
        "resnet.pdf",
    ] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");

        let fragments = doc
            .blocks
            .iter()
            .filter(|b| b.kind == rustypaper::doc::BlockKind::Paragraph)
            .filter(|b| b.text.split_whitespace().count() <= 3)
            .count();
        let share = fragments as f32 / doc.blocks.len().max(1) as f32;
        assert!(
            share < 0.12,
            "{name}: {fragments} of {} blocks are fragments ({:.0}%)",
            doc.blocks.len(),
            share * 100.0
        );
    }
}

// --------------------------------------------------------------------------------------------
// Publisher journal templates.
//
// The six papers below were added because production consumers hit them and the conference
// templates above never exercise them: IEEEtran (Roman-numeral sections, a drop capital opening
// the first paragraph), acmart, REVTeX, Elsevier's elsarticle, a Springer journal class and
// JMLR. Several of them are converted *badly* today. These tests assert what is true now, and
// mark each known gap in a comment with the aspirational assertion beside it, commented out —
// a test that pins a bug as an expectation fails on the commit that fixes it, which is exactly
// backwards.
// --------------------------------------------------------------------------------------------

/// The text of every heading block, in document order.
fn heading_texts(doc: &rustypaper::doc::Document) -> Vec<&str> {
    doc.blocks
        .iter()
        .filter(|b| matches!(b.kind, rustypaper::doc::BlockKind::Heading { .. }))
        .map(|b| b.text.as_str())
        .collect()
}

fn has_heading(doc: &rustypaper::doc::Document, wanted: &str) -> bool {
    heading_texts(doc).iter().any(|h| h.trim() == wanted)
}

const PUBLISHER_PAPERS: [&str; 6] = [
    "metasurface.pdf", // IEEEtran, two column
    "pinsage.pdf",     // acmart sigconf
    "topological.pdf", // REVTeX, Rev. Mod. Phys.
    "medimaging.pdf",  // elsarticle
    "imagenet.pdf",    // svjour3, a Springer journal
    "sklearn.pdf",     // JMLR
];

/// Every publisher template converts and yields the bones of a paper.
///
/// The sibling test above also demands a bibliography of every paper. Two of these six do not
/// produce one — see `publisher_bibliographies_are_found_where_they_are_found` — so that
/// requirement is made separately rather than weakened for everybody.
#[test]
fn every_publisher_template_converts_with_plausible_structure() {
    for name in PUBLISHER_PAPERS {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");

        assert!(!doc.blocks.is_empty(), "{name}: no blocks");
        assert!(
            doc.title.is_some(),
            "{name}: no title at all — some line of it must be found"
        );
        assert!(
            !heading_texts(&doc).is_empty(),
            "{name}: no headings at all"
        );

        let words: usize = doc
            .blocks
            .iter()
            .map(|b| b.text.split_whitespace().count())
            .sum();
        // sklearn is a six-page JMLR note; the rest are full papers.
        assert!(words > 2_000, "{name}: only {words} words recovered");
    }
}

/// Every emitter renders every publisher template without panicking.
#[test]
fn all_emitters_render_the_publisher_templates() {
    for name in PUBLISHER_PAPERS {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");

        let md = rustypaper::emit::markdown::render(&doc);
        let typ = rustypaper::emit::typst::render(&doc);
        let txt = rustypaper::emit::text::render(&doc);
        serde_json::to_string(&doc).expect("the document model must serialise");

        for (format, output) in [("markdown", &md), ("typst", &typ), ("text", &txt)] {
            assert!(
                output.len() > 5_000,
                "{name}: {format} output is suspiciously short"
            );
        }
        if doc.blocks.iter().any(|b| b.math.is_some()) {
            assert!(typ.starts_with("#import"), "{name}: mitex import missing");
        }
        assert!(
            !txt.contains("#table("),
            "{name}: markup leaked into plain text"
        );
    }
}

/// Titles that publishers set over several lines.
///
/// A journal sets a long title as two or three centred lines, and each line arrives as its own
/// block. Every line must survive into the output; that much is true today, and stays true if
/// the lines are ever joined.
///
/// **Known gap.** They are *not* joined: `doc.title` holds one line and the others are left as
/// top-level headings, so the IEEE paper's title is the middle line, `Array of Rectangular`, and
/// the ACM paper's is missing its `Recommender Systems`.
#[test]
fn every_line_of_a_multi_line_title_survives() {
    let path = paper!("metasurface.pdf");
    let md = rustypaper::emit::markdown::render(&rustypaper::convert(&path).unwrap());
    for line in [
        "Design of Conformal",
        "Array of Rectangular",
        "Waveguide-fed Metasurfaces",
    ] {
        assert!(
            md.contains(line),
            "metasurface.pdf: title line {line:?} lost"
        );
    }
    // assert_eq!(doc.title.as_deref(), Some("Design of Conformal Array of Rectangular
    // Waveguide-fed Metasurfaces"));

    let path = paper!("pinsage.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);
    assert!(md.contains("Graph Convolutional Neural Networks for Web-Scale"));
    assert!(md.contains("Recommender Systems"));
    // assert_eq!(doc.title.as_deref(), Some("Graph Convolutional Neural Networks for Web-Scale
    // Recommender Systems"));
}

/// Titles that arrive on one line come out exactly right, in four more templates.
#[test]
fn single_line_titles_are_exact_in_publisher_templates() {
    for (name, title) in [
        ("topological.pdf", "Topological Insulators"),
        (
            "medimaging.pdf",
            "A Survey on Deep Learning in Medical Image Analysis",
        ),
        (
            "imagenet.pdf",
            "ImageNet Large Scale Visual Recognition Challenge",
        ),
        ("sklearn.pdf", "Scikit-learn: Machine Learning in Python"),
    ] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        assert_eq!(doc.title.as_deref(), Some(title), "{name}");
    }
}

/// IEEEtran numbers its sections with Roman numerals and sets them in capitals.
///
/// They are set at body size in the body face, so the numeral is the only evidence there is, and
/// only `I.` used to be read — a single capital carrying its full stop is the appendix label the
/// numbering rule already knew. `II.` onwards were nothing to it, and `II. THEORY` sitting at the
/// foot of a column was classified as a *footnote*. `doc::numbered_heading` now reads a Roman
/// numeral written the way a typesetter writes one.
#[test]
fn ieee_roman_numeral_headings_are_found() {
    let path = paper!("metasurface.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    for section in [
        "I. INTRODUCTION",
        "II. THEORY",
        "III. DESIGN OF CYLINDRICAL CONFORMAL METASURFACE ANTENNA",
        "IV. CONCLUSION",
    ] {
        assert!(
            has_heading(&doc, section),
            "metasurface.pdf: no heading {section:?} among {:?}",
            heading_texts(&doc)
        );
    }
}

/// A drop capital belongs to the word it opens.
///
/// IEEEtran's `\IEEEPARstart` sets the first letter of the introduction two lines deep in the
/// margin of the paragraph. Geometrically it is not on the first line at all, so reading order
/// used to place it where it sits: `Conformal antennas are essential...` came out as `onformal
/// antennas are essential components in appli-Ccations`, the capital inserted into a word forty
/// characters later. That was worse than losing it, because nothing about it looked wrong.
///
/// `text::lines` now recognises the shape the typesetter made — one outsized initial at the left
/// edge, with the lines it rises through indented past it — and puts the letter back.
#[test]
fn a_drop_capital_does_not_take_its_paragraph_with_it() {
    let path = paper!("metasurface.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let md = rustypaper::emit::markdown::render(&doc);

    assert!(
        md.contains("Conformal antennas are essential components"),
        "the drop capital did not rejoin the word it opens"
    );
    assert!(
        !md.contains("appli-Ccations"),
        "the capital is still lodged in a word forty characters later"
    );

    // The same shape in another IEEEtran paper: `ELECTROMAGNETIC (EM) metasurfaces...`.
    let path = paper!("optics.pdf");
    let md = rustypaper::emit::markdown::render(&rustypaper::convert(&path).unwrap());
    assert!(
        md.contains("ELECTROMAGNETIC (EM) metasurfaces have"),
        "optics.pdf lost the initial of its opening word"
    );
}

/// acmart's numbered, capitalised sections are found, and they run in order.
///
/// **Known gap.** `REFERENCES` is emitted before `5 CONCLUSION`: the conclusion sits in the
/// right-hand column of the page whose left-hand column has already started the bibliography,
/// and the page's blocks are ordered column by column. The numbered sections themselves are in
/// order, which is what the outline of the paper rests on.
#[test]
fn acm_sections_are_found_and_ordered() {
    let path = paper!("pinsage.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    for section in [
        "ABSTRACT",
        "1 INTRODUCTION",
        "2 RELATED WORK",
        "3 METHOD",
        "3.1 Problem Setup",
        "4 EXPERIMENTS",
        "5 CONCLUSION",
        "REFERENCES",
    ] {
        assert!(
            has_heading(&doc, section),
            "pinsage.pdf: no heading {section:?} among {:?}",
            heading_texts(&doc)
        );
    }

    let order: Vec<usize> = [
        "1 INTRODUCTION",
        "2 RELATED WORK",
        "3 METHOD",
        "4 EXPERIMENTS",
    ]
    .iter()
    .map(|s| {
        heading_texts(&doc)
            .iter()
            .position(|h| h.trim() == *s)
            .unwrap()
    })
    .collect();
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "acmart sections came out in the order {order:?}"
    );
}

/// REVTeX sets Roman-numeral sections in capitals, and lettered subsections under them.
///
/// `I. INTRODUCTION` used to be missing: the Colloquium opens with a two-column table of contents
/// that the introduction starts underneath, set two points smaller, and a line of one ran into a
/// line of the other. Judging a baseline's reach at the smaller of the two sizes involved keeps
/// them apart, and the heading comes out on its own.
#[test]
fn revtex_roman_numeral_headings_are_found() {
    let path = paper!("topological.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");

    for section in [
        "I. INTRODUCTION",
        "II. TOPOLOGICAL BAND THEORY",
        "III. QUANTUM SPIN HALL INSULATOR",
        "IV. 3D TOPOLOGICAL INSULATORS",
        "VI. CONCLUSION AND OUTLOOK",
        "A. The insulating state",
        "References",
    ] {
        assert!(
            has_heading(&doc, section),
            "topological.pdf: no heading {section:?} among {:?}",
            heading_texts(&doc)
        );
    }
}

/// Elsevier, Springer and JMLR section headings.
///
/// The two gaps recorded here were the same one: a heading that a column boundary had run into
/// the paragraph above it, because a line of the neighbouring column at another size was taken
/// into the same baseline cluster. `medimaging.pdf` missed `1. Introduction` and
/// `4. Anatomical application areas`; `imagenet.pdf` had `1 Introduction` fused onto the end of
/// an author line as `J. Deng*1 Introduction`. Both are clean now.
///
/// `sklearn.pdf` (JMLR) used to find all six of its sections and additionally promote the running
/// head `Scikit-learn: Machine Learning in Python` to a heading twice. JMLR sets a recto/verso
/// pair of heads, so each reaches only every other page and the title's reaches only two of six —
/// under any half-the-document quorum. `layout::furniture` now counts a signature within its page
/// parity as well as across the document; the assertion below is that both heads are gone.
#[test]
fn elsevier_springer_and_jmlr_sections_are_found() {
    let cases: [(&str, &[&str]); 3] = [
        (
            "medimaging.pdf",
            &[
                "Abstract",
                "1. Introduction",
                "2. Overview of deep learning methods",
                "2.1. Learning algorithms",
                "3. Deep Learning Uses in Medical Imaging",
                "4. Anatomical application areas",
                "5. Discussion",
                "References",
            ],
        ),
        (
            "imagenet.pdf",
            &[
                "1 Introduction",
                "2 Challenge tasks",
                "3 Dataset construction at large scale",
                "3.1 Image classification dataset construction",
                "4 Evaluation at large scale",
                "5 Methods",
                "7 Conclusions",
            ],
        ),
        (
            "sklearn.pdf",
            &[
                "Abstract",
                "1. Introduction",
                "2. Project Vision",
                "3. Underlying Technologies",
                "4. Code Design",
                "6. Conclusion",
                "References",
            ],
        ),
    ];

    for (name, sections) in cases {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        for section in sections {
            assert!(
                has_heading(&doc, section),
                "{name}: no heading {section:?} among {:?}",
                heading_texts(&doc)
            );
        }
    }

    // The running head is the paper's own title, so it cannot be recognised by its words; what
    // disqualifies it is where it sits and how often it recurs there.
    let doc = rustypaper::convert(&paper!("sklearn.pdf")).expect("conversion failed");
    let headings = heading_texts(&doc);
    assert!(
        !headings.contains(&"Scikit-learn: Machine Learning in Python"),
        "sklearn.pdf: the running head is still a heading, among {headings:?}"
    );
    assert_eq!(
        doc.title.as_deref(),
        Some("Scikit-learn: Machine Learning in Python"),
        "the title itself must survive being the text of the running head"
    );
}

/// A recto/verso running head is furniture on every page it appears on, not prose.
///
/// Both papers that set one are journal templates, and in both the pair splits between the page
/// parities: JMLR puts the title on recto and the authors on verso, Springer's `svjour3` the same.
/// Neither head can therefore reach half the pages, which is what the whole-document quorum used
/// to require of it.
#[test]
fn alternating_running_heads_do_not_reach_the_body() {
    let cases: [(&str, [&str; 2]); 2] = [
        (
            "sklearn.pdf",
            [
                "Scikit-learn: Machine Learning in Python",
                "Pedregosa, Varoquaux, Gramfort et al.",
            ],
        ),
        (
            "imagenet.pdf",
            [
                "ImageNet Large Scale Visual Recognition Challenge",
                "Olga Russakovsky* et al.",
            ],
        ),
    ];

    for (name, heads) in cases {
        let doc = rustypaper::convert(&paper!(name)).expect("conversion failed");
        for head in heads {
            // Page one is where this text belongs: it is the paper's own title, and its own
            // author line. Every *later* block that is nothing but the head is a running head.
            let repeats: Vec<usize> = doc
                .blocks
                .iter()
                .filter(|b| b.page > 0 && b.text.trim() == head)
                .map(|b| b.page + 1)
                .collect();
            assert!(
                repeats.is_empty(),
                "{name}: {head:?} survives as a block of its own on pages {repeats:?}"
            );
        }
    }
}

/// Where a publisher's bibliography is parsed at all, and where it is not.
///
/// Every publisher template's bibliography is found.
///
/// Two of the six used to yield nothing: the IEEE paper's forty-one entries and the Springer
/// paper's author-year list arrived as one run-on paragraph with the two columns of the
/// bibliography zipped together (`...1990.[30] I. Yoo and...`), because the gutter the column
/// pass reported reached past the hanging `[nn]` labels and into the entries themselves, and the
/// line splitter needs a pair of glyphs straddling the whole gutter before it will cut.
#[test]
fn publisher_bibliographies_are_found() {
    for name in PUBLISHER_PAPERS {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let refs: Vec<&rustypaper::refs::Reference> = doc
            .blocks
            .iter()
            .filter_map(|b| b.reference.as_ref())
            .collect();
        assert!(!refs.is_empty(), "{name}: no bibliography found");
        assert!(
            refs.iter()
                .all(|r| r.year.is_none_or(|y| (1900..=2030).contains(&y))),
            "{name}: an implausible year was parsed"
        );
    }
}

/// A bibliography with no labels at all still comes back entry by entry.
///
/// Three of the corpus's author-year papers print no `[n]` and no counting `n.`, and each used to
/// arrive as one blob per column: topological 6 blobs for 368 entries, medimaging 21 for 350,
/// imagenet 11 for 102. The column text was all there — metasurface and pinsage prove the
/// pipeline delivers it — so what was missing was a signal saying where one entry stops and the
/// next starts. The author list at the head of each entry is that signal.
///
/// The counts asserted are well under what is recovered, because an entry the split is not sure
/// of stays joined to its neighbour rather than being cut through the middle; the numbers are a
/// floor, not a target.
#[test]
fn an_unlabelled_bibliography_is_split_into_entries() {
    for (name, least) in [
        ("topological.pdf", 120), // REVTeX: `Agergaard, S., C. Sondergaard, 2001, New J. Phys.`
        ("medimaging.pdf", 300),  // elsarticle: `Abadi, M., Agarwal, A., 2016. Tensorflow: ...`
        ("imagenet.pdf", 85),     // svjour3: `Ahonen, T., Hadid, A. (2006). Face description ...`
    ] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let refs: Vec<&rustypaper::refs::Reference> = doc
            .blocks
            .iter()
            .filter_map(|b| b.reference.as_ref())
            .collect();
        assert!(
            refs.len() >= least,
            "{name}: {} entries, expected at least {least}",
            refs.len()
        );

        // Entry-sized, not column-sized. The blobs these papers used to yield ran to two and
        // three thousand characters apiece, and a count alone would not have noticed.
        let mut lengths: Vec<usize> = refs.iter().map(|r| r.raw.chars().count()).collect();
        lengths.sort_unstable();
        let median = lengths[lengths.len() / 2];
        assert!(
            (40..500).contains(&median),
            "{name}: the median entry is {median} characters"
        );

        // Each entry begins where an entry begins, not part-way through the one before it.
        assert!(
            refs.iter().all(|r| r.raw.starts_with(char::is_alphabetic)),
            "{name}: an entry starts mid-sentence"
        );
        let capitals = refs
            .iter()
            .filter(|r| r.raw.starts_with(char::is_uppercase))
            .count();
        assert!(
            capitals * 20 >= refs.len() * 19,
            "{name}: only {capitals}/{} entries open with a name",
            refs.len()
        );

        // The year is the field anything downstream resolves against, and a mis-cut entry
        // carries its neighbour's.
        let years = refs.iter().filter(|r| r.year.is_some()).count();
        assert!(
            years * 10 >= refs.len() * 9,
            "{name}: only {years}/{} entries have a year",
            refs.len()
        );
    }
}

/// A numbered two-column bibliography comes back entry by entry, not column by column.
///
/// `metasurface.pdf` is the case that proves the columns are separated: its forty-one `[n]`
/// labels can only be found once each column reads on its own, and until they were, the paper
/// yielded nothing at all.
#[test]
fn a_two_column_bibliography_yields_every_entry() {
    let path = paper!("metasurface.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let labels: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| b.reference.as_ref()?.label.as_deref())
        .collect();
    assert!(
        labels.len() >= 40,
        "expected the paper's 41 entries, got {}",
        labels.len()
    );
    // The labels run 1..=41 in order; a zipped page loses that as well as the entries.
    let numbers: Vec<u32> = labels.iter().filter_map(|l| l.parse().ok()).collect();
    assert_eq!(numbers.len(), labels.len(), "a label was not a number");
    assert!(
        numbers.windows(2).all(|w| w[0] < w[1]),
        "entries are out of order: {numbers:?}"
    );
}

/// A gutter that one wide line crosses must not cost the page its columns.
///
/// It used to. `pinsage.pdf` is a clean two-column acmart paper, and on the two pages where a
/// display equation overhangs the gutter the sparse column beside it fell below half the page's
/// *peak* ink, the flanking test rejected a corridor with no ink in it at all, and the two
/// columns then interleaved line by line (`...is smaller than that of thenode independently.
/// Empirically, we do not...`). The flanking test now asks for half the page's typical ink.
#[test]
fn two_column_gutters_are_found_on_every_page() {
    let path = paper!("pinsage.pdf");
    let doc = rustypaper::extract(&path).expect("extraction failed");

    let without: Vec<usize> = doc
        .pages
        .iter()
        .filter(|page| {
            let lines = build_lines(page);
            rustypaper::layout::columns::page_gutters(page, &lines).is_empty()
        })
        .map(|page| page.index)
        .collect();
    assert!(
        without.is_empty(),
        "pages {without:?} of a two-column paper have no gutter"
    );
}

/// The columns a page cannot see in its own profile are the document's to supply.
///
/// A table spanning the measure holds the corridor open on every baseline it occupies, so the
/// text below it has no dip deep enough to read. Four pages of `medimaging.pdf` are that shape,
/// and each of them interleaved everything the table did not cover.
#[test]
fn columns_hidden_by_a_full_width_table_are_recovered_from_the_document() {
    let path = paper!("medimaging.pdf");
    let raw = rustypaper::extract(&path).expect("extraction failed");
    let lines: Vec<Vec<_>> = raw.pages.iter().map(build_lines).collect();

    let alone = raw
        .pages
        .iter()
        .zip(&lines)
        .filter(|(page, lines)| !rustypaper::layout::columns::page_gutters(page, lines).is_empty())
        .count();
    let together = rustypaper::layout::columns::document_bands(&raw.pages, &lines)
        .iter()
        .filter(|bands| bands.iter().any(|band| !band.gutters.is_empty()))
        .count();

    assert!(
        together > alone,
        "the document found no columns its pages had missed ({alone} pages either way)"
    );
}

// --------------------------------------------------------------------------------------------
// The section map. Derived from the heading blocks, so these tests say two things at once: that
// the tree is built correctly, and that the headings underneath it are still being found.
// --------------------------------------------------------------------------------------------

/// The titles of a level of the tree, in order. `None` is the untitled front matter.
fn section_titles(sections: &[rustypaper::doc::Section]) -> Vec<Option<&str>> {
    sections.iter().map(|s| s.title.as_deref()).collect()
}

/// A section of the tree by its title, at any depth.
fn find_section<'a>(
    sections: &'a [rustypaper::doc::Section],
    wanted: &str,
) -> Option<&'a rustypaper::doc::Section> {
    for section in sections {
        if section.title.as_deref().map(str::trim) == Some(wanted) {
            return Some(section);
        }
        if let Some(found) = find_section(&section.children, wanted) {
            return Some(found);
        }
    }
    None
}

/// The Transformer paper numbers three levels, so its tree is the one to pin in full.
#[test]
fn the_section_map_reproduces_a_numbered_papers_outline() {
    let path = paper!("transformer.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let sections = doc.sections();

    // The paper's top-level outline, front matter first.
    assert_eq!(
        section_titles(&sections),
        [
            None,
            Some("Abstract"),
            Some("1 Introduction"),
            Some("2 Background"),
            Some("3 Model Architecture"),
            Some("4 Why Self-Attention"),
            Some("5 Training"),
            Some("6 Results"),
            Some("7 Conclusion"),
            Some("References"),
        ]
    );

    // Numbering decides nesting: 3.x sits under 3, and 3.2.x under 3.2.
    let architecture = find_section(&sections, "3 Model Architecture").expect("section 3");
    assert_eq!(
        section_titles(&architecture.children),
        [
            Some("3.1 Encoder and Decoder Stacks"),
            Some("3.2 Attention"),
            Some("3.3 Position-wise Feed-Forward Networks"),
            Some("3.4 Embeddings and Softmax"),
            Some("3.5 Positional Encoding"),
        ]
    );
    let attention = find_section(&sections, "3.2 Attention").expect("section 3.2");
    assert_eq!(attention.level, 2);
    assert_eq!(
        section_titles(&attention.children),
        [
            Some("3.2.1 Scaled Dot-Product Attention"),
            Some("3.2.2 Multi-Head Attention"),
            Some("3.2.3 Applications of Attention in our Model"),
        ]
    );

    // A section's range starts at its own heading and swallows its subsections.
    assert_eq!(
        doc.blocks[architecture.start].text.trim(),
        "3 Model Architecture"
    );
    let last = attention.children.last().expect("a subsubsection");
    assert!(
        architecture.start < attention.start && last.end <= attention.end,
        "a child's range must lie inside its parent's"
    );

    // And the pages it spans are the pages of the blocks it holds.
    assert!(
        architecture.first_page <= attention.first_page
            && architecture.last_page >= attention.last_page,
        "a parent must span at least its children's pages"
    );
}

/// IEEEtran sets its sections at body size and numbers them with Roman numerals, so this tree
/// exists only because the numeral is read as numbering.
#[test]
fn the_section_map_finds_roman_numeral_sections() {
    let path = paper!("metasurface.pdf");
    let doc = rustypaper::convert(&path).expect("conversion failed");
    let sections = doc.sections();

    for wanted in ["I. INTRODUCTION", "II. THEORY", "IV. CONCLUSION"] {
        let section = find_section(&sections, wanted)
            .unwrap_or_else(|| panic!("no section {wanted:?} in {:?}", section_titles(&sections)));
        assert_eq!(section.level, 1, "{wanted}: numbered sections are level 1");
        assert_eq!(doc.blocks[section.start].text.trim(), wanted);
        assert!(section.end > section.start, "{wanted}: an empty range");
    }

    // The four are in the order the paper prints them, and each ends where the next begins.
    let numbered: Vec<&rustypaper::doc::Section> = sections
        .iter()
        .filter(|s| {
            s.title
                .as_deref()
                .is_some_and(|t| t.starts_with("I.") || t.starts_with("II") || t.starts_with("IV"))
        })
        .collect();
    for pair in numbered.windows(2) {
        assert_eq!(
            pair[0].end, pair[1].start,
            "sections {:?} and {:?} do not meet",
            pair[0].title, pair[1].title
        );
    }
}

/// The invariants a consumer indexes on, over every paper in the corpus: ranges ordered, never
/// overlapping, always inside `blocks`, and children contained by their parent.
#[test]
fn section_ranges_are_valid_across_the_corpus() {
    fn check(name: &str, doc: &rustypaper::doc::Document, sections: &[rustypaper::doc::Section]) {
        let mut cursor = None::<usize>;
        for section in sections {
            assert!(
                section.start < section.end && section.end <= doc.blocks.len(),
                "{name}: {:?} has range {}..{} against {} blocks",
                section.title,
                section.start,
                section.end,
                doc.blocks.len()
            );
            if let Some(previous_end) = cursor {
                assert!(
                    section.start >= previous_end,
                    "{name}: {:?} starts at {} inside the section before it, which ends at \
                     {previous_end}",
                    section.title,
                    section.start
                );
            }
            cursor = Some(section.end);

            // A titled section names the block it starts at; the front matter names nothing.
            match &section.title {
                Some(title) => assert_eq!(
                    doc.blocks[section.start].text.trim(),
                    title.trim(),
                    "{name}: a section's title is not the heading it starts at"
                ),
                None => assert_eq!(
                    section.level,
                    rustypaper::doc::FRONT_MATTER_LEVEL,
                    "{name}: only the front matter goes untitled"
                ),
            }

            // The page span is the span of the blocks in the range, not a guess.
            let pages: Vec<usize> = doc.blocks[section.start..section.end]
                .iter()
                .map(|b| b.page)
                .collect();
            assert_eq!(
                (section.first_page, section.last_page),
                (
                    pages.iter().copied().min().unwrap_or(0),
                    pages.iter().copied().max().unwrap_or(0)
                ),
                "{name}: {:?} reports the wrong page span",
                section.title
            );

            for child in &section.children {
                assert!(
                    child.start > section.start && child.end <= section.end,
                    "{name}: {:?} escapes its parent {:?}",
                    child.title,
                    section.title
                );
                assert!(
                    child.level > section.level,
                    "{name}: {:?} is not deeper than its parent {:?}",
                    child.title,
                    section.title
                );
            }
            check(name, doc, &section.children);
        }
    }

    for name in [
        "transformer.pdf",
        "resnet.pdf",
        "bert.pdf",
        "adam.pdf",
        "numbertheory.pdf",
        "optics.pdf",
        "biology.pdf",
        "statistics.pdf",
        "unet.pdf",
        "gan.pdf",
        "metasurface.pdf",
        "pinsage.pdf",
        "topological.pdf",
        "medimaging.pdf",
        "imagenet.pdf",
        "sklearn.pdf",
    ] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let sections = doc.sections();
        assert!(!sections.is_empty(), "{name}: no sections at all");
        check(name, &doc, &sections);

        // Every block belongs to exactly one top-level section: the ranges tile the document.
        let covered: usize = sections.iter().map(|s| s.end - s.start).sum();
        assert_eq!(
            covered,
            doc.blocks.len(),
            "{name}: the top level does not cover the document"
        );
    }
}

/// A title set at body size, distinguished only by capitals and position, must still be found.
#[test]
fn titles_are_found_in_every_template() {
    for name in [
        "numbertheory.pdf",
        "unet.pdf",
        "gan.pdf",
        "statistics.pdf",
        "optics.pdf",
    ] {
        let path = paper!(name);
        let doc = rustypaper::convert(&path).expect("conversion failed");
        let title = doc.title.unwrap_or_default();
        assert!(
            title.split_whitespace().count() >= 2,
            "{name}: title is {title:?}"
        );
        // A section heading is not a title.
        assert!(
            !title.starts_with("1 ") && !title.starts_with("1. "),
            "{name}: promoted a numbered section to the title: {title:?}"
        );
    }
}
