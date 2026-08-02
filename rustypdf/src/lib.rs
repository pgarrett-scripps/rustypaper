//! Structure-aware conversion of born-digital scientific PDFs.
//!
//! The pipeline is a sequence of passes over an intermediate representation:
//!
//! ```text
//! PDF --[backend]--> PageRaw (glyphs, paths, images)
//!     --[text]-----> lines and words, with Unicode repaired
//!     --[layout]---> columns, reading order, typed blocks
//!     --[math/table/refs]--> a Document
//!     --[emit]-----> Markdown / Typst / JSON / text
//! ```
//!
//! The `Document` is the real output; Markdown is one rendering of it.

pub mod backend;
pub mod doc;
pub mod emit;
pub mod error;
pub mod figure;
pub mod ir;
pub mod layout;
pub mod math;
pub mod refs;
pub mod table;
pub mod text;

pub use error::{Error, Result};

use std::path::Path;

use backend::pdfium::PdfiumBackend;
use backend::PageSource;
use ir::{DocRaw, FontTable};
use rayon::prelude::*;

/// Options for [`convert_with`].
#[derive(Debug, Clone)]
pub struct Options {
    /// Where to write extracted figures. Figures are still detected when this is `None`; only
    /// the image files are skipped.
    pub assets: Option<std::path::PathBuf>,
    /// Resolution to rasterise figures at.
    pub figure_dpi: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            assets: None,
            figure_dpi: 150.0,
        }
    }
}

/// Extracts every page's primitives from a PDF.
///
/// Fails with [`Error::Scanned`] when the document is image-only, which this pipeline does not
/// handle: there are no glyphs to reconstruct structure from. A document is only rejected when
/// *most* pages are scanned, so a paper with one scanned appendix page still converts.
pub fn extract(path: impl AsRef<Path>) -> Result<DocRaw> {
    let path = path.as_ref();
    extract_from(&PdfiumBackend::open(path)?, path)
}

fn extract_from(backend: &PdfiumBackend, path: &Path) -> Result<DocRaw> {
    let total = backend.page_count();
    let mut fonts = FontTable::new();
    let mut pages = Vec::with_capacity(total);

    for index in 0..total {
        pages.push(backend.page(index, &mut fonts)?);
    }

    let scanned = pages.iter().filter(|p| p.looks_scanned()).count();
    if total > 0 && scanned * 2 > total {
        return Err(Error::Scanned {
            path: path.to_path_buf(),
            scanned,
            total,
        });
    }

    Ok(DocRaw { fonts, pages })
}

/// Converts a PDF into a [`doc::Document`] with default options.
pub fn convert(path: impl AsRef<Path>) -> Result<doc::Document> {
    convert_with(path, &Options::default())
}

