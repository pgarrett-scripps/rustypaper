//! Finding the gutters between columns.
//!
//! A gutter is a vertical band that running text avoids. It is found from a coverage profile
//! rather than by assuming two columns, so single-column preprints, two-column conference
//! templates and the occasional three-column layout all work without a mode switch.
//!
//! Full-width elements — the title block, wide figures, tables spanning the page — *do* cross
//! the gutter, so the test is relative: a gutter is where coverage collapses well below the
//! page's typical coverage, not where it reaches zero.
//!
//! The profile is built from **glyph** extents, not line extents. Lines are the tempting choice
//! and are wrong: both columns of a paper usually sit on the same baseline grid, so before the
//! gutter is known a "line" is often one row of *both* columns and therefore spans the gutter
//! itself. Measured on the corpus, profiling by line found the gutter on 2 of 12 pages of a
//! plainly two-column paper. Glyphs cannot straddle a gutter, so they cannot hide one.

use crate::ir::{PageRaw, Rect};
use crate::text::lines::Line;

/// Width of a profile bin, in points. Fine enough to place a gutter edge accurately, coarse
/// enough that a page is only a few hundred bins.
const BIN: f32 = 1.0;

/// A band counts as a gutter when its coverage is at most this fraction of the page's peak.
///
/// Set generously because full-width elements legitimately cross the gutter: on a figure-heavy
/// page the title, wide figures and their captions can contribute a fifth of the ink found in a
/// column interior. A single column never dips this far below its own peak, so raising the bar
/// costs no false negatives.
const MAX_GUTTER_COVERAGE: f32 = 0.25;

/// Narrower than this and it is inter-word space or a ragged right edge, not a gutter.
const MIN_GUTTER_WIDTH: f32 = 9.0;

/// Text on both sides of a gutter must reach at least this fraction of the page's peak coverage.
/// Sparse regions inside a large figure produce wide low-coverage bands that are not gutters.
const MIN_FLANK_COVERAGE: f32 = 0.5;

/// How far either side of a band to look for that flanking text, in points.
const FLANK_WINDOW: f32 = 24.0;

/// More bands than this and the page is not multi-column text.
///
/// Three or more gutters means four or more columns, which papers do not use. What produces them
/// is a wide table or a figure whose labels leave regular vertical whitespace — and splitting
/// lines on those bands scrambles the very content it is trying to organise. Reporting none
/// leaves the page as a single region, which the reading-order pass already handles.
const MAX_GUTTERS: usize = 2;

/// A page with less ink than this has no reliable profile to read.
const MIN_GLYPHS_FOR_PROFILE: usize = 200;

/// Finds the gutters of a page whose lines have already been built.
///
/// Only glyphs that survived into `lines` are counted, which keeps sideways margin stamps out of
/// the profile.
pub fn page_gutters(page: &PageRaw, lines: &[Line]) -> Vec<(f32, f32)> {
    let boxes: Vec<Rect> = lines
        .iter()
        .flat_map(|line| line.glyphs.iter())
        .map(|placed| page.glyphs[placed.index].bbox)
        .collect();
    gutters(page.width, &boxes)
}

