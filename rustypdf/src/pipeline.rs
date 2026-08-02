//! The conversion pipeline: PDF in, [`Document`](crate::doc::Document) out.
//!
//! The order of the passes is load-bearing and is documented at each step. The shape is: read
//! primitives, recover lines, decide the page's geometry, lift out everything that is not prose,
//! then assemble what remains.

use std::path::Path;

use rayon::prelude::*;

use crate::backend::pdfium::PdfiumBackend;
use crate::backend::PageSource;
use crate::error::{Error, Result};
use crate::ir::{DocRaw, FontTable};
use crate::{doc, figure, ir, layout, math, refs, table, text};

/// Options for [`convert_with`].
#[derive(Debug, Clone)]
pub struct Options {
    /// Where to write extracted figures. Figures are still detected when this is `None`; only
    /// the image files are skipped.
    pub assets: Option<std::path::PathBuf>,
    /// Resolution to rasterise figures at.
    pub figure_dpi: f32,
    /// Strip grammatical scaffolding from prose — see [`crate::compress`].
    pub caveman: Option<crate::compress::Level>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            assets: None,
            figure_dpi: 150.0,
            caveman: None,
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
    let heights: Vec<f32> = raw.pages.iter().map(|p| p.height).collect();

    // Everything from here to assembly is pure Rust, so it runs across pages at once. Ingest
    // cannot: pdfium is single-threaded and serialised behind a lock inside the backend.
    let mut pages = build_pages(&raw);
    layout::furniture::strip(&mut pages, &heights);

    // Measured after furniture removal so running heads cannot skew the body size, and before
    // reading order because neither depends on the other.
    let stats = layout::stats::Stats::measure(&pages);

    let ordered: Vec<Vec<text::lines::Line>> = pages
        .into_par_iter()
        .map(layout::order::reading_order)
        .collect();

    // Everything that is not prose is lifted out before assembly, so that a table's cells and an
    // equation's symbols can never be mistaken for paragraphs. What is left on each page is
    // prose, with inline formulae folded into it as single words.
    let (tables, mut prose) = lift_tables(&raw, ordered, stats);
    let equations = lift_equations(&raw, &mut prose);

    let vocab = text::vocab::Vocabulary::build(&prose);
    let mut document = doc::assemble(&prose, &heights, stats, &vocab);

    insert_tables(&mut document, tables);
    insert_equations(&mut document, equations);
    attach_figures(&backend, &raw, &mut document, options)?;
    crop_uncertain_equations(&backend, &mut document, options)?;
    extract_bibliography(&mut document);

    // Last, so that every structural pass has seen the prose as written.
    if let Some(level) = options.caveman {
        crate::compress::compress(&mut document, level);
    }

    Ok(document)
}

/// Recovers each page's lines, split at its column gutters.
fn build_pages(raw: &DocRaw) -> Vec<Vec<text::lines::Line>> {
    raw.pages
        .par_iter()
        .map(|page| {
            let lines = text::lines::build_lines(page);
            // Gutters are measured before splitting, because a line spanning two columns is
            // exactly what the coverage profile needs to see in order to discount it.
            let gutters = layout::columns::page_gutters(page, &lines);
            text::lines::split_at_gutters(page, lines, &gutters)
        })
        .collect()
}

/// Lifts tables out of the line stream, returning them and the prose that remains.
fn lift_tables(
    raw: &DocRaw,
    ordered: Vec<Vec<text::lines::Line>>,
    stats: layout::stats::Stats,
) -> (Vec<PlacedTable>, Vec<Vec<text::lines::Line>>) {
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

    let mut tables = Vec::new();
    let mut prose = Vec::with_capacity(per_page.len());
    for (found, lines) in per_page {
        tables.extend(found);
        prose.push(lines);
    }
    (tables, prose)
}

