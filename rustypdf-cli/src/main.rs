use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rustypdf::backend::pdfium::PdfiumBackend;
use rustypdf::backend::PageSource;
use rustypdf::ir::{FontTable, PathKind};
use rustypdf::text::lines::build_lines;

#[derive(Parser)]
#[command(
    name = "rp2m",
    version,
    about = "Convert scientific PDFs to structured text"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Md => "md",
            Format::Json => "json",
            Format::Typst => "typ",
            Format::Text => "txt",
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    /// GitHub-flavoured Markdown.
    Md,
    /// The document model, which is the real output; every other format renders this.
    Json,
    /// Typst, which models figures, tables and references natively.
    Typst,
    /// Plain text, for indexing.
    Text,
}

#[derive(Subcommand)]
enum Command {
    /// Dump extracted page primitives as JSON.
    Dump {
        pdf: PathBuf,
        /// Restrict output to a single zero-based page.
        #[arg(short, long)]
        page: Option<usize>,
        #[arg(long)]
        pretty: bool,
    },
    /// Summarise what the backend sees, for eyeballing extraction quality.
    Probe {
        pdf: PathBuf,
        /// Show a per-page breakdown rather than just document totals.
        #[arg(long)]
        pages: bool,
    },
    /// Convert one or more PDFs to structured text.
    Convert {
        #[arg(required = true)]
        pdf: Vec<PathBuf>,
        /// Output format.
        #[arg(short, long, default_value = "md")]
        format: Format,
        /// Write to a file instead of stdout. With several inputs, a directory.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Extract figures into this directory and reference them from the output.
        #[arg(long)]
        assets: Option<PathBuf>,
        /// Resolution to rasterise figures at.
        #[arg(long, default_value_t = 150.0)]
        figure_dpi: f32,
        /// Strip articles, copulas and stock academic phrases from the prose, for feeding to a
        /// model that charges by the token. Maths, tables and citations are left alone.
        #[arg(long)]
        caveman: bool,
    },
    /// Print reconstructed lines, one per output line. Reading order is not applied yet.
    Text {
        pdf: PathBuf,
        /// Restrict output to a single zero-based page.
        #[arg(short, long)]
        page: Option<usize>,
        /// Prefix each line with its baseline, x-range and dominant font size.
        #[arg(long)]
        geometry: bool,
    },
}

fn main() -> Result<()> {
    let mut out = BufWriter::new(std::io::stdout().lock());

    let result = match Cli::parse().command {
        Command::Dump { pdf, page, pretty } => dump(&mut out, pdf, page, pretty),
        Command::Convert {
            pdf,
            format,
            out: dest,
            assets,
            figure_dpi,
            caveman,
        } => convert(&mut out, pdf, format, dest, assets, figure_dpi, caveman),
        Command::Probe { pdf, pages } => probe(&mut out, pdf, pages),
        Command::Text {
            pdf,
            page,
            geometry,
        } => text(&mut out, pdf, page, geometry),
    };

    // Piping into `head` closes stdout early. That is normal shell usage, not a failure, so it
    // must not surface as a panic or a non-zero exit.
    match result.and_then(|()| out.flush().map_err(Into::into)) {
        Err(e) if is_broken_pipe(&e) => Ok(()),
        other => other,
    }
}

fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .any(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
}

fn dump(out: &mut impl Write, pdf: PathBuf, page: Option<usize>, pretty: bool) -> Result<()> {
    match page {
        Some(index) => {
            let backend = PdfiumBackend::open(&pdf)?;
            let mut fonts = FontTable::new();
            let raw = backend.page(index, &mut fonts)?;
            write_json(out, &(fonts, raw), pretty)
        }
        None => {
            let doc =
                rustypdf::extract(&pdf).with_context(|| format!("extracting {}", pdf.display()))?;
            write_json(out, &doc, pretty)
        }
    }
}

fn write_json(mut w: impl Write, value: &impl serde::Serialize, pretty: bool) -> Result<()> {
    if pretty {
        serde_json::to_writer_pretty(&mut w, value)?;
    } else {
        serde_json::to_writer(&mut w, value)?;
    }
    writeln!(w)?;
    Ok(())
}

