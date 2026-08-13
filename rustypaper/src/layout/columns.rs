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
//!
//! One page is not always enough to read. A wide table or a full-page figure fills the corridor
//! for most of the page's height and leaves the text below it with no profile of its own, so
//! [`document_bands`] asks the *document* what its columns are and then looks for them again
//! within horizontal slices of the pages that found nothing.

use rayon::prelude::*;

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

/// Text on both sides of a gutter must reach at least this fraction of the page's *typical* text
/// coverage. Sparse regions inside a large figure produce wide low-coverage bands that are not
/// gutters.
///
/// Typical, not peak. The two columns of a page are not equally dense — one of them holds the
/// algorithm float, the ragged appendix list, the half-empty last column of a section — and
/// measured against the densest bin on the page the sparse side of a perfectly real gutter never
/// qualifies. The page then reports no columns at all and every line of it interleaves, which is
/// a far worse failure than the phantom gutter this test exists to prevent. Two of `pinsage`'s
/// ten pages and four of `imagenet`'s went that way: a corridor 20pt wide with *zero* coverage,
/// rejected because the pseudo-code beside it reaches only a third of the ink of the body text
/// across the page.
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

/// How far above a band's own floor a bin may sit and still count as part of the empty corridor,
/// as a fraction of the page's peak coverage. A stray descender or a hyphen poking into the
/// gutter must not shorten it.
const CORE_TOLERANCE: f32 = 0.02;

/// How far a band's gutter may sit from the document's own gutter and still be the same one.
/// Columns do not move between pages; this is slack for ragged edges, not for a second geometry.
const SPINE_TOLERANCE: f32 = 12.0;

/// Pages agreeing on one gutter, below which a document has no settled column geometry to look
/// for. Two pages agreeing could be two pages with the same wide table.
const MIN_SPINE_PAGES: usize = 3;

/// How many times a page may be cut in half in search of a band with columns. Two cuts is four
/// bands, which is more structure than a page of a paper has.
const MAX_BAND_DEPTH: usize = 2;

/// A horizontal slice of a page, and the gutters that hold inside it.
///
/// Most pages are one band covering the whole page. A page is only sliced when it reports no
/// columns as a whole and the document says it should have some.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    /// Top of the slice. The first band on a page starts at negative infinity, so that every
    /// line lands in exactly one band whatever the page's furniture does.
    pub y0: f32,
    /// Bottom of the slice; the last band on a page ends at infinity.
    pub y1: f32,
    pub gutters: Vec<(f32, f32)>,
}

impl Band {
    /// The whole page as one band.
    pub fn whole(gutters: Vec<(f32, f32)>) -> Self {
        Self {
            y0: f32::NEG_INFINITY,
            y1: f32::INFINITY,
            gutters,
        }
    }

    pub fn contains(&self, y: f32) -> bool {
        y >= self.y0 && y < self.y1
    }
}

/// The gutters that apply to a line at height `y`.
pub fn gutters_at(bands: &[Band], y: f32) -> &[(f32, f32)] {
    bands
        .iter()
        .find(|band| band.contains(y))
        .map_or(&[][..], |band| &band.gutters)
}

/// Finds the gutters of a page whose lines have already been built.
///
/// Only glyphs that survived into `lines` are counted, which keeps sideways margin stamps out of
/// the profile.
pub fn page_gutters(page: &PageRaw, lines: &[Line]) -> Vec<(f32, f32)> {
    gutters(page.width, &glyph_boxes(page, lines.iter()))
}

