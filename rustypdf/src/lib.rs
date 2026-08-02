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
pub mod error;
pub mod ir;
pub mod text;

pub use error::{Error, Result};

use std::path::Path;

use backend::pdfium::PdfiumBackend;
use backend::PageSource;
use ir::{DocRaw, FontTable};

/// Extracts every page's primitives from a PDF.
///
/// Fails with [`Error::Scanned`] when the document is image-only, which this pipeline does not
/// handle: there are no glyphs to reconstruct structure from. A document is only rejected when
/// *most* pages are scanned, so a paper with one scanned appendix page still converts.
pub fn extract(path: impl AsRef<Path>) -> Result<DocRaw> {
    let path = path.as_ref();
    let backend = PdfiumBackend::open(path)?;

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
