//! Table detection and reconstruction.
//!
//! Scientific papers overwhelmingly use `booktabs`, which draws **horizontal rules only** — a
//! top rule, a rule under the header, and a bottom rule, with no vertical lines anywhere. That
//! makes the classic "find the grid" approach useless here: there is no grid. What the rules do
//! give is the table's *extent*, which is the hard part, and columns are then recovered from the
//! whitespace between cells.
//!
//! Rules also tell us which rows are the header, since the second rule of a `booktabs` table is
//! conventionally the one below the column titles.

use crate::ir::{PageRaw, PathKind, Rect};
use crate::text::lines::Line;

/// Two rules belong to the same table if their horizontal extents overlap by at least this
/// fraction of the shorter one.
const MIN_RULE_OVERLAP: f32 = 0.6;

/// Rules further apart than this vertically are not the same table.
const MAX_RULE_SPACING: f32 = 320.0;

/// A gap between columns, as a fraction of the body font size.
const MIN_COLUMN_GAP: f32 = 0.55;

/// A table needs at least this many rows and columns to be worth calling one.
const MIN_ROWS: usize = 2;
const MIN_COLUMNS: usize = 2;

/// A column separator may be crossed by at most this fraction of the rows.
///
/// Not zero. Tables routinely carry a `\multicolumn` title spanning the whole width, and
/// demanding that no row cross a separator lets one such row erase every column in the table.
const MAX_SEPARATOR_OCCUPANCY: f32 = 0.34;

/// A reconstructed table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub bbox: Rect,
    /// Cell text, row-major. Rows are padded to a uniform width.
    pub rows: Vec<Vec<String>>,
    /// How many leading rows are header rows.
    pub header_rows: usize,
    /// Indices into the page's line list that were consumed.
    pub consumed: Vec<usize>,
}

impl Table {
    pub fn columns(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }
}

/// Finds the tables on a page.
pub fn detect(page: &PageRaw, lines: &[Line], body_size: f32) -> Vec<Table> {
    let mut rules: Vec<Rect> = page
        .paths
        .iter()
        .filter(|p| p.kind == PathKind::HorizontalRule)
        .map(|p| p.bbox)
        .collect();
    rules.sort_by(|a, b| a.y0.total_cmp(&b.y0));

    let mut tables = Vec::new();
    let mut used: Vec<bool> = vec![false; lines.len()];

    for group in group_rules(&rules) {
        if let Some(table) = build(&group, lines, body_size, &mut used) {
            tables.push(table);
        }
    }

    tables
}

/// Clusters rules that plausibly belong to the same table.
fn group_rules(rules: &[Rect]) -> Vec<Vec<Rect>> {
    let mut groups: Vec<Vec<Rect>> = Vec::new();

    for &rule in rules {
        match groups.last_mut() {
            Some(group)
                if aligned(group.last().unwrap(), &rule)
                    && rule.y0 - group.last().unwrap().y1 <= MAX_RULE_SPACING =>
            {
                group.push(rule);
            }
            _ => groups.push(vec![rule]),
        }
    }

    groups.retain(|g| g.len() >= 2);
    groups
}

fn aligned(a: &Rect, b: &Rect) -> bool {
    let shorter = a.width().min(b.width()).max(1.0);
    a.x_overlap(b) / shorter >= MIN_RULE_OVERLAP
}

/// Builds a table from a group of rules and the lines they enclose.
fn build(rules: &[Rect], lines: &[Line], body_size: f32, used: &mut [bool]) -> Option<Table> {
    let top = rules.first()?.y0;
    let bottom = rules.last()?.y1;
    let left = rules.iter().map(|r| r.x0).fold(f32::MAX, f32::min);
    let right = rules.iter().map(|r| r.x1).fold(f32::MIN, f32::max);
    let region = Rect {
        x0: left,
        y0: top,
        x1: right,
        y1: bottom,
    };

    // Rows are the lines between the first and last rule. Using the baseline rather than the
    // bounding box keeps a tall cell from being claimed by the row above it.
    let mut members: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            !used[i]
                && lines[i].baseline > top
                && lines[i].baseline < bottom
                && lines[i].bbox.center_x() >= left - body_size
                && lines[i].bbox.center_x() <= right + body_size
        })
        .collect();
    members.sort_by(|&a, &b| lines[a].baseline.total_cmp(&lines[b].baseline));

    if members.len() < MIN_ROWS {
        return None;
    }

    let boundaries = column_boundaries(lines, &members, region, body_size);
    if boundaries.len() < MIN_COLUMNS {
        return None;
    }

    let rows: Vec<Vec<String>> = members
        .iter()
        .map(|&i| row_cells(&lines[i], &boundaries))
        .collect();

    // A table where every row is one cell is a run of centred text, not a table.
    if rows
        .iter()
        .all(|r| r.iter().filter(|c| !c.is_empty()).count() < MIN_COLUMNS)
    {
        return None;
    }

    for &i in &members {
        used[i] = true;
    }

    Some(Table {
        bbox: region,
        header_rows: header_rows(rules, lines, &members),
        rows,
        consumed: members,
    })
}