/// Finds gutters, as `(start_x, end_x)` pairs in left-to-right order.
///
/// Returns empty for single-column pages.
pub fn gutters(page_width: f32, glyph_boxes: &[Rect]) -> Vec<(f32, f32)> {
    if glyph_boxes.len() < MIN_GLYPHS_FOR_PROFILE || page_width <= 0.0 {
        return Vec::new();
    }

    let bins = (page_width / BIN).ceil() as usize;
    if bins == 0 {
        return Vec::new();
    }
    let mut coverage = vec![0u32; bins];

    for bbox in glyph_boxes {
        let lo = ((bbox.x0 / BIN).floor().max(0.0) as usize).min(bins);
        let hi = ((bbox.x1 / BIN).ceil().max(0.0) as usize).min(bins).max(lo);
        for slot in &mut coverage[lo..hi] {
            *slot += 1;
        }
    }

    let peak = coverage.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    let threshold = (peak as f32 * MAX_GUTTER_COVERAGE).floor() as u32;

    // The left and right margins are also low-coverage, so start from the text block's own
    // extent: only bands *between* text count as gutters.
    let first = coverage.iter().position(|&c| c > threshold);
    let last = coverage.iter().rposition(|&c| c > threshold);
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut run_start: Option<usize> = None;

    for i in first..=last {
        let low = coverage[i] <= threshold;
        match (low, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(start)) => {
                push_gutter(&mut out, &coverage, peak, start, i);
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = run_start {
        push_gutter(&mut out, &coverage, peak, start, last + 1);
    }

    if out.len() > MAX_GUTTERS {
        return Vec::new();
    }
    out
}

fn push_gutter(out: &mut Vec<(f32, f32)>, coverage: &[u32], peak: u32, start: usize, end: usize) {
    let (x0, x1) = (start as f32 * BIN, end as f32 * BIN);
    if x1 - x0 < MIN_GUTTER_WIDTH {
        return;
    }
    if !is_flanked_by_text(coverage, peak, start, end) {
        return;
    }
    out.push((x0, x1));
}

/// True when dense text sits immediately on both sides of the band.
///
/// Without this, the sparse interior of a large figure reads as several wide gutters — one page
/// of the corpus reported four — and splitting lines on them scrambles the figure's labels into
/// phantom columns.
fn is_flanked_by_text(coverage: &[u32], peak: u32, start: usize, end: usize) -> bool {
    let window = (FLANK_WINDOW / BIN) as usize;
    let required = (peak as f32 * MIN_FLANK_COVERAGE) as u32;

    let left = coverage[start.saturating_sub(window)..start]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    let right = coverage[end.min(coverage.len())..(end + window).min(coverage.len())]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);

    left >= required && right >= required
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;

    /// Lays out a row of 5pt glyph boxes across `x0..x1`.
    fn row(boxes: &mut Vec<Rect>, baseline: f32, x0: f32, x1: f32) {
        let mut x = x0;
        while x + 5.0 <= x1 {
            boxes.push(Rect::from_corners(x, baseline - 7.0, x + 5.0, baseline));
            x += 5.0;
        }
    }

    /// Two columns, 72..288 and 320..540, gutter between 288 and 320.
    fn two_column_page() -> Vec<Rect> {
        let mut boxes = Vec::new();
        for i in 0..25 {
            let y = 100.0 + i as f32 * 12.0;
            row(&mut boxes, y, 72.0, 288.0);
            row(&mut boxes, y, 320.0, 540.0);
        }
        boxes
    }

    #[test]
    fn finds_the_gutter_of_a_two_column_page() {
        let found = gutters(612.0, &two_column_page());
        assert_eq!(found.len(), 1, "expected exactly one gutter, got {found:?}");
        let (x0, x1) = found[0];
        assert!(x0 >= 285.0 && x1 <= 322.0, "gutter at {x0}..{x1}");
    }

    /// The regression that motivated profiling by glyph: when both columns share a baseline
    /// grid, every body *line* spans the gutter, so a line-based profile cannot see it.
    #[test]
    fn a_shared_baseline_grid_does_not_hide_the_gutter() {
        let boxes = two_column_page();
        // Every row here is one line spanning 72..540 as far as line building is concerned.
        assert!(!gutters(612.0, &boxes).is_empty());
    }

    #[test]
    fn full_width_elements_do_not_hide_the_gutter() {
        let mut boxes = two_column_page();
        // A title and a wide figure caption, both crossing the gutter.
        for y in [60.0, 80.0, 420.0, 440.0] {
            row(&mut boxes, y, 72.0, 540.0);
        }
        let found = gutters(612.0, &boxes);
        assert_eq!(
            found.len(),
            1,
            "full-width rows masked the gutter: {found:?}"
        );
    }

    #[test]
    fn single_column_pages_have_no_gutter() {
        let mut boxes = Vec::new();
        for i in 0..30 {
            row(&mut boxes, 100.0 + i as f32 * 12.0, 72.0, 540.0);
        }
        assert!(gutters(612.0, &boxes).is_empty());
    }

    #[test]
    fn margins_are_not_gutters() {
        let mut boxes = Vec::new();
        for i in 0..30 {
            row(&mut boxes, 100.0 + i as f32 * 12.0, 200.0, 400.0);
        }
        assert!(gutters(612.0, &boxes).is_empty());
    }

    #[test]
    fn three_columns_yield_two_gutters() {
        let mut boxes = Vec::new();
        for i in 0..25 {
            let y = 100.0 + i as f32 * 12.0;
            row(&mut boxes, y, 40.0, 190.0);
            row(&mut boxes, y, 220.0, 370.0);
            row(&mut boxes, y, 400.0, 550.0);
        }
        assert_eq!(gutters(612.0, &boxes).len(), 2);
    }

    #[test]
    fn a_page_of_vertical_bands_is_not_treated_as_columns() {
        // Five narrow blocks: a wide table, not a five-column page.
        let mut boxes = Vec::new();
        for i in 0..25 {
            let y = 100.0 + i as f32 * 12.0;
            for c in 0..5 {
                let x = 60.0 + c as f32 * 100.0;
                row(&mut boxes, y, x, x + 70.0);
            }
        }
        assert!(
            gutters(612.0, &boxes).is_empty(),
            "four gutters should be rejected outright"
        );
    }

    #[test]
    fn a_nearly_empty_page_is_left_alone() {
        let mut boxes = Vec::new();
        row(&mut boxes, 100.0, 72.0, 288.0);
        row(&mut boxes, 100.0, 320.0, 540.0);
        assert!(gutters(612.0, &boxes).is_empty());
    }
}
