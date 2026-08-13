//! The conversion pipeline: PDF in, [`Document`](crate::doc::Document) out.
//!
//! The order of the passes is load-bearing and is documented at each step. The shape is: read
//! primitives, recover lines, decide the page's geometry, lift out everything that is not prose,
//! then assemble what remains.

use std::path::Path;

use rayon::prelude::*;

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
    extract_from(&crate::backend::open(path)?, path)
}

fn extract_from(backend: &impl PageSource, path: &Path) -> Result<DocRaw> {
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
    let backend = crate::backend::open(path)?;
    let raw = extract_from(&backend, path)?;
    let heights: Vec<f32> = raw.pages.iter().map(|p| p.height).collect();

    // Everything from here to assembly runs across pages at once. Ingest above it does not, and
    // ingest is where the time goes: 93% of wall time across the corpus, because the parallel
    // stages disappear into the same clock. `cargo run --release -p rustypaper --example
    // ingest_share` prints the split. The reader is `Sync`, so parallelising it is available
    // and is the one change here with real headroom left in it.
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
    // `lines_at` follows the prose through both lifts: for each line still standing, where it
    // sat in its page's ordered line stream. That index is what puts a lifted table or equation
    // back in the right place — see [`place_lifted`].
    let (tables, mut prose, mut lines_at) = lift_tables(&raw, ordered, stats);
    let equations = lift_equations(&raw, &mut prose, &mut lines_at);

    let vocab = text::vocab::Vocabulary::build(&prose);
    let (mut document, starts) = doc::assemble_placed(&prose, &heights, stats, &vocab);

    place_lifted(&mut document, &starts, &lines_at, tables, equations);
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

/// Lifts tables out of the line stream, returning them, the prose that remains, and where each
/// remaining line sat in the stream.
fn lift_tables(
    raw: &DocRaw,
    ordered: Vec<Vec<text::lines::Line>>,
    stats: layout::stats::Stats,
) -> (
    Vec<PlacedTable>,
    Vec<Vec<text::lines::Line>>,
    Vec<Vec<usize>>,
) {
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
                // The first line the table swallowed is where it belongs in reading order.
                let at = table.consumed.iter().copied().min().unwrap_or(lines.len());
                tables.push((
                    page.index,
                    at,
                    table.bbox,
                    doc::TableData {
                        rows: table.rows.clone(),
                        header_rows: table.header_rows,
                    },
                ));
            }

            let mut prose = Vec::with_capacity(lines.len());
            let mut at = Vec::with_capacity(lines.len());
            for (i, line) in lines.into_iter().enumerate() {
                if !consumed[i] {
                    prose.push(line);
                    at.push(i);
                }
            }
            (tables, prose, at)
        })
        .collect();

    let mut tables = Vec::new();
    let mut prose = Vec::with_capacity(per_page.len());
    let mut lines_at = Vec::with_capacity(per_page.len());
    for (found, lines, at) in per_page {
        tables.extend(found);
        prose.push(lines);
        lines_at.push(at);
    }
    (tables, prose, lines_at)
}