fn probe(out: &mut impl Write, pdf: PathBuf, per_page: bool) -> Result<()> {
    let started = Instant::now();
    let doc = rustypdf::extract(&pdf).with_context(|| format!("extracting {}", pdf.display()))?;
    let elapsed = started.elapsed();

    let pages = doc.pages.len();
    let glyphs: usize = doc.pages.iter().map(|p| p.glyphs.len()).sum();
    let images: usize = doc.pages.iter().map(|p| p.images.len()).sum();

    let mut kinds = (0usize, 0usize, 0usize, 0usize);
    for path in doc.pages.iter().flat_map(|p| &p.paths) {
        match path.kind {
            PathKind::HorizontalRule => kinds.0 += 1,
            PathKind::VerticalRule => kinds.1 += 1,
            PathKind::Box => kinds.2 += 1,
            PathKind::Other => kinds.3 += 1,
        }
    }

    writeln!(out, "{}", pdf.display())?;
    writeln!(
        out,
        "  {pages} pages, {glyphs} glyphs, {images} images in {:.0} ms ({:.1} ms/page)",
        elapsed.as_secs_f64() * 1000.0,
        elapsed.as_secs_f64() * 1000.0 / pages.max(1) as f64,
    )?;
    writeln!(
        out,
        "  paths: {} h-rules, {} v-rules, {} boxes, {} other",
        kinds.0, kinds.1, kinds.2, kinds.3
    )?;

    // Font usage is the first thing to check when text comes out wrong: a symbolic TeX math
    // font where prose is expected means the Unicode repair pass has work to do.
    let mut by_font: BTreeMap<&str, usize> = BTreeMap::new();
    for page in &doc.pages {
        for glyph in &page.glyphs {
            *by_font.entry(doc.fonts.name(glyph.font)).or_default() += 1;
        }
    }
    let mut ranked: Vec<_> = by_font.into_iter().collect();
    ranked.sort_by_key(|(_, n)| std::cmp::Reverse(*n));

    writeln!(out, "  fonts ({} distinct):", doc.fonts.len())?;
    for (name, count) in ranked.iter().take(12) {
        let pct = *count as f64 * 100.0 / glyphs.max(1) as f64;
        writeln!(out, "    {count:>7}  {pct:>5.1}%  {name}")?;
    }
    if ranked.len() > 12 {
        writeln!(out, "    ... and {} more", ranked.len() - 12)?;
    }

    if per_page {
        writeln!(out, "  per page:")?;
        for page in &doc.pages {
            writeln!(
                out,
                "    {:>3}: {:>5} glyphs  {:>3} paths  {:>2} images  {:.0}x{:.0}pt rot={}  {} lines  gutters={:?}",
                page.index,
                page.glyphs.len(),
                page.paths.len(),
                page.images.len(),
                page.width,
                page.height,
                page.rotation,
                {
                    let lines = build_lines(page);
                    lines.len()
                },
                {
                    let lines = build_lines(page);
                    rustypdf::layout::columns::page_gutters(page, &lines)
                        .iter()
                        .map(|(a, b)| (a.round() as i32, b.round() as i32))
                        .collect::<Vec<_>>()
                },
            )?;
            for region in rustypdf::figure::regions(page) {
                let pct =
                    100.0 * region.bbox.width() * region.bbox.height() / (page.width * page.height);
                writeln!(
                    out,
                    "         figure {:.0},{:.0}..{:.0},{:.0}  {:.0}% of page  {} images {} paths",
                    region.bbox.x0,
                    region.bbox.y0,
                    region.bbox.x1,
                    region.bbox.y1,
                    pct,
                    region.images,
                    region.paths
                )?;
            }
        }
    }

    Ok(())
}

fn text(out: &mut impl Write, pdf: PathBuf, only: Option<usize>, geometry: bool) -> Result<()> {
    let doc = rustypdf::extract(&pdf).with_context(|| format!("extracting {}", pdf.display()))?;

    for page in &doc.pages {
        if only.is_some_and(|p| p != page.index) {
            continue;
        }
        writeln!(out, "--- page {} ---", page.index)?;
        for line in build_lines(page) {
            if geometry {
                writeln!(
                    out,
                    "[y={:7.2} x={:6.1}..{:6.1} size={:5.2}] {}",
                    line.baseline,
                    line.bbox.x0,
                    line.bbox.x1,
                    line.size,
                    line.text()
                )?;
            } else {
                writeln!(out, "{}", line.text())?;
            }
        }
    }
    Ok(())
}

fn convert(
    out: &mut impl Write,
    pdfs: Vec<PathBuf>,
    format: Format,
    dest: Option<PathBuf>,
    assets: Option<PathBuf>,
    figure_dpi: f32,
    caveman: bool,
) -> Result<()> {
    let batch = pdfs.len() > 1;
    if batch && dest.is_none() {
        anyhow::bail!("converting several files needs --out <directory>");
    }
    if batch {
        if let Some(dir) = &dest {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
    }

    // Sequential on purpose. Each conversion already parallelises its own pure-Rust stages, and
    // ingest is serialised behind pdfium's lock whatever the caller does, so converting several
    // documents at once in one process buys nothing and multiplies peak memory. Shard across
    // processes to scale out.
    let mut failed = 0;
    for pdf in &pdfs {
        let stem = pdf
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let assets = assets.clone().or_else(|| {
            if batch {
                dest.as_ref().map(|d| d.join(format!("{stem}_assets")))
            } else {
                None
            }
        });

        let options = rustypdf::Options {
            assets,
            figure_dpi,
            caveman,
        };
        let doc = match rustypdf::convert_with(pdf, &options) {
            Ok(doc) => doc,
            // One unreadable file must not abandon the rest of a batch.
            Err(e) if batch => {
                eprintln!("{}: {e}", pdf.display());
                failed += 1;
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("converting {}", pdf.display())),
        };

        let rendered = match format {
            Format::Md => rustypdf::emit::markdown::render(&doc),
            Format::Json => serde_json::to_string_pretty(&doc)? + "\n",
            Format::Typst => rustypdf::emit::typst::render(&doc),
            Format::Text => rustypdf::emit::text::render(&doc),
        };

        match &dest {
            Some(path) if batch => {
                let file = path.join(format!("{stem}.{}", format.extension()));
                std::fs::write(&file, rendered)
                    .with_context(|| format!("writing {}", file.display()))?;
            }
            Some(path) => std::fs::write(path, rendered)
                .with_context(|| format!("writing {}", path.display()))?,
            None => out.write_all(rendered.as_bytes())?,
        }
    }

    if failed > 0 {
        anyhow::bail!("{failed} of {} files failed", pdfs.len());
    }
    Ok(())
}