/// Infers column boundaries from the whitespace that every row shares.
///
/// A column separator is an x-band that no word crosses. Occupancy is counted per *row* rather
/// than per word so that one wide title spanning the table does not erase the separators the
/// other rows agree on.
fn column_boundaries(
    lines: &[Line],
    members: &[usize],
    region: Rect,
    body_size: f32,
) -> Vec<(f32, f32)> {
    const BIN: f32 = 1.0;

    let width = region.width();
    if width <= 0.0 {
        return Vec::new();
    }
    let bins = (width / BIN).ceil() as usize + 1;
    let mut occupied = vec![0u32; bins];

    for &i in members {
        let mut row_mask = vec![false; bins];
        for word in &lines[i].words {
            let lo = (((word.bbox.x0 - region.x0) / BIN).floor().max(0.0) as usize).min(bins - 1);
            let hi = (((word.bbox.x1 - region.x0) / BIN).ceil().max(0.0) as usize).min(bins);
            for slot in &mut row_mask[lo..hi] {
                *slot = true;
            }
        }
        for (slot, hit) in occupied.iter_mut().zip(row_mask) {
            *slot += u32::from(hit);
        }
    }

    let min_gap = (body_size * MIN_COLUMN_GAP).max(2.0);
    let ceiling = (members.len() as f32 * MAX_SEPARATOR_OCCUPANCY).floor() as u32;
    let mut columns = Vec::new();
    let mut start: Option<usize> = None;
    let mut gap_start: Option<usize> = None;

    for i in 0..bins {
        let empty = occupied[i] <= ceiling;
        match (empty, start, gap_start) {
            (false, None, _) => {
                start = Some(i);
                gap_start = None;
            }
            (false, Some(_), Some(g)) => {
                // A gap ended. If it was wide enough, it separated two columns.
                if (i - g) as f32 * BIN >= min_gap {
                    push_column(&mut columns, region.x0, start.take().unwrap(), g);
                    start = Some(i);
                }
                gap_start = None;
            }
            (true, Some(_), None) => gap_start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        push_column(&mut columns, region.x0, s, gap_start.unwrap_or(bins));
    }

    columns
}

fn push_column(columns: &mut Vec<(f32, f32)>, origin: f32, start: usize, end: usize) {
    if end > start {
        columns.push((origin + start as f32, origin + end as f32));
    }
}

/// Assigns a row's words to columns.
fn row_cells(line: &Line, boundaries: &[(f32, f32)]) -> Vec<String> {
    let mut cells = vec![String::new(); boundaries.len()];

    for word in &line.words {
        // The column its centre falls in, or the nearest one if it straddles a boundary.
        let centre = word.bbox.center_x();
        let index = boundaries
            .iter()
            .position(|&(lo, hi)| centre >= lo && centre <= hi)
            .or_else(|| {
                boundaries
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        distance(centre, **a).total_cmp(&distance(centre, **b))
                    })
                    .map(|(i, _)| i)
            });
        if let Some(i) = index {
            if !cells[i].is_empty() {
                cells[i].push(' ');
            }
            cells[i].push_str(&word.text);
        }
    }

    cells
}

fn distance(x: f32, (lo, hi): (f32, f32)) -> f32 {
    if x < lo {
        lo - x
    } else if x > hi {
        x - hi
    } else {
        0.0
    }
}