/// Lifts display equations out of the prose, and folds inline formulae into the lines that
/// carry them so that everything downstream sees `$x^2$` as one word.
fn lift_equations(
    raw: &DocRaw,
    prose: &mut [Vec<text::lines::Line>],
    lines_at: &mut [Vec<usize>],
) -> Vec<PlacedEquation> {
    raw.pages
        .par_iter()
        .zip(prose.par_iter_mut())
        .zip(lines_at.par_iter_mut())
        .flat_map(|((page, lines), at)| {
            let extent = text_extent(lines);
            // Display maths is centred *in its column*, so on a two-column page the page-wide
            // text extent is the wrong yardstick: an equation centred in the left column sits
            // far left of the pair, and reads as not centred at all. Every unnumbered display
            // equation in a two-column paper was being rejected on that basis.
            let gutters = layout::columns::page_gutters(page, lines);
            let mut is_display = vec![false; lines.len()];
            let mut found_here = Vec::new();

            for i in 0..lines.len() {
                let column = column_of(lines[i].bbox, extent, &gutters);
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
                    at.get(i).copied().unwrap_or(i),
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

            // The lines and their places in the stream are dropped in step, so what remains
            // still says where it came from.
            let mut keep = is_display.iter().map(|d| !d);
            lines.retain(|_| keep.next().unwrap_or(true));
            let mut keep = is_display.iter().map(|d| !d);
            at.retain(|_| keep.next().unwrap_or(true));
            found_here
        })
        .collect()
}

/// A table, with the page, the line of that page's stream, and the place it was lifted from.
type PlacedTable = (usize, usize, ir::Rect, doc::TableData);

/// A display equation, with the page, the line of that page's stream, and the place it was
/// lifted from.
type PlacedEquation = (usize, usize, ir::Rect, doc::MathData);

/// The tables lifted off one page, the prose that remains on it, and where in the page's line
/// stream each remaining line sat.
type PageTables = (Vec<PlacedTable>, Vec<text::lines::Line>, Vec<usize>);

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

/// The column a line sits in, given the page's text extent and its gutters.
///
/// Gutters split the extent into bands; the line belongs to the band its horizontal midpoint
/// falls in. A line that spans a gutter — a full-width title, a wide table — belongs to the
/// whole extent, which is the honest answer for something that is not in a column at all.
fn column_of(line: ir::Rect, extent: ir::Rect, gutters: &[(f32, f32)]) -> ir::Rect {
    if gutters.is_empty() {
        return extent;
    }
    // A line crossing any gutter is not inside a single column.
    if gutters
        .iter()
        .any(|&(start, end)| line.x0 < start && line.x1 > end)
    {
        return extent;
    }

    let mid = (line.x0 + line.x1) * 0.5;
    let mut left = extent.x0;
    let mut right = extent.x1;
    for &(start, end) in gutters {
        if end <= mid {
            left = left.max(end);
        } else if start >= mid {
            right = right.min(start);
            break;
        }
    }
    if right <= left {
        return extent;
    }
    ir::Rect {
        x0: left,
        y0: extent.y0,
        x1: right,
        y1: extent.y1,
    }
}

/// The horizontal extent of a page's text, used as the column when the page has no gutters.
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

/// Puts the lifted tables and equations back into the document.
///
/// Each one is placed by the line it was lifted from, not by where it sits on the page.
/// `document.blocks` is in *reading* order, and reading order is not a function of y: in a
/// two-column paper a left-column paragraph at y=300 comes before a right-column equation at
/// y=100, so "before the first block that starts below this one" put a right-column equation
/// into the middle of the left column, half a page from where it belonged. `starts` says which
/// line of its page each block began at, and `lines_at` translates that back into the stream as
/// it was before anything was lifted out of it — which is the same numbering the lifted items
/// carry, so the two merge.
fn place_lifted(
    document: &mut doc::Document,
    starts: &[usize],
    lines_at: &[Vec<usize>],
    tables: Vec<PlacedTable>,
    equations: Vec<PlacedEquation>,
) {
    let mut lifted: Vec<((usize, usize), doc::Block)> =
        Vec::with_capacity(tables.len() + equations.len());
    for (page, line, bbox, data) in tables {
        let mut block = doc::Block::new(doc::BlockKind::Table, page, bbox);
        block.table = Some(data);
        lifted.push(((page, line), block));
    }
    for (page, line, bbox, math) in equations {
        let mut block = doc::Block::new(doc::BlockKind::Equation, page, bbox);
        block.math = Some(math);
        lifted.push(((page, line), block));
    }
    lifted.sort_by_key(|(key, _)| *key);

    // Every block in the same numbering, so the two sequences can simply be merged. Both are
    // sorted: blocks run page by page and, within a page, down the line stream. A block whose
    // origin cannot be recovered takes no lifted item ahead of itself.
    let keys: Vec<(usize, usize)> = document
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let line = starts
                .get(index)
                .and_then(|&start| lines_at.get(block.page)?.get(start))
                .copied()
                .unwrap_or(usize::MAX);
            (block.page, line)
        })
        .collect();

    let mut out = Vec::with_capacity(document.blocks.len() + lifted.len());
    let mut lifted = lifted.into_iter().peekable();
    for (block, key) in document.blocks.drain(..).zip(keys) {
        while lifted.peek().is_some_and(|(at, _)| *at <= key) {
            out.push(lifted.next().expect("just peeked").1);
        }
        out.push(block);
    }
    out.extend(lifted.map(|(_, block)| block));

    document.blocks = out;
}

