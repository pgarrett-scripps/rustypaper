//! Reading order by recursive XY-cut.
//!
//! The page is split along whitespace bands that no line crosses, recursively, and the resulting
//! tree is walked top-to-bottom then left-to-right.
//!
//! The ordering of the two cut directions is what makes papers work. A region containing a
//! full-width title above two columns has no valid vertical cut — the title crosses any gutter —
//! so it is cut horizontally first, and only the body region below is then cut into columns.
//! That falls out of the algorithm rather than needing a special case, and it is the reason a
//! wide figure dropped into the middle of a two-column page does not scramble the text around
//! it. Preferring vertical cuts where one exists is what stops the two columns from being
//! interleaved line by line.

use crate::text::lines::Line;

/// A vertical band must be at least this wide to cut on. Narrower bands are ragged right edges.
const MIN_VERTICAL_GAP: f32 = 9.0;

/// A horizontal band must be at least this tall, relative to the region's typical line height,
/// to cut on. Anything smaller is ordinary leading.
const MIN_HORIZONTAL_GAP_RATIO: f32 = 0.15;

/// Below this many lines, a region is just sorted; there is not enough evidence for a vertical
/// cut and a spurious one is worse than none.
const MIN_LINES_FOR_VERTICAL_CUT: usize = 4;

/// Guards against pathological recursion on adversarial input.
const MAX_DEPTH: usize = 32;

/// Returns `lines` in reading order.
pub fn reading_order(lines: Vec<Line>) -> Vec<Line> {
    let mut indices: Vec<usize> = (0..lines.len()).collect();
    let mut out = Vec::with_capacity(lines.len());
    cut(&mut indices, &lines, &mut out, 0);

    let mut ordered: Vec<Option<Line>> = lines.into_iter().map(Some).collect();
    out.into_iter().filter_map(|i| ordered[i].take()).collect()
}

fn cut(indices: &mut Vec<usize>, lines: &[Line], out: &mut Vec<usize>, depth: usize) {
    if indices.len() <= 1 || depth >= MAX_DEPTH {
        indices.sort_by(|&a, &b| compare_position(&lines[a], &lines[b]));
        out.append(indices);
        return;
    }

    // Vertical first: within a body region the columns are the dominant structure, and a
    // horizontal cut there would interleave them.
    if indices.len() >= MIN_LINES_FOR_VERTICAL_CUT {
        if let Some(groups) = split_vertically(indices, lines) {
            for mut group in groups {
                cut(&mut group, lines, out, depth + 1);
            }
            return;
        }
    }

    if let Some(groups) = split_horizontally(indices, lines) {
        for mut group in groups {
            cut(&mut group, lines, out, depth + 1);
        }
        return;
    }

    indices.sort_by(|&a, &b| compare_position(&lines[a], &lines[b]));
    out.append(indices);
}

fn compare_position(a: &Line, b: &Line) -> std::cmp::Ordering {
    a.baseline
        .total_cmp(&b.baseline)
        .then(a.bbox.x0.total_cmp(&b.bbox.x0))
}

/// Splits into horizontal bands at the widest whitespace no line crosses.
fn split_horizontally(indices: &[usize], lines: &[Line]) -> Option<Vec<Vec<usize>>> {
    let threshold = typical_height(indices, lines) * MIN_HORIZONTAL_GAP_RATIO;
    split_on_axis(indices, threshold, |i| (lines[i].bbox.y0, lines[i].bbox.y1))
}

/// Splits into columns at the widest vertical band no line crosses.
fn split_vertically(indices: &[usize], lines: &[Line]) -> Option<Vec<Vec<usize>>> {
    split_on_axis(indices, MIN_VERTICAL_GAP, |i| {
        (lines[i].bbox.x0, lines[i].bbox.x1)
    })
}

/// Sweeps one axis and splits at the widest gaps.
///
/// Cutting at *every* gap rather than the widest is the mistake that makes XY-cut useless on
/// papers: ordinary leading between two body lines is a gap, so a page would be shredded into
/// one band per line and the column structure could never be seen. Splitting only at the widest
/// gap — and any within 15% of it, so uniformly-leaded lines stay together in one step — lets
/// the dominant structure win at each level and the rest be found by recursion.
fn split_on_axis(
    indices: &[usize],
    threshold: f32,
    extent: impl Fn(usize) -> (f32, f32),
) -> Option<Vec<Vec<usize>>> {
    let mut order: Vec<usize> = indices.to_vec();
    order.sort_by(|&a, &b| extent(a).0.total_cmp(&extent(b).0));

    // Gap before each entry, in sorted order, measured against how far the preceding entries
    // reach. Overlapping entries give a negative gap.
    let mut gaps = Vec::with_capacity(order.len());
    let mut reach = f32::NEG_INFINITY;
    for (n, &i) in order.iter().enumerate() {
        let (lo, hi) = extent(i);
        gaps.push(if n == 0 {
            f32::NEG_INFINITY
        } else {
            lo - reach
        });
        reach = reach.max(hi);
    }

    let widest = gaps
        .iter()
        .copied()
        .filter(|g| g.is_finite())
        .fold(f32::NEG_INFINITY, f32::max);
    if !widest.is_finite() || widest <= threshold {
        return None;
    }
    let cut_at = widest * 0.85;

    let mut groups = Vec::new();
    let mut current = Vec::new();
    for (n, &i) in order.iter().enumerate() {
        if gaps[n] >= cut_at && !current.is_empty() {
            groups.push(std::mem::take(&mut current));
        }
        current.push(i);
    }
    if !current.is_empty() {
        groups.push(current);
    }

    (groups.len() > 1).then_some(groups)
}