/// Lifts display equations out of the prose, and folds inline formulae into the lines that
/// carry them so that everything downstream sees `$x^2$` as one word.
fn lift_equations(raw: &DocRaw, prose: &mut [Vec<text::lines::Line>]) -> Vec<PlacedEquation> {
    raw.pages
        .par_iter()
        .zip(prose.par_iter_mut())
        .flat_map(|(page, lines)| {
            let column = text_extent(lines);
            let mut is_display = vec![false; lines.len()];
            let mut found_here = Vec::new();

            for i in 0..lines.len() {
                let Some(found) = math::display(page, &raw.fonts, lines, i, column) else {
                    continue;
                };
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

            for (i, display) in is_display.iter().enumerate() {
                if !display {
                    apply_inline_math(page, &raw.fonts, lines, i);
                }
            }

            let mut keep = is_display.iter().map(|d| !d);
            lines.retain(|_| keep.next().unwrap_or(true));
            found_here
        })
        .collect()
}

/// A table, with the page and place it was lifted from.
type PlacedTable = (usize, ir::Rect, doc::TableData);

/// A display equation, with the page and place it was lifted from.
type PlacedEquation = (usize, ir::Rect, doc::MathData);

/// The tables lifted off one page, and the prose that remains on it.
type PageTables = (Vec<PlacedTable>, Vec<text::lines::Line>);

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

    // The whole bibliography is split at once, not block by block.
    //
    // Paragraph assembly gives it back as one block per column, and entry numbering runs
    // *across* those blocks — ResNet's first column ends at [27] and its second begins at [28].
    // Splitting each block independently restarts the sequence and finds nothing in the second,
    // which cost more than half of that paper's entries.
    let mut joined = String::new();
    let mut origins: Vec<(usize, &doc::Block)> = Vec::new();
    for (n, block) in document.blocks[first..end].iter().enumerate() {
        if block.kind != doc::BlockKind::Paragraph {
            continue;
        }
        let body = if n == 0 {
            &block.text[strip.min(block.text.len())..]
        } else {
            &block.text
        };
        if !joined.is_empty() {
            joined.push(' ');
        }
        origins.push((joined.len(), block));
        joined.push_str(body);
    }

    // Where there are no labels there is nothing to split on, and the joined text is one giant
    // entry. Falling back to one entry per column block is arbitrary, but a bibliography in
    // column-sized pieces is more use to a caller than a single undifferentiated blob.
    let split = refs::split_entries(&joined);
    let split = if split.len() > 1 {
        split
    } else {
        origins
            .iter()
            .map(|(_, b)| b.text.trim().to_owned())
            .filter(|t| !t.is_empty())
            .collect()
    };

    let mut entries: Vec<doc::Block> = Vec::new();
    let mut cursor = 0usize;
    for text in split {
        // Attribute the entry to whichever block it started in, so page numbers stay right.
        let at = joined[cursor..]
            .find(&text)
            .map(|i| cursor + i)
            .unwrap_or(cursor);
        cursor = at + text.len();
        let source = origins
            .iter()
            .rev()
            .find(|(offset, _)| *offset <= at)
            .map(|(_, b)| *b);

        let parsed = refs::parse(&text);
        let (page, bbox, size) = match source {
            Some(b) => (b.page, b.bbox, b.size),
            None => (0, ir::Rect::from_corners(0.0, 0.0, 0.0, 0.0), 0.0),
        };
        let mut entry = doc::Block::new(doc::BlockKind::Reference, page, bbox)
            .with_text(text)
            .with_size(size);
        entry.reference = Some(parsed);
        entries.push(entry);
    }

    // Anything that was not a paragraph keeps its place.
    for block in &document.blocks[first..end] {
        if block.kind != doc::BlockKind::Paragraph {
            entries.push(block.clone());
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
/// Inserts a block at its position in reading order.
///
/// Tables, equations and uncaptioned figures are all lifted out of the line stream before
/// assembly and have to be put back where they belong: before the first block that starts below
/// them, or on a later page.
fn insert_in_reading_order(document: &mut doc::Document, block: doc::Block) {
    let at = document
        .blocks
        .iter()
        .position(|b| b.page > block.page || (b.page == block.page && b.bbox.y0 > block.bbox.y1))
        .unwrap_or(document.blocks.len());
    document.blocks.insert(at, block);
}

/// Places display equations into the document.
fn insert_equations(document: &mut doc::Document, equations: Vec<PlacedEquation>) {
    for (page, bbox, math) in equations {
        let mut block = doc::Block::new(doc::BlockKind::Equation, page, bbox);
        block.math = Some(math);
        insert_in_reading_order(document, block);
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

/// Places reconstructed tables into the document.
fn insert_tables(document: &mut doc::Document, tables: Vec<PlacedTable>) {
    for (page, bbox, data) in tables {
        let mut block = doc::Block::new(doc::BlockKind::Table, page, bbox);
        block.table = Some(data);
        insert_in_reading_order(document, block);
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
    /// How far outside a figure its own labels can sit, in points.
    ///
    /// A plot's axis labels are *outside* the plotted area by construction — below the x-axis,
    /// left of the y-axis — and the region is built from the artwork, so containment alone never
    /// catches them. On a physics paper whose figures are small insets this left a hundred
    /// fragments like `z z` and `Kt=Kx+Ky y` scattered through the prose.
    const MARGIN: f32 = 18.0;

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
                let padded = ir::Rect {
                    x0: region.x0 - MARGIN,
                    y0: region.y0 - MARGIN,
                    x1: region.x1 + MARGIN,
                    y1: region.y1 + MARGIN,
                };
                // The block's centre, not just its area. A label that straddles the boundary —
                // which is exactly where an axis label sits — never reaches an area threshold
                // however far the region is padded.
                let (cx, cy) = (block.bbox.center_x(), block.bbox.center_y());
                let centred =
                    cx >= padded.x0 && cx <= padded.x1 && cy >= padded.y0 && cy <= padded.y1;
                let overlap =
                    padded.x_overlap(&block.bbox).max(0.0) * padded.y_overlap(&block.bbox).max(0.0);
                centred || overlap / area >= MIN_CONTAINED
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

/// Inserts an uncaptioned figure.
fn insert_figure(document: &mut doc::Document, page: usize, bbox: ir::Rect, asset: Option<String>) {
    let mut block = doc::Block::new(doc::BlockKind::Figure, page, bbox);
    block.asset = asset;
    insert_in_reading_order(document, block);
}
