//! The boundary between "reading a PDF" and "understanding a paper".
//!
//! Everything above this trait is pure Rust operating on [`PageRaw`]. Two backends implement it:
//! [`rustium`], a pure-Rust PDF interpreter, and [`pdfium`], the Chromium engine behind FFI.
//! Which one is compiled in is a feature choice; nothing above this module can tell the
//! difference, because the shape classification and page clipping that downstream passes depend
//! on live here rather than in either backend.

use crate::error::Result;
use crate::ir::{FontTable, PageRaw, PathKind, Rect};

#[cfg(feature = "pdfium")]
pub mod pdfium;
#[cfg(feature = "rustium")]
pub mod rustium;

/// A source of page primitives.
///
/// Implementations are not required to be `Sync`: pdfium is not thread-safe, so ingest is
/// serialised and the expensive pure-Rust stages are what get parallelised. The rustium backend
/// *is* `Sync`, but the pipeline does not yet depend on that.
pub trait PageSource {
    /// Number of pages in the document.
    fn page_count(&self) -> usize;

    /// Extracts one page's primitives, interning any new font names into `fonts`.
    fn page(&self, index: usize, fonts: &mut FontTable) -> Result<PageRaw>;

    /// Renders a region of a page to a PNG at the given resolution.
    ///
    /// Used for figure crops and as the fallback when math reconstruction is not confident
    /// enough to emit LaTeX.
    fn render_region(&self, index: usize, region: Rect, dpi: f32) -> Result<Vec<u8>>;
}

/// The backend selected at compile time.
///
/// rustium wins the tie when both are enabled: it is the one that keeps the build pure Rust, so
/// pdfium has to be asked for deliberately rather than fallen into.
#[cfg(feature = "rustium")]
pub type Backend = rustium::RustiumBackend;
#[cfg(all(feature = "pdfium", not(feature = "rustium")))]
pub type Backend = pdfium::PdfiumBackend;

/// Opens a PDF with the compiled-in backend.
#[cfg(any(feature = "rustium", feature = "pdfium"))]
pub fn open(path: impl AsRef<std::path::Path>) -> Result<Backend> {
    Backend::open(path)
}

// ---- shape classification, shared by every backend --------------------------------------------
//
// These live here, not in a backend, because they define what downstream passes are promised.
// Two backends that classified rules differently would make table detection depend on which one
// was compiled in, which is exactly the coupling the `PageSource` trait exists to prevent.

/// A vector path at most this thick (in points) is a candidate rule rather than a filled shape.
/// `\toprule` is ~0.8pt and a fraction bar ~0.4pt; anything above 3pt is a drawn box edge.
pub(crate) const MAX_RULE_THICKNESS: f32 = 3.0;

/// A rule must be at least this many times longer than it is thick, which keeps small filled
/// squares (list bullets, plot markers) out of the rule sets.
pub(crate) const MIN_RULE_ASPECT: f32 = 3.0;

/// Tolerance, in points, for calling two coordinates equal when testing for a rectangle.
pub(crate) const RECT_EPSILON: f32 = 0.01;

/// Classifies a path by the shape of its bounding box, returning the rule thickness alongside.
pub(crate) fn classify_path(bbox: &Rect) -> (PathKind, f32) {
    let (w, h) = (bbox.width(), bbox.height());

    if h <= MAX_RULE_THICKNESS && w >= h * MIN_RULE_ASPECT {
        (PathKind::HorizontalRule, h)
    } else if w <= MAX_RULE_THICKNESS && h >= w * MIN_RULE_ASPECT {
        (PathKind::VerticalRule, w)
    } else {
        (PathKind::Other, w.min(h))
    }
}

/// Intersects an object's bounds with the page, returning `None` when nothing is left visible.
///
/// A clipped path reports the bounds of its *unclipped* geometry, which can run tens of
/// thousands of points off-page. Taking that at face value let a single object drag a figure
/// region to 6000% of the page and swallow every block on it. What is visible is what falls on
/// the page, so that is what gets recorded. Both backends report unclipped geometry, so both
/// need this.
pub(crate) fn clip_to_page(bbox: Rect, width: f32, height: f32) -> Option<Rect> {
    if !bbox.x0.is_finite() || !bbox.y0.is_finite() || !bbox.x1.is_finite() || !bbox.y1.is_finite()
    {
        return None;
    }
    let clipped = Rect {
        x0: bbox.x0.max(0.0),
        y0: bbox.y0.max(0.0),
        x1: bbox.x1.min(width),
        y1: bbox.y1.min(height),
    };
    (clipped.width() > 0.0 && clipped.height() > 0.0).then_some(clipped)
}