/// Converts a PDF into a [`doc::Document`].
///
/// The whole pipeline in order: extract primitives, build lines, split them at column gutters,
/// remove page furniture, put each page into reading order, measure the document, assemble
/// blocks, then attach figures.
pub fn convert_with(path: impl AsRef<Path>, options: &Options) -> Result<doc::Document> {
    let path = path.as_ref();
    let backend = PdfiumBackend::open(path)?;
    let raw = extract_from(&backend, path)?;

    // Everything from here on is pure Rust, so it can run across pages at once. Ingest cannot:
    // pdfium is single-threaded and serialised behind a lock inside the backend.
    let mut pages: Vec<Vec<text::lines::Line>> = raw
        .pages
        .par_iter()
        .map(|page| {
            let lines = text::lines::build_lines(page);
            // Gutters are measured before splitting, because a line spanning two columns is
            // exactly what the coverage profile needs to see in order to discount it.
            let gutters = layout::columns::page_gutters(page, &lines);
            text::lines::split_at_gutters(page, lines, &gutters)
        })
        .collect();

    let heights: Vec<f32> = raw.pages.iter().map(|p| p.height).collect();
    layout::furniture::strip(&mut pages, &heights);

    // Measured after furniture removal so running heads cannot skew the body size, and before
    // reading order because neither depends on the other.
    let stats = layout::stats::Stats::measure(&pages);

    let ordered: Vec<Vec<text::lines::Line>> = pages
        .into_par_iter()
        .map(layout::order::reading_order)
        .collect();

    // Tables are lifted out before assembly so that their cells never become paragraphs. What
    // is left on each page is prose.
    let per_page: Vec<PageTables> = raw
        .pages
        .par_iter()
        .zip(ordered)
        .map(|(page, lines)| {
            let found = table::detect(page, &lines, stats.body_size);
            let mut consumed = vec![false; lines.len()];
            let mut tables = Vec::new();
            for table in &found {
                for &i in &table.consumed {
                    consumed[i] = true;
                }
                tables.push((
                    page.index,
                    table.bbox,
                    doc::TableData {
                        rows: table.rows.clone(),
                        header_rows: table.header_rows,
                    },
                ));
            }
            let prose = lines
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !consumed[*i])
                .map(|(_, line)| line)
                .collect();
            (tables, prose)
        })
        .collect();

    let mut tables: Vec<(usize, ir::Rect, doc::TableData)> = Vec::new();
    let mut prose: Vec<Vec<text::lines::Line>> = Vec::with_capacity(per_page.len());
    #[allow(clippy::needless_late_init)]
    for (found, lines) in per_page {
        tables.extend(found);
        prose.push(lines);
    }

    // Mathematics is resolved before assembly: display equations are lifted out as their own
    // blocks, and inline formulae are folded into the words of the lines that carry them, so
    // that everything downstream sees `$x^2$` as a single word.
    let equations: Vec<(usize, ir::Rect, doc::MathData)> = raw
        .pages
        .par_iter()
        .zip(prose.par_iter_mut())
        .flat_map(|(page, lines)| {
            let column = text_extent(lines);
            let mut is_display = vec![false; lines.len()];
            let mut found_here = Vec::new();

            for i in 0..lines.len() {
                if let Some(found) = math::display(page, &raw.fonts, lines, i, column) {
                    let latex = math::latex::render(&found.formula.root);
                    if latex.trim().is_empty() {
                        continue;
                    }
                    is_display[i] = true;
                    found_here.push((
                        page.index,
                        lines[i].bbox,
                        doc::MathData {
                            latex,
                            number: found.number,
                            confidence: found.formula.confidence,
                        },
                    ));
                }
            }

            for (i, display) in is_display.iter().enumerate() {
                if !display {
                    apply_inline_math(page, &raw.fonts, lines, i);
                }
            }

            let mut keep = is_display.iter().map(|d| !d);
            lines.retain(|_| keep.next().unwrap_or(true));
            found_here
        })
        .collect();

    let vocab = text::vocab::Vocabulary::build(&prose);
    let mut document = doc::assemble(&prose, &heights, stats, &vocab);
    insert_tables(&mut document, tables);
    insert_equations(&mut document, equations);
    attach_figures(&backend, &raw, &mut document, options)?;
    crop_uncertain_equations(&backend, &mut document, options)?;
    extract_bibliography(&mut document);

    Ok(document)
}

/// The tables lifted off one page, and the prose that remains on it.
type PageTables = (
    Vec<(usize, ir::Rect, doc::TableData)>,
    Vec<text::lines::Line>,
);

/// Turns the bibliography into structured entries and links the citations that point at them.
fn extract_bibliography(document: &mut doc::Document) {
    // Either a heading of its own, or a body-size heading that assembly merged into the first
    // entry — both happen across the corpus.
    let mut start = None;
    let mut strip = 0usize;
    for (i, block) in document.blocks.iter().enumerate() {
        if matches!(block.kind, doc::BlockKind::Heading { .. })
            && refs::is_bibliography_heading(&block.text)
        {
            start = Some(i + 1);
            break;
        }
        if block.kind == doc::BlockKind::Paragraph {
            // A heading set at body size is a paragraph as far as classification is concerned,
            // whether it stands alone or was merged into the first entry.
            if refs::is_bibliography_heading(&block.text) {
                start = Some(i + 1);
                break;
            }
            if let Some(offset) = refs::opens_bibliography(&block.text) {
                start = Some(i);
                strip = offset;
                break;
            }
        }
    }
    let Some(first) = start else {
        return;
    };

    // The bibliography runs to the next heading — papers put appendices after it.
    let end = document.blocks[first..]
        .iter()
        .skip(1)
        .position(|b| matches!(b.kind, doc::BlockKind::Heading { .. }))
        .map(|i| first + 1 + i)
        .unwrap_or(document.blocks.len());

    let mut entries: Vec<doc::Block> = Vec::new();
    for (n, block) in document.blocks[first..end].iter().enumerate() {
        if block.kind != doc::BlockKind::Paragraph {
            entries.push(block.clone());
            continue;
        }
        let body = if n == 0 {
            &block.text[strip..]
        } else {
            &block.text
        };
        for text in refs::split_entries(body) {
            let parsed = refs::parse(&text);
            entries.push(doc::Block {
                kind: doc::BlockKind::Reference,
                text,
                page: block.page,
                bbox: block.bbox,
                size: block.size,
                asset: None,
                table: None,
                math: None,
                reference: Some(parsed),
            });
        }
    }

    document.blocks.splice(first..end, entries);

    // Link citations only once the labels are known.
    let labels: Vec<String> = document
        .blocks
        .iter()
        .filter_map(|b| b.reference.as_ref()?.label.clone())
        .collect();
    if labels.is_empty() {
        return;
    }
    for block in &mut document.blocks {
        if matches!(
            block.kind,
            doc::BlockKind::Paragraph | doc::BlockKind::ListItem { .. } | doc::BlockKind::Caption
        ) {
            block.text = refs::link_citations(&block.text, &labels);
        }
    }
}

