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

/// Parses the `caveman` argument shared by the conversion entry points.
///
/// Spelled as a string rather than an enum because the caller is Python: an
/// enum would have to be constructed and imported, where the levels are
/// already named in the CLI and the docs. `None` and `"off"` both mean no
/// compression, so a caller can pass a config value straight through without
/// special-casing the disabled state.
fn caveman_level(caveman: Option<&str>) -> PyResult<Option<rustypdf::compress::Level>> {
    match caveman {
        None | Some("off") | Some("none") => Ok(None),
        Some("light") => Ok(Some(rustypdf::compress::Level::Light)),
        Some("hard") => Ok(Some(rustypdf::compress::Level::Hard)),
        Some(other) => Err(PyValueError::new_err(format!(
            "unknown caveman level {other:?}; expected \"off\", \"light\" or \"hard\""
        ))),
    }
}

/// Converts a PDF to Markdown.
///
/// `caveman` strips grammatical scaffolding for models that charge by the
/// token — see [`rustypdf::compress`] for what each level drops. It is exposed
/// here because the callers that care about token cost are the scripting ones,
/// which is the whole reason this binding exists.
#[pyfunction]
#[pyo3(signature = (path, caveman = None))]
fn to_markdown(py: Python<'_>, path: &str, caveman: Option<&str>) -> PyResult<String> {
    let options = rustypdf::Options {
        caveman: caveman_level(caveman)?,
        ..Default::default()
    };
    py.detach(|| {
        rustypdf::convert_with(path, &options)
            .map(|doc| rustypdf::emit::markdown::render(&doc))
            .map_err(to_py_err)
    })
}

/// Converts a PDF to the document model, as JSON.
///
/// The Python wrapper parses this into a dict. Passing JSON across the boundary keeps the
/// binding free of a serde-to-Python bridge, and the cost is irrelevant next to conversion.
///
/// Takes `caveman` for the same reason `to_markdown` does, and so that a
/// caller inspecting the document model sees the same text the Markdown
/// rendering would carry.
#[pyfunction]
#[pyo3(signature = (path, caveman = None))]
fn to_json(py: Python<'_>, path: &str, caveman: Option<&str>) -> PyResult<String> {
    let options = rustypdf::Options {
        caveman: caveman_level(caveman)?,
        ..Default::default()
    };
    py.detach(|| {
        let doc = rustypdf::convert_with(path, &options).map_err(to_py_err)?;
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
