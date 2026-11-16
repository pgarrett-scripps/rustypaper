//! The boundary between "reading a PDF" and "understanding a paper".
//!
//! Everything above this trait is pure Rust operating on [`PageRaw`], produced by [`rustium`].
//! The trait is still worth having: the shape classification and page clipping that downstream
//! passes depend on live here rather than in the backend, so what the rest of the pipeline is
//! promised does not move if the reader underneath it ever does.

use crate::error::Result;
use crate::ir::{FontTable, PageRaw, PathKind, Rect};

pub mod rustium;

/// A source of page primitives.
///
/// Implementations are not required to be `Sync`, though [`rustium`]'s is, so ingest could be
/// parallelised alongside the stages above it. The pipeline does not depend on that yet.
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

/// The reader every page comes through.
pub type Backend = rustium::RustiumBackend;

/// Opens a PDF with the compiled-in backend.
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
/// the page, so that is what gets recorded. The reader reports unclipped geometry, so this
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
/// Markdown — wants the letters. The backend expands these in its glyph-name fallback, so doing
/// it here keeps the expansion in one place, above whatever reads the page.
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

// ---- page-frame normalisation ------------------------------------------------------------------

/// How far off a quarter turn a glyph may sit and still count toward that quarter turn's bucket.
/// The same 5° that line building uses to call a glyph horizontal.
const CARDINAL_TOLERANCE_RADIANS: f32 = 0.0873;

/// Fewer sideways glyphs than this cannot turn a page: an arXiv margin stamp is ~40 glyphs but
/// always shares its page with a body of upright text, so it never outnumbers; this floor keeps
/// a nearly blank page (a stamp beside a lone page number) from being rotated on no evidence.
const MIN_DOMINANT_ROTATED: usize = 20;