/// The horizontal extent of a page's text, used as the column a display equation is centred in.
fn text_extent(lines: &[text::lines::Line]) -> ir::Rect {
    lines
        .iter()
        .map(|l| l.bbox)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(ir::Rect::from_corners(0.0, 0.0, 0.0, 0.0))
}

/// Replaces the words covered by an inline formula with a single `$...$` word.
///
/// Rewriting words rather than the finished text means every later pass — paragraph assembly,
/// the vocabulary, table cells — sees the formula as one indivisible token, which is what it is.
fn apply_inline_math(
    page: &ir::PageRaw,
    fonts: &ir::FontTable,
    lines: &mut [text::lines::Line],
    index: usize,
) {
    let spans = math::spans(page, fonts, lines, index);
    if spans.is_empty() {
        return;
    }

    let line = &mut lines[index];
    let mut rebuilt: Vec<text::lines::Word> = Vec::with_capacity(line.words.len());

    for word in &line.words {
        // A word belongs to a span when its glyphs fall inside it.
        let covering = spans
            .iter()
            .find(|s| word.start >= s.start && word.end <= s.end);
        match covering {
            Some(span) => {
                // Emit the formula once, at its first word.
                if rebuilt.last().is_some_and(|w| w.start >= span.start) {
                    continue;
                }
                let latex = math::latex::render(&span.formula.root);
                if latex.trim().is_empty() {
                    rebuilt.push(word.clone());
                    continue;
                }
                rebuilt.push(text::lines::Word {
                    bbox: word.bbox,
                    text: format!("${latex}$"),
                    start: span.start,
                    end: span.end,
                });
            }
            None => rebuilt.push(word.clone()),
        }
    }

    line.words = rebuilt;
}

/// Places display equations into the document at their position in reading order.
fn insert_equations(
    document: &mut doc::Document,
    equations: Vec<(usize, ir::Rect, doc::MathData)>,
) {
    for (page, bbox, math) in equations {
        let block = doc::Block {
            kind: doc::BlockKind::Equation,
            text: String::new(),
            page,
            bbox,
            size: 0.0,
            asset: None,
            table: None,
            math: Some(math),
            reference: None,
        };
        let at = document
            .blocks
            .iter()
            .position(|b| b.page > page || (b.page == page && b.bbox.y0 > bbox.y1))
            .unwrap_or(document.blocks.len());
        document.blocks.insert(at, block);
    }
}

/// Renders a picture of any equation the reconstruction was not confident about.
///
/// The alternative is emitting LaTeX that looks authoritative and is wrong, which is worse than
/// an image: a reader can check an image, and a downstream tool will not silently ingest a
/// mangled formula as fact.
fn crop_uncertain_equations(
    backend: &PdfiumBackend,
    document: &mut doc::Document,
    options: &Options,
) -> Result<()> {
    let Some(dir) = &options.assets else {
        return Ok(());
    };

    let mut counter = 0usize;
    for index in 0..document.blocks.len() {
        let uncertain = document.blocks[index]
            .math
            .as_ref()
            .is_some_and(|m| m.confidence < math::MIN_CONFIDENCE);
        if !uncertain {
            continue;
        }
        counter += 1;

        let block = &document.blocks[index];
        let padded = ir::Rect {
            x0: block.bbox.x0 - 4.0,
            y0: block.bbox.y0 - 4.0,
            x1: block.bbox.x1 + 4.0,
            y1: block.bbox.y1 + 4.0,
        };
        let png = backend.render_region(block.page, padded, options.figure_dpi)?;
        let name = format!("equation-{counter:03}.png");
        let file = dir.join(&name);
        std::fs::write(&file, png).map_err(|source| Error::Io { path: file, source })?;

        let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or("assets");
        document.blocks[index].asset = Some(format!("{folder}/{name}"));
    }
    Ok(())
}

/// Places reconstructed tables into the document at their position in reading order.
fn insert_tables(document: &mut doc::Document, tables: Vec<(usize, ir::Rect, doc::TableData)>) {
    for (page, bbox, data) in tables {
        let block = doc::Block {
            kind: doc::BlockKind::Table,
            text: String::new(),
            page,
            bbox,
            size: 0.0,
            asset: None,
            table: Some(data),
            math: None,
            reference: None,
        };
        let at = document
            .blocks
            .iter()
            .position(|b| b.page > page || (b.page == page && b.bbox.y0 > bbox.y1))
            .unwrap_or(document.blocks.len());
        document.blocks.insert(at, block);
    }
}