/// How many leading rows sit above the second rule.
///
/// `booktabs` puts a `\midrule` under the column headings, so the rule after the top one marks
/// the end of the header. With only two rules there is no midrule and no header.
fn header_rows(rules: &[Rect], lines: &[Line], members: &[usize]) -> usize {
    if rules.len() < 3 {
        return 0;
    }
    let midrule = rules[1].y1;
    members
        .iter()
        .take_while(|&&i| lines[i].baseline < midrule)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{PathItem, Rgba};
    use crate::text::lines::Word;

    fn rule(y: f32, x0: f32, x1: f32) -> PathItem {
        PathItem {
            bbox: Rect::from_corners(x0, y, x1, y + 0.6),
            kind: PathKind::HorizontalRule,
            thickness: 0.6,
            color: Rgba::BLACK,
            filled: true,
            stroked: false,
        }
    }

    /// A row of cells at fixed x positions.
    fn row(baseline: f32, cells: &[(&str, f32, f32)]) -> Line {
        let words: Vec<Word> = cells
            .iter()
            .map(|&(text, x0, x1)| Word {
                bbox: Rect::from_corners(x0, baseline - 8.0, x1, baseline),
                text: text.to_owned(),
                start: 0,
                end: 1,
            })
            .collect();
        let bbox = words
            .iter()
            .map(|w| w.bbox)
            .reduce(|a, b| a.union(&b))
            .unwrap();
        Line {
            bbox,
            baseline,
            size: 10.0,
            bold: false,
            italic: false,
            glyphs: Vec::new(),
            words,
        }
    }

    fn page_with(rules: Vec<PathItem>) -> PageRaw {
        PageRaw {
            index: 0,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            glyphs: Vec::new(),
            paths: rules,
            images: Vec::new(),
            expansions: Vec::new(),
        }
    }

    /// The canonical booktabs shape: three rules, a header row, two body rows.
    fn booktabs() -> (PageRaw, Vec<Line>) {
        let page = page_with(vec![
            rule(100.0, 72.0, 400.0),
            rule(120.0, 72.0, 400.0),
            rule(170.0, 72.0, 400.0),
        ]);
        let lines = vec![
            row(
                115.0,
                &[
                    ("Model", 76.0, 110.0),
                    ("BLEU", 200.0, 232.0),
                    ("Cost", 320.0, 350.0),
                ],
            ),
            row(
                140.0,
                &[
                    ("ByteNet", 76.0, 120.0),
                    ("23.75", 200.0, 232.0),
                    ("1.0", 320.0, 340.0),
                ],
            ),
            row(
                160.0,
                &[
                    ("GNMT", 76.0, 112.0),
                    ("24.61", 200.0, 232.0),
                    ("2.3", 320.0, 340.0),
                ],
            ),
        ];
        (page, lines)
    }

    #[test]
    fn reconstructs_a_booktabs_table() {
        let (page, lines) = booktabs();
        let tables = detect(&page, &lines, 10.0);
        assert_eq!(tables.len(), 1, "expected one table, got {tables:?}");

        let table = &tables[0];
        assert_eq!(table.columns(), 3);
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0], ["Model", "BLEU", "Cost"]);
        assert_eq!(table.rows[1], ["ByteNet", "23.75", "1.0"]);
        assert_eq!(table.rows[2], ["GNMT", "24.61", "2.3"]);
    }

    #[test]
    fn the_midrule_marks_the_header() {
        let (page, lines) = booktabs();
        let tables = detect(&page, &lines, 10.0);
        assert_eq!(tables[0].header_rows, 1);
    }

    #[test]
    fn two_rules_alone_give_no_header() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0), rule(170.0, 72.0, 400.0)]);
        let lines = vec![
            row(120.0, &[("a", 76.0, 90.0), ("b", 200.0, 214.0)]),
            row(140.0, &[("c", 76.0, 90.0), ("d", 200.0, 214.0)]),
        ];
        let tables = detect(&page, &lines, 10.0);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].header_rows, 0);
    }

    #[test]
    fn consumed_lines_are_reported() {
        let (page, lines) = booktabs();
        let tables = detect(&page, &lines, 10.0);
        assert_eq!(tables[0].consumed, vec![0, 1, 2]);
    }

    /// A single rule is a section divider or a footnote separator, not a table.
    #[test]
    fn one_rule_is_not_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0)]);
        let lines = vec![row(120.0, &[("a", 76.0, 90.0), ("b", 200.0, 214.0)])];
        assert!(detect(&page, &lines, 10.0).is_empty());
    }

    /// Rules with no horizontal relationship belong to different tables, or to none.
    #[test]
    fn unaligned_rules_do_not_form_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 200.0), rule(140.0, 400.0, 540.0)]);
        let lines = vec![row(120.0, &[("a", 76.0, 90.0), ("b", 420.0, 434.0)])];
        assert!(detect(&page, &lines, 10.0).is_empty());
    }

    /// A rule far above and one far below are page furniture, not a table's extent.
    #[test]
    fn rules_too_far_apart_are_not_one_table() {
        let page = page_with(vec![rule(60.0, 72.0, 540.0), rule(700.0, 72.0, 540.0)]);
        let lines = vec![
            row(300.0, &[("body", 76.0, 110.0), ("text", 200.0, 230.0)]),
            row(320.0, &[("more", 76.0, 110.0), ("text", 200.0, 230.0)]),
        ];
        assert!(detect(&page, &lines, 10.0).is_empty());
    }

    /// Centred prose between two rules has no column structure and must not become a table.
    #[test]
    fn single_column_content_is_not_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0), rule(170.0, 72.0, 400.0)]);
        let lines = vec![
            row(120.0, &[("a line of running text here", 76.0, 390.0)]),
            row(140.0, &[("and another line of text", 76.0, 380.0)]),
        ];
        assert!(detect(&page, &lines, 10.0).is_empty());
    }

    #[test]
    fn a_wide_spanning_cell_does_not_erase_the_columns() {
        // A title spanning the table, then two rows that agree on three columns.
        let page = page_with(vec![
            rule(100.0, 72.0, 400.0),
            rule(120.0, 72.0, 400.0),
            rule(180.0, 72.0, 400.0),
        ]);
        let lines = vec![
            row(115.0, &[("Results across all datasets", 76.0, 396.0)]),
            row(
                140.0,
                &[
                    ("A", 76.0, 90.0),
                    ("1.0", 200.0, 220.0),
                    ("2.0", 320.0, 340.0),
                ],
            ),
            row(
                160.0,
                &[
                    ("B", 76.0, 90.0),
                    ("3.0", 200.0, 220.0),
                    ("4.0", 320.0, 340.0),
                ],
            ),
        ];
        let tables = detect(&page, &lines, 10.0);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].columns(),
            3,
            "the spanning title erased the columns"
        );
        assert_eq!(tables[0].rows[1], ["A", "1.0", "2.0"]);
    }
}