/// Renders a picture of any equation the reconstruction was not confident about.
///
/// The alternative is emitting LaTeX that looks authoritative and is wrong, which is worse than
/// an image: a reader can check an image, and a downstream tool will not silently ingest a
/// mangled formula as fact.
fn crop_uncertain_equations(
    backend: &impl PageSource,
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

/// Binds detected figure regions to their captions, optionally rasterising them.
fn attach_figures(
    backend: &impl PageSource,
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
/// So are tables and equations, and for a sharper reason: their content is in `block.table` and
/// `block.math`, not in `block.text`, so the word count that protects real prose reads zero for
/// them and the guard inverts — a numbered display equation beside an inset figure, cells and
/// LaTeX and all, was silently deleted for being "short".
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
        if matches!(
            block.kind,
            doc::BlockKind::Caption
                | doc::BlockKind::Figure
                | doc::BlockKind::Table
                | doc::BlockKind::Equation
        ) {
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

/// Inserts an uncaptioned figure at its place in reading order.
///
/// A figure region is drawn, not written, so unlike a table or an equation it has no line of the
/// stream to be placed by and geometry is all there is. Geometry still has to be read in the
/// document's own terms: `blocks` is in reading order, so the figure goes *after* the last block
/// that shares its column and sits above it, rather than *before* the first block that happens
/// to start lower down the page — on a two-column page that first block is usually still in the
/// left column, and a right-column figure would land half a page early.
fn insert_figure(document: &mut doc::Document, page: usize, bbox: ir::Rect, asset: Option<String>) {
    let mut block = doc::Block::new(doc::BlockKind::Figure, page, bbox);
    block.asset = asset;

    let above = document
        .blocks
        .iter()
        .rposition(|b| b.page == page && b.bbox.y1 <= bbox.y0 && b.bbox.x_overlap(&bbox) > 0.0);
    let at = match above {
        Some(index) => index + 1,
        // Nothing above it in its column: it opens its page, or the page has no text at all.
        None => document
            .blocks
            .iter()
            .position(|b| b.page >= page)
            .unwrap_or(document.blocks.len()),
    };
    document.blocks.insert(at, block);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> ir::Rect {
        ir::Rect::from_corners(x0, y0, x1, y1)
    }

    fn paragraph(page: usize, bbox: ir::Rect, text: &str) -> doc::Block {
        doc::Block::new(doc::BlockKind::Paragraph, page, bbox).with_text(text)
    }

    fn maths(latex: &str) -> doc::MathData {
        doc::MathData {
            latex: latex.to_owned(),
            number: None,
            confidence: 1.0,
        }
    }

    fn cells() -> doc::TableData {
        doc::TableData {
            rows: vec![vec!["a".to_owned(), "b".to_owned()]],
            header_rows: 1,
        }
    }

    /// One two-column page. Reading order runs down the left column (lines 0–3) and then down
    /// the right (lines 4–8), and line 4 — the top of the right column — was lifted out.
    fn two_columns() -> (doc::Document, Vec<usize>, Vec<Vec<usize>>) {
        let document = doc::Document {
            title: None,
            blocks: vec![
                paragraph(0, rect(50.0, 100.0, 250.0, 200.0), "left top"),
                paragraph(0, rect(50.0, 250.0, 250.0, 400.0), "left bottom"),
                paragraph(0, rect(320.0, 130.0, 520.0, 200.0), "right top"),
                paragraph(0, rect(320.0, 250.0, 520.0, 400.0), "right bottom"),
            ],
        };
        // Blocks begin at prose lines 0, 2, 4 and 6; the prose that remains is every line but 4.
        (
            document,
            vec![0, 2, 4, 6],
            vec![vec![0, 1, 2, 3, 5, 6, 7, 8]],
        )
    }

    /// The regression this placement exists for: an equation at the *top* of the right column
    /// sits above the left column's second paragraph, so "before the first block that starts
    /// below it" puts it in the middle of the left column, half a page early.
    #[test]
    fn equation_returns_to_its_own_column() {
        let (mut document, starts, lines_at) = two_columns();
        let equation = (0, 4, rect(330.0, 100.0, 500.0, 120.0), maths("x^2"));

        place_lifted(
            &mut document,
            &starts,
            &lines_at,
            Vec::new(),
            vec![equation],
        );

        let text: Vec<&str> = document.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            text,
            ["left top", "left bottom", "", "right top", "right bottom"]
        );
        assert_eq!(document.blocks[2].kind, doc::BlockKind::Equation);
    }

    /// Two items lifted from the same page keep the order the page had them in, whatever their
    /// geometry says.
    #[test]
    fn lifted_items_keep_their_order() {
        let (mut document, starts, lines_at) = two_columns();
        let equation = (0, 4, rect(330.0, 100.0, 500.0, 120.0), maths("x^2"));
        let table = (0, 1, rect(50.0, 150.0, 250.0, 190.0), cells());

        place_lifted(
            &mut document,
            &starts,
            &lines_at,
            vec![table],
            vec![equation],
        );

        let kinds: Vec<doc::BlockKind> = document.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                doc::BlockKind::Paragraph,
                doc::BlockKind::Table,
                doc::BlockKind::Paragraph,
                doc::BlockKind::Equation,
                doc::BlockKind::Paragraph,
                doc::BlockKind::Paragraph,
            ]
        );
    }

    /// A page whose every line was lifted has no block to be placed before, and the item still
    /// belongs before the pages that follow it.
    #[test]
    fn lifted_item_from_an_empty_page_keeps_its_page() {
        let mut document = doc::Document {
            title: None,
            blocks: vec![
                paragraph(0, rect(50.0, 100.0, 250.0, 200.0), "page one"),
                paragraph(2, rect(50.0, 100.0, 250.0, 200.0), "page three"),
            ],
        };
        let equation = (1, 0, rect(50.0, 100.0, 250.0, 120.0), maths("y"));

        place_lifted(
            &mut document,
            &[0, 0],
            &[vec![0], Vec::new(), vec![0]],
            Vec::new(),
            vec![equation],
        );

        let pages: Vec<usize> = document.blocks.iter().map(|b| b.page).collect();
        assert_eq!(pages, [0, 1, 2]);
        assert_eq!(document.blocks[1].kind, doc::BlockKind::Equation);
    }

    /// An uncaptioned figure has no line to be placed by, so it follows the last block above it
    /// *in its own column* — not the first block that starts lower down the page.
    #[test]
    fn uncaptioned_figure_lands_in_its_own_column() {
        let (mut document, _, _) = two_columns();

        insert_figure(&mut document, 0, rect(320.0, 210.0, 520.0, 240.0), None);

        let text: Vec<&str> = document.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            text,
            ["left top", "left bottom", "right top", "", "right bottom"]
        );
        assert_eq!(document.blocks[3].kind, doc::BlockKind::Figure);
    }

    /// A table or an equation carries its content in `table` and `math`, never in `text`, so the
    /// word count that keeps real prose out of a figure region reads zero for them.
    #[test]
    fn figures_do_not_swallow_equations_and_tables() {
        let region = rect(300.0, 100.0, 540.0, 400.0);
        let mut equation = doc::Block::new(
            doc::BlockKind::Equation,
            0,
            rect(320.0, 150.0, 520.0, 180.0),
        );
        equation.math = Some(maths("x^2"));
        let mut table = doc::Block::new(doc::BlockKind::Table, 0, rect(320.0, 200.0, 520.0, 260.0));
        table.table = Some(cells());
        let mut document = doc::Document {
            title: None,
            blocks: vec![
                equation,
                table,
                paragraph(0, rect(320.0, 300.0, 520.0, 320.0), "axis label"),
            ],
        };

        drop_text_inside_figures(&mut document, &[(0, region)]);

        let kinds: Vec<doc::BlockKind> = document.blocks.iter().map(|b| b.kind.clone()).collect();
        assert_eq!(kinds, [doc::BlockKind::Equation, doc::BlockKind::Table]);
    }
}