fn glyph_boxes<'a>(page: &PageRaw, lines: impl Iterator<Item = &'a Line>) -> Vec<Rect> {
    lines
        .flat_map(|line| line.glyphs.iter())
        .map(|placed| page.glyphs[placed.index].bbox)
        .collect()
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

    let typical = typical_coverage(&coverage, threshold, first, last);

    let mut out = Vec::new();
    let mut run_start: Option<usize> = None;

    for i in first..=last {
        let low = coverage[i] <= threshold;
        match (low, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(start)) => {
                push_gutter(&mut out, &coverage, peak, typical, start, i);
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = run_start {
        push_gutter(&mut out, &coverage, peak, typical, start, last + 1);
    }

    if out.len() > MAX_GUTTERS {
        return Vec::new();
    }
    out
}

fn push_gutter(
    out: &mut Vec<(f32, f32)>,
    coverage: &[u32],
    peak: u32,
    typical: u32,
    start: usize,
    end: usize,
) {
    let (start, end) = empty_core(coverage, peak, start, end);
    let (x0, x1) = (start as f32 * BIN, end as f32 * BIN);
    if x1 - x0 < MIN_GUTTER_WIDTH {
        return;
    }
    if !is_flanked_by_text(coverage, typical, start, end) {
        return;
    }
    out.push((x0, x1));
}

/// The coverage of a bin that holds ordinary text, as the median over the bins that do.
///
/// The median rather than the peak, because the peak is whichever column happens to be densest
/// and is not what the other column has to live up to.
fn typical_coverage(coverage: &[u32], threshold: u32, first: usize, last: usize) -> u32 {
    let mut text: Vec<u32> = coverage[first..=last]
        .iter()
        .copied()
        .filter(|&c| c > threshold)
        .collect();
    if text.is_empty() {
        return 0;
    }
    text.sort_unstable();
    text[text.len() / 2]
}

/// Narrows a low-coverage band to the corridor within it that is actually empty.
///
/// The band is found by asking where coverage falls below a quarter of the page's peak, and a
/// bibliography defeats that: its hanging `[12]` labels sit in a column of their own at the very
/// edge of the text, and being present on only the first line of each entry they cover their bins
/// a fifth as often as running text does. The band therefore swallows the label column, its far
/// edge lands *inside* the right-hand column's ink, and the line splitter — which needs a pair of
/// glyphs straddling the whole band before it will cut — cannot cut the very lines that open an
/// entry. The two columns of the bibliography then arrive zipped together.
///
/// Trimming to the band's own floor puts the edges back where the white space really ends. Where
/// the floor is not zero, because a title or a wide figure crosses the gutter, every bin in the
/// band sits at it and the band is returned unchanged — which is what must happen, since a full
/// width line has to keep crossing. A core narrower than a gutter is not believed either, so a
/// word space inside a crossing line cannot become a cut.
fn empty_core(coverage: &[u32], peak: u32, start: usize, end: usize) -> (usize, usize) {
    let band = &coverage[start..end];
    let Some(&floor) = band.iter().min() else {
        return (start, end);
    };
    let ceiling = floor + (peak as f32 * CORE_TOLERANCE).round() as u32;

    // A sentinel past the end closes the last run without a special case.
    let (mut best, mut run) = ((0usize, 0usize), None);
    for (i, &bin) in band.iter().chain(std::iter::once(&u32::MAX)).enumerate() {
        match (bin <= ceiling, run) {
            (true, None) => run = Some(i),
            (false, Some(from)) => {
                if i - from > best.1 - best.0 {
                    best = (from, i);
                }
                run = None;
            }
            _ => {}
        }
    }

    if (best.1 - best.0) as f32 * BIN >= MIN_GUTTER_WIDTH {
        (start + best.0, start + best.1)
    } else {
        (start, end)
    }
}

/// Resolves every page's column structure, using the document to read the pages that cannot be
/// read on their own.
///
/// A page-wide profile fails on the pages where it is needed most. A wide table or a full-page
/// figure holds the corridor open — or fills it — for most of the page's height, and the columns
/// of running text above or below it are then a minority of the ink and leave no dip in the
/// profile deep enough to see. `medimaging.pdf` loses four pages that way, `resnet.pdf` one,
/// `imagenet.pdf` one; each of them then interleaves the text that *is* in two columns.
///
/// The fix has to be anchored, because looking for gutters inside arbitrary slices of a page
/// finds plenty that are not gutters: measured across the corpus, a naive band search invents a
/// column boundary in a table on eight pages of single-column papers — precisely where splitting
/// lines does the most damage. So the search is given the answer first. A document that sets two
/// columns sets them in the same place on every page, so the gutters agreed by the pages that
/// *could* be read become the document's spine, and a band is only allowed to report a gutter
/// that lands on it.
///
/// Reading a page's profile is per-page work and stays parallel, as it was when each page
/// answered for itself; only the vote between them is serial, and it is a handful of numbers.
pub fn document_bands(pages: &[PageRaw], lines: &[Vec<Line>]) -> Vec<Vec<Band>> {
    let per_page: Vec<Vec<(f32, f32)>> = pages
        .par_iter()
        .zip(lines.par_iter())
        .map(|(page, lines)| page_gutters(page, lines))
        .collect();

    let Some(spine) = document_spine(&per_page) else {
        return per_page.into_iter().map(|g| vec![Band::whole(g)]).collect();
    };

    pages
        .par_iter()
        .zip(lines.par_iter())
        .zip(per_page)
        .map(|((page, lines), found)| {
            if !found.is_empty() {
                return vec![Band::whole(found)];
            }
            let mut bands = Vec::new();
            let mut slice: Vec<&Line> = lines.iter().collect();
            slice.sort_by(|a, b| a.bbox.y0.total_cmp(&b.bbox.y0));
            search_bands(page, &slice, spine, 0, &mut bands);
            stitch(bands)
        })
        .collect()
}

/// The gutter this document uses, if its pages agree on one.
///
/// Only pages reporting a single gutter vote: a page with two is either three columns or a wide
/// table read as one, and neither is the spine of a paper.
fn document_spine(per_page: &[Vec<(f32, f32)>]) -> Option<(f32, f32)> {
    let mut votes: Vec<(f32, f32)> = per_page
        .iter()
        .filter(|g| g.len() == 1)
        .map(|g| g[0])
        .collect();
    if votes.len() < MIN_SPINE_PAGES {
        return None;
    }
    votes.sort_by(|a, b| center(*a).total_cmp(&center(*b)));
    let median = votes[votes.len() / 2];

    let agreeing = votes
        .iter()
        .filter(|&&g| (center(g) - center(median)).abs() <= SPINE_TOLERANCE)
        .count();
    (agreeing * 2 >= votes.len()).then_some(median)
}

fn center((x0, x1): (f32, f32)) -> f32 {
    (x0 + x1) * 0.5
}

/// Looks for the document's gutter inside a slice of a page, halving the slice when it is not
/// there.
fn search_bands(
    page: &PageRaw,
    slice: &[&Line],
    spine: (f32, f32),
    depth: usize,
    out: &mut Vec<Band>,
) {
    let extent = || {
        let y0 = slice
            .iter()
            .map(|l| l.bbox.y0)
            .fold(f32::INFINITY, f32::min);
        let y1 = slice
            .iter()
            .map(|l| l.bbox.y1)
            .fold(f32::NEG_INFINITY, f32::max);
        (y0, y1)
    };

    let found: Vec<(f32, f32)> = gutters(page.width, &glyph_boxes(page, slice.iter().copied()))
        .into_iter()
        .filter(|&g| (center(g) - center(spine)).abs() <= SPINE_TOLERANCE)
        .collect();

    if !found.is_empty() || depth >= MAX_BAND_DEPTH {
        let (y0, y1) = extent();
        out.push(Band {
            y0,
            y1,
            gutters: found,
        });
        return;
    }

    let Some(at) = widest_horizontal_gap(slice) else {
        let (y0, y1) = extent();
        out.push(Band {
            y0,
            y1,
            gutters: Vec::new(),
        });
        return;
    };
    search_bands(page, &slice[..at], spine, depth + 1, out);
    search_bands(page, &slice[at..], spine, depth + 1, out);
}

/// Where a slice of lines, already sorted by top edge, is most cleanly divided in two.
fn widest_horizontal_gap(slice: &[&Line]) -> Option<usize> {
    let mut best = (0.0f32, 0usize);
    let mut reach = f32::NEG_INFINITY;
    for (n, line) in slice.iter().enumerate() {
        if n > 0 && line.bbox.y0 - reach > best.0 {
            best = (line.bbox.y0 - reach, n);
        }
        reach = reach.max(line.bbox.y1);
    }
    (best.1 > 0 && best.0 > 0.0).then_some(best.1)
}

/// Merges neighbouring bands that agree, and stretches the result to cover the page.
///
/// A band's own extent is the ink it was measured from; the boundary between two bands belongs
/// halfway between them, so that a line just outside the ink — a caption, a page number the
/// furniture pass left behind — still lands in exactly one band.
fn stitch(mut bands: Vec<Band>) -> Vec<Band> {
    bands.dedup_by(|b, a| {
        (b.gutters == a.gutters)
            .then(|| a.y1 = a.y1.max(b.y1))
            .is_some()
    });
    for i in 1..bands.len() {
        let boundary = (bands[i - 1].y1 + bands[i].y0) * 0.5;
        bands[i - 1].y1 = boundary;
        bands[i].y0 = boundary;
    }
    if let Some(first) = bands.first_mut() {
        first.y0 = f32::NEG_INFINITY;
    }
    if let Some(last) = bands.last_mut() {
        last.y1 = f32::INFINITY;
    }
    if bands.is_empty() {
        bands.push(Band::whole(Vec::new()));
    }
    bands
}

/// True when dense text sits immediately on both sides of the band, and the band itself is a
/// collapse away from it.
///
/// Without the first half, the sparse interior of a large figure reads as several wide gutters —
/// one page of the corpus reported four — and splitting lines on them scrambles the figure's
/// labels into phantom columns.
///
/// Without the second half, so does the ragged right edge of a *single* column. Coverage there
/// decays over the last inch of the measure rather than stopping, and a page-wide threshold cuts
/// that slope somewhere: `adam.pdf` reported an 11pt gutter in its own right margin, where the
/// band held four fifths of the ink of the text beside it. A gutter is a collapse, so it is the
/// text beside the band, not the page's average, that the band has to be empty *by*.
fn is_flanked_by_text(coverage: &[u32], typical: u32, start: usize, end: usize) -> bool {
    let window = (FLANK_WINDOW / BIN) as usize;
    let required = (typical as f32 * MIN_FLANK_COVERAGE) as u32;

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

    let flank = left.min(right);
    let floor = coverage[start..end].iter().copied().min().unwrap_or(0);
    flank >= required && floor.saturating_mul(2) <= flank
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FontFlags, FontId, Glyph, GlyphText, Point, Rect, Rgba};

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

    /// One column of a page is routinely sparser at its inner edge than the other — a ragged
    /// appendix list, an algorithm float — and the gutter beside it is still a gutter.
    ///
    /// Every line starts at the margin, so the margin is the page's densest bin by a long way;
    /// measured against *that* peak the sparse side never qualifies, the page reports no columns
    /// at all, and every line of it interleaves. This is `imagenet.pdf`'s appendix in miniature.
    #[test]
    fn a_sparse_column_beside_a_dense_one_still_has_a_gutter() {
        let mut boxes = Vec::new();
        for i in 0..40 {
            let y = 100.0 + i as f32 * 8.0;
            // A list: every entry starts at the margin, two in five run the full measure.
            row(&mut boxes, y, 72.0, if i % 5 < 2 { 288.0 } else { 150.0 });
        }
        for i in 0..25 {
            row(&mut boxes, 100.0 + i as f32 * 12.0, 320.0, 540.0);
        }
        let found = gutters(612.0, &boxes);
        assert_eq!(found.len(), 1, "sparse column lost its gutter: {found:?}");
    }

    /// A bibliography hangs its `[12]` labels in a column of their own at the very edge of the
    /// text, and they appear on only the first line of each entry. The band must not swallow
    /// them: its far edge has to land where the entries start, or the line splitter — which
    /// wants a pair of glyphs straddling the whole band — cannot cut the lines that open one.
    #[test]
    fn a_hanging_label_column_is_not_swallowed_by_the_gutter() {
        let mut boxes = Vec::new();
        for i in 0..30 {
            let y = 100.0 + i as f32 * 10.0;
            row(&mut boxes, y, 72.0, 288.0);
            // Every sixth line opens an entry, with its label hanging out to 300.
            if i % 6 == 0 {
                row(&mut boxes, y, 300.0, 315.0);
            }
            row(&mut boxes, y, 320.0, 540.0);
        }
        let found = gutters(612.0, &boxes);
        assert_eq!(found.len(), 1, "expected one gutter, got {found:?}");
        let (x0, x1) = found[0];
        assert!(
            x1 <= 300.0,
            "the gutter reaches {x1}, past the labels at 300"
        );
        assert!(x0 >= 286.0, "the gutter starts at {x0}, inside the text");
    }

    /// The other half of that bargain: where a full-width line crosses the gutter, the band's
    /// floor is that line's ink and there is no emptier core to trim to. Trimming to a word
    /// space inside the crossing line would let the splitter cut the line in half.
    #[test]
    fn a_crossing_line_does_not_shrink_the_gutter_to_a_word_space() {
        let mut boxes = two_column_page();
        for y in [60.0, 72.0, 84.0] {
            // A title crossing the gutter, with a word space sitting in the middle of it.
            row(&mut boxes, y, 72.0, 300.0);
            row(&mut boxes, y, 305.0, 540.0);
        }
        let found = gutters(612.0, &boxes);
        assert_eq!(found.len(), 1, "expected one gutter, got {found:?}");
        let (x0, x1) = found[0];
        assert!(
            x1 - x0 >= 20.0,
            "gutter shrank to {x0}..{x1}, the crossing line's word space"
        );
    }

    #[test]
    fn a_nearly_empty_page_is_left_alone() {
        let mut boxes = Vec::new();
        row(&mut boxes, 100.0, 72.0, 288.0);
        row(&mut boxes, 100.0, 320.0, 540.0);
        assert!(gutters(612.0, &boxes).is_empty());
    }

    // -----------------------------------------------------------------------------------------
    // Whole-document column geometry.
    // -----------------------------------------------------------------------------------------

    /// Lays out a row of 5pt glyphs across `x0..x1`, as a page would report them.
    fn glyph_row(glyphs: &mut Vec<Glyph>, baseline: f32, x0: f32, x1: f32) {
        let mut x = x0;
        while x + 5.0 <= x1 {
            glyphs.push(Glyph {
                text: GlyphText::Char('x'),
                bbox: Rect::from_corners(x, baseline - 7.0, x + 5.0, baseline),
                origin: Point::new(x, baseline),
                font: FontId(0),
                size: 10.0,
                angle: 0.0,
                flags: FontFlags::default(),
                color: Rgba::BLACK,
                generated: false,
            });
            x += 5.0;
        }
    }

    fn page_of(index: usize, glyphs: Vec<Glyph>) -> PageRaw {
        PageRaw {
            index,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            glyphs,
            ..Default::default()
        }
    }

    /// An ordinary two-column page: 25 rows of both columns.
    fn two_column(index: usize) -> PageRaw {
        let mut glyphs = Vec::new();
        for i in 0..25 {
            let y = 100.0 + i as f32 * 12.0;
            glyph_row(&mut glyphs, y, 72.0, 288.0);
            glyph_row(&mut glyphs, y, 320.0, 540.0);
        }
        page_of(index, glyphs)
    }

    /// A page whose top two thirds is a table spanning the measure, with two columns of running
    /// text below it. The table's rows cross the corridor on every baseline, so the page-wide
    /// profile sees no gutter at all.
    fn table_over_two_columns(index: usize) -> PageRaw {
        let mut glyphs = Vec::new();
        for i in 0..20 {
            glyph_row(&mut glyphs, 100.0 + i as f32 * 12.0, 72.0, 540.0);
        }
        for i in 0..10 {
            let y = 420.0 + i as f32 * 12.0;
            glyph_row(&mut glyphs, y, 72.0, 288.0);
            glyph_row(&mut glyphs, y, 320.0, 540.0);
        }
        page_of(index, glyphs)
    }

    fn bands_of(pages: &[PageRaw]) -> Vec<Vec<Band>> {
        let lines: Vec<Vec<Line>> = pages.iter().map(crate::text::lines::build_lines).collect();
        document_bands(pages, &lines)
    }

    #[test]
    fn a_page_that_reads_its_own_columns_is_one_band() {
        let pages: Vec<PageRaw> = (0..4).map(two_column).collect();
        for bands in bands_of(&pages) {
            assert_eq!(bands.len(), 1, "{bands:?}");
            assert_eq!(bands[0].gutters.len(), 1, "{bands:?}");
        }
    }

    /// The failure this pass exists for: the columns below a full-width table are invisible to
    /// the page's own profile, and the document has to supply them.
    #[test]
    fn columns_below_a_full_width_table_are_found_band_wise() {
        let mut pages: Vec<PageRaw> = (0..3).map(two_column).collect();
        pages.push(table_over_two_columns(3));

        let bands = bands_of(&pages).pop().expect("a page of bands");
        assert!(bands.len() >= 2, "the page was not split: {bands:?}");
        assert!(
            bands[0].gutters.is_empty(),
            "the table was given columns: {bands:?}"
        );

        let below = gutters_at(&bands, 470.0);
        assert_eq!(below.len(), 1, "no gutter under the table: {bands:?}");
        assert!(below[0].0 >= 285.0 && below[0].1 <= 322.0, "{below:?}");
        assert!(
            gutters_at(&bands, 150.0).is_empty(),
            "the table's own rows were given a gutter"
        );
    }

    /// And the reason the search has to be anchored: the same page in a single-column document
    /// must keep its table intact. A band of a page is a small sample, and left to itself it
    /// finds column boundaries in tables, author blocks and figure labels.
    #[test]
    fn a_document_with_no_columns_is_never_given_any() {
        let mut pages = Vec::new();
        for index in 0..3 {
            let mut glyphs = Vec::new();
            for i in 0..30 {
                glyph_row(&mut glyphs, 100.0 + i as f32 * 12.0, 72.0, 540.0);
            }
            pages.push(page_of(index, glyphs));
        }
        pages.push(table_over_two_columns(3));

        for bands in bands_of(&pages) {
            assert_eq!(bands.len(), 1, "a single-column page was sliced: {bands:?}");
            assert!(bands[0].gutters.is_empty(), "{bands:?}");
        }
    }

    /// Two pages agreeing could be two pages carrying the same wide table. A spine needs more.
    #[test]
    fn too_few_agreeing_pages_are_not_a_spine() {
        let mut pages: Vec<PageRaw> = (0..2).map(two_column).collect();
        pages.push(table_over_two_columns(2));

        let bands = bands_of(&pages).pop().expect("a page of bands");
        assert_eq!(bands.len(), 1, "{bands:?}");
        assert!(bands[0].gutters.is_empty(), "{bands:?}");
    }

    #[test]
    fn every_height_lands_in_exactly_one_band() {
        let mut pages: Vec<PageRaw> = (0..3).map(two_column).collect();
        pages.push(table_over_two_columns(3));
        let bands = bands_of(&pages).pop().expect("a page of bands");

        for y in [-100.0, 0.0, 150.0, 399.5, 470.0, 792.0, 10_000.0] {
            let hits = bands.iter().filter(|b| b.contains(y)).count();
            assert_eq!(hits, 1, "y={y} landed in {hits} bands of {bands:?}");
        }
    }
}