/// Turns a page upright when its text is predominantly set at a quarter turn.
///
/// Two document shapes arrive this way: a landscape table drawn sideways on a portrait page
/// (`/Rotate 0`, every glyph at ±90°), and a page whose content is upright in user space but
/// carries a `/Rotate` the backend has applied, leaving every glyph angled in display space.
/// Downstream, line building drops any glyph more than 5° off horizontal — the right call for a
/// margin stamp interleaved with body text, and the wrong one when the sideways text *is* the
/// page, which then converts to nothing.
///
/// The page is only turned when one quarter-turn bucket strictly outnumbers the upright glyphs,
/// so a stamp or sideways table header on an ordinary page changes nothing. After the turn the
/// formerly upright minority is sideways and is dropped as a stamp would be: that minority is
/// page furniture on every page in evidence, and interleaving it would be the garbage the angle
/// filter exists to prevent.
pub(crate) fn normalize_rotation(page: &mut PageRaw) {
    use std::f32::consts::{FRAC_PI_2, PI};

    /// A point map in y-down page space: `(x, y, old_width, old_height)` to the turned point.
    type PointMap = fn(f32, f32, f32, f32) -> (f32, f32);

    // Bucket text by nearest quarter turn: 0 upright, 1 at +90°, 2 flipped, 3 at -90°.
    let mut counts = [0usize; 4];
    for glyph in page.glyphs.iter().filter(|g| !g.generated) {
        if !glyph.angle.is_finite() {
            continue;
        }
        let quarters = glyph.angle / FRAC_PI_2;
        if (quarters - quarters.round()).abs() * FRAC_PI_2 <= CARDINAL_TOLERANCE_RADIANS {
            counts[(quarters.round() as i32).rem_euclid(4) as usize] += 1;
        }
    }
    let (dominant, &count) = counts
        .iter()
        .enumerate()
        .skip(1)
        .max_by_key(|(_, &n)| n)
        .expect("three rotated buckets");
    if count <= counts[0] || count < MIN_DOMINANT_ROTATED {
        return;
    }

    // The frame turn that brings the dominant bucket to 0, as a point map in y-down page space
    // and the angle it adds to every direction. `w`/`h` are the extent *before* the turn.
    let (w, h) = (page.width, page.height);
    let (map, delta): (PointMap, f32) = match dominant {
        1 => (|x, y, w, _h| (y, w - x), -FRAC_PI_2),
        2 => (|x, y, w, h| (w - x, h - y), PI),
        _ => (|x, y, _w, h| (h - y, x), FRAC_PI_2),
    };
    let map_rect = |r: &Rect| {
        let (ax, ay) = map(r.x0, r.y0, w, h);
        let (bx, by) = map(r.x1, r.y1, w, h);
        Rect::from_corners(ax, ay, bx, by)
    };

    for glyph in &mut page.glyphs {
        let (x, y) = map(glyph.origin.x, glyph.origin.y, w, h);
        glyph.origin = crate::ir::Point::new(x, y);
        glyph.bbox = map_rect(&glyph.bbox);
        let turned = glyph.angle + delta;
        glyph.angle = if turned > PI {
            turned - 2.0 * PI
        } else if turned <= -PI {
            turned + 2.0 * PI
        } else {
            turned
        };
    }
    for path in &mut page.paths {
        path.bbox = map_rect(&path.bbox);
        if dominant != 2 {
            path.kind = match path.kind {
                PathKind::HorizontalRule => PathKind::VerticalRule,
                PathKind::VerticalRule => PathKind::HorizontalRule,
                other => other,
            };
        }
    }
    for image in &mut page.images {
        image.bbox = map_rect(&image.bbox);
    }
    if dominant != 2 {
        page.width = h;
        page.height = w;
    }
    page.turned = true;
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

    fn glyph_at(x: f32, y: f32, angle: f32) -> crate::ir::Glyph {
        crate::ir::Glyph {
            advance: 5.0,
            text: crate::ir::GlyphText::Char('a'),
            bbox: Rect::from_corners(x, y - 8.0, x + 5.0, y),
            origin: crate::ir::Point::new(x, y),
            font: crate::ir::FontId(0),
            size: 10.0,
            angle,
            flags: Default::default(),
            color: Default::default(),
            generated: false,
        }
    }

    fn sideways_page() -> PageRaw {
        // 30 glyphs reading bottom-to-top (a landscape table on a portrait page), 3 upright.
        let mut page = PageRaw {
            width: 600.0,
            height: 800.0,
            ..Default::default()
        };
        for i in 0..30 {
            page.glyphs.push(glyph_at(
                100.0,
                700.0 - 20.0 * i as f32,
                -std::f32::consts::FRAC_PI_2,
            ));
        }
        for i in 0..3 {
            page.glyphs
                .push(glyph_at(300.0 + 6.0 * i as f32, 790.0, 0.0));
        }
        page.paths.push(crate::ir::PathItem {
            bbox: Rect::from_corners(100.0, 100.0, 100.5, 700.0),
            kind: PathKind::VerticalRule,
            thickness: 0.5,
            color: Default::default(),
            filled: true,
            stroked: false,
        });
        page
    }

    #[test]
    fn a_predominantly_sideways_page_is_turned_upright() {
        let mut page = sideways_page();
        normalize_rotation(&mut page);

        // The frame is landscape now and the table text reads left to right.
        assert_eq!((page.width, page.height), (800.0, 600.0));
        assert!((page.glyphs[0].angle).abs() < 1e-4);
        // A point map check: origin (100, 700) under (x, y) -> (h - y, x) with h = 800.
        assert_eq!(
            (page.glyphs[0].origin.x, page.glyphs[0].origin.y),
            (100.0, 100.0)
        );
        // The table's column separator turned with it.
        assert_eq!(page.paths[0].kind, PathKind::HorizontalRule);
        // The upright minority is sideways now, where line building will set it aside.
        let last = page.glyphs.last().expect("glyphs survive");
        assert!((last.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn a_margin_stamp_does_not_turn_the_page() {
        // The usual arXiv shape: a body of upright text and a sideways stamp.
        let mut page = PageRaw {
            width: 600.0,
            height: 800.0,
            ..Default::default()
        };
        for i in 0..40 {
            page.glyphs
                .push(glyph_at(72.0 + 6.0 * i as f32, 400.0, 0.0));
        }
        for i in 0..25 {
            page.glyphs.push(glyph_at(
                20.0,
                700.0 - 10.0 * i as f32,
                std::f32::consts::FRAC_PI_2,
            ));
        }
        let before = page.clone();
        normalize_rotation(&mut page);
        assert_eq!(page.glyphs, before.glyphs);
        assert_eq!((page.width, page.height), (600.0, 800.0));
    }

    #[test]
    fn an_upside_down_page_keeps_its_extent() {
        let mut page = sideways_page();
        for glyph in &mut page.glyphs {
            glyph.angle = std::f32::consts::PI;
        }
        normalize_rotation(&mut page);
        assert_eq!((page.width, page.height), (600.0, 800.0));
        assert!(page.glyphs[0].angle.abs() < 1e-4);
        // (100, 700) under (x, y) -> (w - x, h - y).
        assert_eq!(
            (page.glyphs[0].origin.x, page.glyphs[0].origin.y),
            (500.0, 100.0)
        );
        // A half turn leaves a vertical rule vertical.
        assert_eq!(page.paths[0].kind, PathKind::VerticalRule);
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
