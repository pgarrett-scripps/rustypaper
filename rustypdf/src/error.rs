use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The pdfium shared library could not be located or loaded.
    #[error(
        "could not load pdfium: {0}\n\
         hint: run `scripts/fetch-pdfium.sh`, or set PDFIUM_DYNAMIC_LIB_PATH to the directory \
         containing libpdfium.so"
    )]
    PdfiumUnavailable(String),

    #[error("pdfium could not open the document: {0}")]
    PdfOpen(String),

    #[error("pdfium failed while reading page {page}: {source}")]
    PdfPage {
        page: usize,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Deliberate limitation: this pipeline reconstructs structure from real glyph data and has
    /// nothing to work with on an image-only page.
    #[error(
        "{path} looks like a scanned document ({scanned}/{total} pages have no extractable \
         text); rustypdf handles born-digital PDFs only"
    )]
    Scanned {
        path: PathBuf,
        scanned: usize,
        total: usize,
    },

    #[error("page {0} is out of range")]
    PageOutOfRange(usize),
}