fn typical_height(indices: &[usize], lines: &[Line]) -> f32 {
    let mut heights: Vec<f32> = indices
        .iter()
        .map(|&i| lines[i].bbox.height().max(lines[i].size))
        .filter(|h| *h > 0.0)
        .collect();
    if heights.is_empty() {
        return 10.0;
    }
    heights.sort_by(f32::total_cmp);
    heights[heights.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;
    use crate::text::lines::Line;

    fn line(text: &str, baseline: f32, x0: f32, x1: f32) -> Line {
        Line {
            bbox: Rect::from_corners(x0, baseline - 10.0, x1, baseline),
            baseline,
            size: 10.0,
            bold: false,
            italic: false,
            glyphs: Vec::new(),
            words: vec![crate::text::lines::Word {
                bbox: Rect::from_corners(x0, baseline - 10.0, x1, baseline),
                text: text.to_owned(),
                start: 0,
                end: 1,
            }],
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.text()).collect()
    }

    #[test]
    fn single_column_stays_in_vertical_order() {
        let lines = vec![
            line("one", 100.0, 72.0, 540.0),
            line("two", 112.0, 72.0, 540.0),
            line("three", 124.0, 72.0, 540.0),
        ];
        assert_eq!(texts(&reading_order(lines)), ["one", "two", "three"]);
    }

    #[test]
    fn two_columns_are_read_one_after_the_other_not_interleaved() {
        let mut lines = Vec::new();
        for i in 0..5 {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(line(&format!("L{i}"), y, 72.0, 288.0));
            lines.push(line(&format!("R{i}"), y, 320.0, 540.0));
        }
        assert_eq!(
            texts(&reading_order(lines)),
            ["L0", "L1", "L2", "L3", "L4", "R0", "R1", "R2", "R3", "R4"]
        );
    }

    #[test]
    fn a_full_width_title_is_read_before_both_columns() {
        let mut lines = vec![line("TITLE", 60.0, 72.0, 540.0)];
        for i in 0..5 {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(line(&format!("L{i}"), y, 72.0, 288.0));
            lines.push(line(&format!("R{i}"), y, 320.0, 540.0));
        }
        let ordered = texts(&reading_order(lines));
        assert_eq!(ordered[0], "TITLE");
        assert_eq!(&ordered[1..6], ["L0", "L1", "L2", "L3", "L4"]);
        assert_eq!(&ordered[6..], ["R0", "R1", "R2", "R3", "R4"]);
    }

    /// The case that motivates cutting horizontally before descending into columns: a wide
    /// figure part-way down a two-column page. Text above it and below it must not merge into
    /// one column-major run that reads across the figure.
    #[test]
    fn a_full_width_figure_splits_the_columns_around_it() {
        let mut lines = Vec::new();
        for i in 0..3 {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(line(&format!("aL{i}"), y, 72.0, 288.0));
            lines.push(line(&format!("aR{i}"), y, 320.0, 540.0));
        }
        lines.push(line("FIGURE", 200.0, 72.0, 540.0));
        for i in 0..3 {
            let y = 300.0 + i as f32 * 12.0;
            lines.push(line(&format!("bL{i}"), y, 72.0, 288.0));
            lines.push(line(&format!("bR{i}"), y, 320.0, 540.0));
        }

        let ordered = texts(&reading_order(lines));
        assert_eq!(
            ordered,
            [
                "aL0", "aL1", "aL2", "aR0", "aR1", "aR2", "FIGURE", "bL0", "bL1", "bL2", "bR0",
                "bR1", "bR2"
            ]
        );
    }

    #[test]
    fn three_columns_read_left_to_right() {
        let mut lines = Vec::new();
        for i in 0..4 {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(line(&format!("A{i}"), y, 40.0, 190.0));
            lines.push(line(&format!("B{i}"), y, 220.0, 370.0));
            lines.push(line(&format!("C{i}"), y, 400.0, 550.0));
        }
        let ordered = texts(&reading_order(lines));
        assert_eq!(&ordered[0..4], ["A0", "A1", "A2", "A3"]);
        assert_eq!(&ordered[4..8], ["B0", "B1", "B2", "B3"]);
        assert_eq!(&ordered[8..12], ["C0", "C1", "C2", "C3"]);
    }

    #[test]
    fn empty_input_is_handled() {
        assert!(reading_order(Vec::new()).is_empty());
    }

    #[test]
    fn every_line_survives_the_cut() {
        let mut lines = Vec::new();
        for i in 0..40 {
            let y = 60.0 + i as f32 * 12.0;
            lines.push(line(
                &format!("x{i}"),
                y,
                72.0 + (i % 3) as f32 * 150.0,
                200.0,
            ));
        }
        let n = lines.len();
        assert_eq!(
            reading_order(lines).len(),
            n,
            "lines were lost or duplicated"
        );
    }
}
