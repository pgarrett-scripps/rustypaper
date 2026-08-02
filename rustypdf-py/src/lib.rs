//! Python bindings.
//!
//! The surface is deliberately small: convert a PDF, get Markdown or the document model. The
//! reason it exists is that everything *around* a converter — evaluation, corpus management,
//! comparison against other tools, notebook work — is scripting, and Rust is the wrong language
//! for that while being the right one for the per-glyph algorithms underneath.
//!
//! Conversion releases the GIL. pdfium itself is serialised by a lock inside `rustypdf`, so
//! calling from several Python threads is safe but not parallel; other Python threads do keep
//! running.

use pyo3::create_exception;
use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;

create_exception!(
    _rustypdf,
    ScannedDocument,
    PyValueError,
    "The PDF is image-only, which this pipeline does not handle."
);

fn to_py_err(error: rustypdf::Error) -> PyErr {
    match error {
        // Worth its own type: it is the one failure a caller is likely to want to branch on,
        // typically to route the document to an OCR pipeline instead.
        e @ rustypdf::Error::Scanned { .. } => ScannedDocument::new_err(e.to_string()),
        e @ rustypdf::Error::Io { .. } => PyIOError::new_err(e.to_string()),
        e => PyValueError::new_err(e.to_string()),
    }
}

/// Converts a PDF to Markdown.
#[pyfunction]
fn to_markdown(py: Python<'_>, path: &str) -> PyResult<String> {
    py.detach(|| {
        rustypdf::convert(path)
            .map(|doc| rustypdf::emit::markdown::render(&doc))
            .map_err(to_py_err)
    })
}

/// Converts a PDF to the document model, as JSON.
///
/// The Python wrapper parses this into a dict. Passing JSON across the boundary keeps the
/// binding free of a serde-to-Python bridge, and the cost is irrelevant next to conversion.
#[pyfunction]
fn to_json(py: Python<'_>, path: &str) -> PyResult<String> {
    py.detach(|| {
        let doc = rustypdf::convert(path).map_err(to_py_err)?;
        serde_json::to_string(&doc).map_err(|e| PyValueError::new_err(e.to_string()))
    })
}

/// Extracts page primitives without interpreting them, as JSON. For diagnostics.
#[pyfunction]
fn extract_json(py: Python<'_>, path: &str) -> PyResult<String> {
    py.detach(|| {
        let raw = rustypdf::extract(path).map_err(to_py_err)?;
        serde_json::to_string(&raw).map_err(|e| PyValueError::new_err(e.to_string()))
    })
}

#[pymodule]
fn _rustypdf(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add("ScannedDocument", module.py().get_type::<ScannedDocument>())?;
    module.add_function(wrap_pyfunction!(to_markdown, module)?)?;
    module.add_function(wrap_pyfunction!(to_json, module)?)?;
    module.add_function(wrap_pyfunction!(extract_json, module)?)?;
    Ok(())
}
