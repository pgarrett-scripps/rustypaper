//! Structure-aware conversion of born-digital scientific PDFs.
//!
//! The pipeline is a sequence of passes over an intermediate representation:
//!
//! ```text
//! PDF --[backend]--> PageRaw (glyphs, paths, images)
//!     --[text]-----> lines and words
//!     --[layout]---> columns, reading order, typed blocks
//!     --[math/table/refs]--> a Document
//!     --[emit]-----> Markdown / Typst / JSON / text
//! ```
//!
//! The `Document` is the real output; Markdown is one rendering of it.

pub mod backend;
pub mod compress;
pub mod doc;
pub mod emit;
pub mod error;
pub mod figure;
pub mod ir;
pub mod layout;
pub mod math;
pub mod pipeline;
pub mod refs;
pub mod table;
pub mod text;
pub mod util;

pub use error::{Error, Result};
pub use pipeline::{convert, convert_with, extract, Options};