/// Expands a Latin typographic ligature into the letters it stands for.
///
/// TeX sets `fi` as one glyph, and a PDF that maps it faithfully hands back U+FB01. Every
/// consumer downstream — de-hyphenation, the vocabulary, bigram scoring, search over the
/// Markdown — wants the letters. pdfium expands these in its own glyph-name fallback, so doing
/// it here is what makes the two backends agree rather than a preference of ours.
///
/// Returns `None` for the overwhelmingly common case of a character that is not a ligature.
pub(crate) fn expand_ligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' => "st", // long s + t
        '\u{FB06}' => "st",
        _ => return None,
    })
}

/// Effective font size in points, with fallbacks.
///
/// A backend can report a scaled size of 0 for text whose matrix is a pure rotation — the arXiv
/// stamp down the margin of every preprint is the case that surfaced this. Size is load-bearing
/// everywhere downstream (baseline tolerance, word gaps, heading detection), so the invariant
/// that it is positive and finite is established here rather than defended in every consumer.
pub(crate) fn resolve_size(reported: f32, bbox: &Rect, angle: f32) -> f32 {
    if reported.is_finite() && reported > 0.0 {
        return reported;
    }

    // Last resort: infer from the ink. Font size runs *across* the baseline, so for sideways
    // text that is the box's width, not its height — taking the larger of the two would report
    // the advance and make a margin stamp look like display type.
    let upright = (angle.to_degrees().rem_euclid(180.0) - 90.0).abs() >= 45.0;
    let extent = if upright { bbox.height() } else { bbox.width() };
    if extent.is_finite() && extent > 0.0 {
        // A cap-height glyph is roughly 0.7em.
        extent / 0.7
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_classification() {
        // A booktabs rule: long and 0.8pt thick.
        let rule = Rect::from_corners(72.0, 300.0, 540.0, 300.8);
        assert_eq!(classify_path(&rule).0, PathKind::HorizontalRule);

        // A tabular column separator.
        let vline = Rect::from_corners(300.0, 100.0, 300.5, 400.0);
        assert_eq!(classify_path(&vline).0, PathKind::VerticalRule);

        // A list bullet: thin, but not long enough to be a rule.
        let bullet = Rect::from_corners(72.0, 300.0, 74.0, 302.0);
        assert_eq!(classify_path(&bullet).0, PathKind::Other);

        // A framed figure.
        let boxy = Rect::from_corners(72.0, 100.0, 540.0, 400.0);
        assert_eq!(classify_path(&boxy).0, PathKind::Other);
    }

    #[test]
    fn fraction_bar_is_a_horizontal_rule() {
        // 12pt wide, 0.4pt thick: what `\frac` draws.
        let bar = Rect::from_corners(100.0, 200.0, 112.0, 200.4);
        let (kind, thickness) = classify_path(&bar);
        assert_eq!(kind, PathKind::HorizontalRule);
        assert!((thickness - 0.4).abs() < 1e-5);
    }

    #[test]
    fn off_page_geometry_is_clipped_to_the_page() {
        // What a clipped path reports: bounds far outside the page in both directions.
        let huge = Rect::from_corners(106.0, -38870.0, 492.0, 39239.0);
        let clipped = clip_to_page(huge, 595.0, 842.0).expect("some of it is on the page");
        assert_eq!((clipped.x0, clipped.y0), (106.0, 0.0));
        assert_eq!((clipped.x1, clipped.y1), (492.0, 842.0));

        // Entirely off-page geometry contributes nothing.
        assert_eq!(
            clip_to_page(
                Rect::from_corners(-500.0, -500.0, -100.0, -100.0),
                595.0,
                842.0
            ),
            None
        );
        // Degenerate coordinates must not produce a rect at all.
        assert_eq!(
            clip_to_page(Rect::from_corners(f32::NAN, 0.0, 10.0, 10.0), 595.0, 842.0),
            None
        );
    }

    #[test]
    fn size_falls_back_to_ink_when_unreported() {
        let bbox = Rect::from_corners(0.0, 0.0, 5.0, 7.0);
        // A reported size is taken as-is.
        assert_eq!(resolve_size(9.5, &bbox, 0.0), 9.5);
        // Upright text measures across the baseline: the box's height.
        assert!((resolve_size(0.0, &bbox, 0.0) - 10.0).abs() < 1e-5);
        // Sideways text measures the box's width instead, so a margin stamp is not display type.
        let sideways = std::f32::consts::FRAC_PI_2;
        assert!((resolve_size(0.0, &bbox, sideways) - 5.0 / 0.7).abs() < 1e-5);
    }
}
