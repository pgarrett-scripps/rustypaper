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
pub mod table;
pub mod layout;
pub mod text;

pub use error::{Error, Result};

use std::path::Path;

use backend::pdfium::PdfiumBackend;
use backend::PageSource;
use ir::{DocRaw, FontTable};

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

    let mut pages: Vec<Vec<text::lines::Line>> = Vec::with_capacity(raw.pages.len());
    for page in &raw.pages {
        let lines = text::lines::build_lines(page);
        // Gutters are measured before splitting, because a line spanning two columns is exactly
        // what the coverage profile needs to see in order to discount it.
        let gutters = layout::columns::page_gutters(page, &lines);
        pages.push(text::lines::split_at_gutters(page, lines, &gutters));
    }

    let heights: Vec<f32> = raw.pages.iter().map(|p| p.height).collect();
    layout::furniture::strip(&mut pages, &heights);

    // Measured after furniture removal so running heads cannot skew the body size, and before
    // reading order because neither depends on the other.
    let stats = layout::stats::Stats::measure(&pages);

    let ordered: Vec<Vec<text::lines::Line>> = pages
        .into_iter()
        .map(layout::order::reading_order)
        .collect();

    // The vocabulary is built from the whole document, because the evidence that `learn` and
    // `ing` are one word is usually `learning` written out on some other page entirely.
    let vocab = text::vocab::Vocabulary::build(&ordered);

    // Tables are lifted out before assembly so that their cells never become paragraphs. What
    // is left on each page is prose.
    let mut tables: Vec<(usize, ir::Rect, doc::TableData)> = Vec::new();
    let mut prose: Vec<Vec<text::lines::Line>> = Vec::with_capacity(ordered.len());

    for (page, lines) in raw.pages.iter().zip(ordered) {
        let found = table::detect(page, &lines, stats.body_size);
        let mut consumed = vec![false; lines.len()];
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
        prose.push(
            lines
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !consumed[*i])
                .map(|(_, line)| line)
                .collect(),
        );
    }

    let mut document = doc::assemble(&prose, &heights, stats, &vocab);
    insert_tables(&mut document, tables);
    attach_figures(&backend, &raw, &mut document, options)?;

    Ok(document)
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
                    std::fs::write(&file, png).map_err(|source| Error::Io {
                        path: file,
                        source,
                    })?;
                    Some(format!("{}/{name}", dir.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("assets")))
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
                let overlap = region.x_overlap(&block.bbox).max(0.0)
                    * region.y_overlap(&block.bbox).max(0.0);
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
    };
    let at = document
        .blocks
        .iter()
        .position(|b| b.page > page || (b.page == page && b.bbox.y0 > bbox.y1))
        .unwrap_or(document.blocks.len());
    document.blocks.insert(at, block);
}