/// Binds detected figure regions to their captions, optionally rasterising them.
fn attach_figures(
    backend: &PdfiumBackend,
    raw: &DocRaw,
    document: &mut doc::Document,
    options: &Options,
) -> Result<()> {
    if let Some(dir) = &options.assets {
        std::fs::create_dir_all(dir).map_err(|source| Error::Io {
            path: dir.clone(),
            source,
        })?;
    }

    let mut counter = 0usize;
    let mut swallowed: Vec<(usize, ir::Rect)> = Vec::new();

    for page in &raw.pages {
        for region in figure::regions(page) {
            counter += 1;
            swallowed.push((page.index, region.bbox));

            let asset = match &options.assets {
                Some(dir) => {
                    let name = format!("figure-{:03}.png", counter);
                    let png = backend.render_region(page.index, region.bbox, options.figure_dpi)?;
                    let file = dir.join(&name);
                    std::fs::write(&file, png)
                        .map_err(|source| Error::Io { path: file, source })?;
                    Some(format!(
                        "{}/{name}",
                        dir.file_name().and_then(|n| n.to_str()).unwrap_or("assets")
                    ))
                }
                None => None,
            };

            match caption_for(document, page.index, &region.bbox) {
                Some(index) => {
                    document.blocks[index].kind = doc::BlockKind::Figure;
                    document.blocks[index].asset = asset;
                }
                None => insert_figure(document, page.index, region.bbox, asset),
            }
        }
    }

    drop_text_inside_figures(document, &swallowed);
    Ok(())
}

/// Removes short text that lives inside a figure.
///
/// Axis labels, legend entries and the text inside an architecture diagram are laid out for the
/// eye, not for reading, and they arrive as short lines set at odd sizes — which the classifier
/// duly reports as headings. They belong to the figure, which is now represented by its image
/// and caption, so emitting them again as prose is noise.
///
/// Only *short* blocks go, which is the guard that matters. A page of appendix figures can
/// produce a region covering two thirds of the page, and dropping everything inside it took a
/// measurable bite out of real prose — a 0.03 fall in bigram recall on BERT. A running paragraph
/// that lands inside a figure region is far more likely to be a detection error than a label.
/// Captions and figures are exempt outright, since a caption often overlaps what it describes.
fn drop_text_inside_figures(document: &mut doc::Document, regions: &[(usize, ir::Rect)]) {
    const MIN_CONTAINED: f32 = 0.8;
    const MAX_WORDS: usize = 12;

    document.blocks.retain(|block| {
        if matches!(block.kind, doc::BlockKind::Caption | doc::BlockKind::Figure) {
            return true;
        }
        if block.text.split_whitespace().count() > MAX_WORDS {
            return true;
        }
        let area = (block.bbox.width() * block.bbox.height()).max(f32::EPSILON);
        !regions.iter().any(|(page, region)| {
            *page == block.page && {
                let overlap =
                    region.x_overlap(&block.bbox).max(0.0) * region.y_overlap(&block.bbox).max(0.0);
                overlap / area >= MIN_CONTAINED
            }
        })
    });
}

/// The caption block belonging to a figure region: nearest vertically, and overlapping it
/// horizontally, since a caption sits directly under or over its figure and shares its column.
fn caption_for(document: &doc::Document, page: usize, region: &ir::Rect) -> Option<usize> {
    const MAX_CAPTION_DISTANCE: f32 = 90.0;

    document
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            b.page == page && b.kind == doc::BlockKind::Caption && b.bbox.x_overlap(region) > 0.0
        })
        .map(|(i, b)| {
            let gap = if b.bbox.y0 >= region.y1 {
                b.bbox.y0 - region.y1
            } else {
                region.y0 - b.bbox.y1
            };
            (i, gap)
        })
        .filter(|(_, gap)| *gap <= MAX_CAPTION_DISTANCE)
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| i)
}

/// Inserts an uncaptioned figure at its position in reading order.
fn insert_figure(document: &mut doc::Document, page: usize, bbox: ir::Rect, asset: Option<String>) {
    let block = doc::Block {
        kind: doc::BlockKind::Figure,
        text: String::new(),
        page,
        bbox,
        size: 0.0,
        asset,
        table: None,
        math: None,
        reference: None,
    };
    let at = document
        .blocks
        .iter()
        .position(|b| b.page > page || (b.page == page && b.bbox.y0 > bbox.y1))
        .unwrap_or(document.blocks.len());
    document.blocks.insert(at, block);
}
