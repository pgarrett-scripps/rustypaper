//! The boundary between "reading a PDF" and "understanding a paper".
//!
//! Everything above this trait is pure Rust operating on [`PageRaw`], which keeps the native
//! pdfium dependency confined to one module and leaves room for a pure-Rust backend later
//! without disturbing the pipeline.

use crate::error::Result;
use crate::ir::{FontTable, PageRaw};

pub mod pdfium;

/// A source of page primitives.
///
/// Implementations are not required to be `Sync`: pdfium is not thread-safe, so ingest is
/// serialised and the expensive pure-Rust stages are what get parallelised.
pub trait PageSource {
    /// Number of pages in the document.
    fn page_count(&self) -> usize;

    /// Extracts one page's primitives, interning any new font names into `fonts`.
    fn page(&self, index: usize, fonts: &mut FontTable) -> Result<PageRaw>;

    /// Renders a region of a page to a PNG at the given resolution.
    ///
    /// Used for figure crops and as the fallback when math reconstruction is not confident
    /// enough to emit LaTeX.
    fn render_region(&self, index: usize, region: crate::ir::Rect, dpi: f32) -> Result<Vec<u8>>;
}
