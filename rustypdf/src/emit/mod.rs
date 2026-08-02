//! Renderings of a [`crate::doc::Document`].
//!
//! Emitters are deliberately thin. Anything that requires judgement belongs in the pipeline, so
//! that every output format inherits it rather than reimplementing it.

pub mod markdown;
pub mod text;
pub mod typst;
