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

/// The shorter of two rules must be at least this fraction of the longer to be its table's.
///
/// `aligned` measures overlap against the *shorter* rule, which it has to: a `\cmidrule` under
/// one column of a table is short by design and lies wholly within the `\toprule` above it, so
/// it scores 1.0 and belongs. But so does a 10 pt `\frac` bar lying within a 467 pt page rule,
/// and that one does not. Overlap alone cannot tell them apart; the ratio of their lengths can.
/// Across the corpus the tightest real pairing is BERT's Table 8, whose `\cmidrule`s are 0.36 of
/// its `\toprule`, and the loosest false one is a biology `\frac` bar at 0.07 of the separator
/// above the page's running footer — the rule that, before this, turned a system of ODEs into a
/// 59-row table by vouching for the whole stack of fraction bars beneath it.
const MIN_RULE_WIDTH_RATIO: f32 = 0.25;

/// A group needs one rule at least this wide, in body font sizes, to be a table's rules.
///
/// `\frac` draws its bar as a filled rectangle, so every fraction in the document arrives here
/// looking exactly like a `\toprule`. What separates them is span: a table rule crosses the
/// columns of a tabular, while a fraction bar only covers a numerator. Across the corpus the
/// narrowest real table measures 17.6 em and the widest stack of fraction bars 8.4 em, so the
/// floor sits between them with room on both sides. It is the *widest* rule in the group that
/// has to clear it, since a `\cmidrule` under a single column is legitimately short — it just
/// never appears without a full-width rule above it.
const MIN_TABLE_RULE_WIDTH: f32 = 12.0;

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

/// Fragments of one border line sit at most this far apart horizontally, in points.
///
/// Word exports and journal production systems draw a table grid cell by cell: each border
/// arrives as a row of short rules, abutting or overlapping by fractions of a point, and no
/// single rule spans the table. Every fragment is then narrower than
/// [`MIN_TABLE_RULE_WIDTH`] and the whole table vanishes — the shape that lost every table
/// of two papers in the peer-review pilot. The tolerance is deliberately tiny: real
/// separations — a column gutter between side-by-side tables, the trimmed gap between a
/// pair of `\cmidrule`s — are several points at least, and a dashed decorative rule should
/// stay dashed.
const MAX_FRAGMENT_GAP: f32 = 1.5;

/// Two fragments belong to the same border line if their vertical centres are within this.
const FRAGMENT_Y_TOLERANCE: f32 = 1.0;

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
///
/// `figures` are the page's drawn regions, as [`crate::figure::regions`] reports them. A rule
/// inside one is not a table's — see [`inside_a_figure`] — and a figure between two rules means
/// they bound different things — see [`separated_by_a_figure`].
///
/// `lines` are read twice: for the cells, and for the captions that say where one table ends and
/// the next begins — see [`captioned_between`].
pub fn detect(page: &PageRaw, lines: &[Line], body_size: f32, figures: &[Rect]) -> Vec<Table> {
    let rules: Vec<Rect> = page
        .paths
        .iter()
        .filter(|p| p.kind == PathKind::HorizontalRule)
        .map(|p| p.bbox)
        .filter(|bbox| !inside_a_figure(bbox, figures))
        .collect();
    let mut rules = coalesce_fragments(rules);
    rules.sort_by(|a, b| a.y0.total_cmp(&b.y0));

    let captions = caption_lines(lines);

    let mut tables = Vec::new();
    let mut used: Vec<bool> = vec![false; lines.len()];

    let min_width = body_size * MIN_TABLE_RULE_WIDTH;
    for group in group_rules(&rules, body_size, figures, &captions) {
        // A chain can grow past a table's bottom rule into whatever rules a figure below
        // happens to align — the welded region then holds half a column of prose and
        // `build` rightly refuses it. The table is still there in the chain's head, so peel
        // rules off the tail until a prefix builds, and give what remains the same chance
        // if it could stand as a group on its own.
        let mut rest: &[Rect] = &group;
        while rest.len() >= 2 && rest.iter().any(|r| r.width() >= min_width) {
            let built = (2..=rest.len())
                .rev()
                .find_map(|end| build(&rest[..end], lines, body_size, &mut used).map(|t| (t, end)));
            let Some((table, end)) = built else {
                break;
            };
            tables.push(table);
            rest = &rest[end..];
        }
    }

    tables
}

/// Where each of the page's table captions sits.
///
/// A caption is the one thing on a page that says, in words, *this table ends here and the next
/// one begins*. See [`captioned_between`] for what is done with them.
fn caption_lines(lines: &[Line]) -> Vec<Caption> {
    lines
        .iter()
        .filter(|line| opens_a_caption(line))
        .map(|line| Caption {
            baseline: line.baseline,
            centre_x: line.bbox.center_x(),
        })
        .collect()
}

/// A caption's position: its baseline, and the middle of its measure.
#[derive(Debug, Clone, Copy)]
struct Caption {
    baseline: f32,
    centre_x: f32,
}

/// Whether a line begins `Table 9.`, `TABLE II`, `Tab. 3:` — a table's number, not its mention.
///
/// The label has to be the line's *first* word and be followed by a number. Papers refer to their
/// own tables constantly (`… improves the mAP by over 2 points (Table 9).`), and a mention that
/// happens to land between two rules must not part them; `Table` at the head of a line, with a
/// numeral after it, is a caption or the opening of a sentence about one, and either way the
/// rules on the two sides of it are not one tabular's.
fn opens_a_caption(line: &Line) -> bool {
    let mut words = line.words.iter().map(|w| w.text.as_str());
    let Some(label) = words.next() else {
        return false;
    };
    let label = label.trim_end_matches(['.', ':']);
    if !(label.eq_ignore_ascii_case("table") || label.eq_ignore_ascii_case("tab")) {
        return false;
    }
    words.next().is_some_and(is_a_table_number)
}

/// Whether a word is a table's number: `9`, `9.`, `II`, `IV:`.
///
/// Arabic and Roman both, because the corpus sets it both ways — `Table 9.` in the LaTeX article
/// classes, `TABLE II` under REVTeX and IEEEtran.
fn is_a_table_number(word: &str) -> bool {
    let word = word.trim_end_matches(['.', ':', ')']);
    let arabic = |w: &str| w.chars().all(|c| c.is_ascii_digit());
    let roman = |w: &str| w.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L'));
    !word.is_empty() && (arabic(word) || roman(word))
}

/// Whether a rule lies wholly within one of the page's drawn figure regions.
///
/// A plot is not made of prose, but it is full of straight horizontal lines: error bars, box
/// plots, legend keys, the frame around a panel. Every one of them arrives as a
/// `HorizontalRule`, indistinguishable from a `\toprule` by width alone — biology's population
/// plots draw pairs of 17.5 em lines 117 pt apart, which is a textbook `booktabs` signature —
/// and what they enclose is the letterspaced subset-font text of the plot's own labels. The
/// result reads as a table of `5 D Z  G D W D`.
///
/// Containment has to be total, and in both axes. A real table often shares a column with a
/// figure and so lies inside its region's *horizontal* span: ResNet's Table 1 spans exactly the
/// x-range of the plot below it, and testing x alone would have discarded the paper's largest
/// table. Nor is the padded margin that [`crate::pipeline`] uses for stray labels right here — a
/// table directly under a figure is adjacent to it, not part of it.
fn inside_a_figure(rule: &Rect, figures: &[Rect]) -> bool {
    figures.iter().any(|region| region.contains(rule))
}

/// Whether a whole figure sits in the gap between two rules.
///
/// Rules that far apart are already the doubtful case — [`MAX_RULE_SPACING`] allows 320 pt,
/// because a real table can run that long between its `\midrule` and its `\bottomrule` — and a
/// drawing lying in the gap settles it: whatever the two rules bound, it is not one tabular.
/// ResNet's page 5 carries Table 1, then Figure 4, then Table 2, and the axis ticks under
/// Table 1 overlap Table 2's rules well enough to pass `aligned`, which fused the pair into a
/// single 240 pt "table" of legend text and lost Table 2 entirely.
///
/// The figure has to lie *entirely* within the gap. A tall figure merely beside a table in the
/// other column straddles its rules rather than parting them, and that is the common case.
fn separated_by_a_figure(above: &Rect, below: &Rect, figures: &[Rect]) -> bool {
    figures
        .iter()
        .any(|region| region.y0 >= above.y1 && region.y1 <= below.y0)
}

/// Whether a table caption sits in the gap between two rules.
///
/// Geometry alone cannot part two tables stacked in the same measure. ResNet's page 11 sets four
/// of them one under the next, and their x-extents are *nested* — 139.8–455.4 inside 51.7–543.5
/// around 324.6–529.3 — so every pair scores a perfect [`overlap_fraction`] and clears
/// [`MIN_RULE_WIDTH_RATIO`] as comfortably as a `\cmidrule` does. Spacing cannot part them
/// either: the widest gap between two of these tables is 56 pt, and a real `\midrule` to
/// `\bottomrule` run legitimately reaches [`MAX_RULE_SPACING`]. All seventeen rules chained into
/// one 622 pt "table" of 69 rows that swallowed both columns of the page's body text and lost
/// three tables.
///
/// What is unambiguous is the caption. `Table 9.` between two rules says in words that the one
/// above closes a tabular and the one below opens another, and no arrangement of rules says it.
/// The test is deliberately narrow — see [`opens_a_caption`], which wants the label at the head
/// of the line and a number after it — because a caption that is missed only leaves today's
/// merge, while a mention mistaken for one splits a table that was right.
///
/// The caption must also lie horizontally within the pair, or a caption in the *other* column
/// would part a table in this one. Their union is the span to test against rather than their
/// intersection: a full-measure caption under a table that occupies one column is centred on the
/// page, outside the narrower rule but inside the pair.
fn captioned_between(above: &Rect, below: &Rect, captions: &[Caption]) -> bool {
    let left = above.x0.min(below.x0);
    let right = above.x1.max(below.x1);
    captions.iter().any(|caption| {
        caption.baseline > above.y1
            && caption.baseline < below.y0
            && caption.centre_x >= left
            && caption.centre_x <= right
    })
}

/// Clusters rules that plausibly belong to the same table.
///
/// Rules arrive in y order and are chained by vertical adjacency, but every open chain stays a
/// candidate rather than only the most recent one. Considering only the last chain — the obvious
/// single pass — loses both tables whenever two sit side by side in different columns, which is
/// the common case in the two-column papers this corpus is mostly made of. Their rules
/// interleave in y (A-top, B-top, A-mid, B-mid, …), so every consecutive pair fails `aligned`,
/// every chain ends up a singleton, and the `len() >= 2` filter below then discards the lot.
///
/// Where several chains could take a rule, the best-overlapping one does, and the most recent of
/// those wins a tie. With a single table on the page there is only ever one chain, so this is
/// exactly the old behaviour.
fn group_rules(
    rules: &[Rect],
    body_size: f32,
    figures: &[Rect],
    captions: &[Caption],
) -> Vec<Vec<Rect>> {
    let mut groups: Vec<Vec<Rect>> = Vec::new();

    for &rule in rules {
        let candidate = groups
            .iter_mut()
            .filter(|group| {
                let last = group.last().unwrap();
                rule.y0 - last.y1 <= MAX_RULE_SPACING
                    && aligned(last, &rule)
                    && !separated_by_a_figure(last, &rule, figures)
                    && !captioned_between(last, &rule, captions)
            })
            .max_by(|a, b| {
                overlap_fraction(a.last().unwrap(), &rule)
                    .total_cmp(&overlap_fraction(b.last().unwrap(), &rule))
            });

        match candidate {
            Some(group) => group.push(rule),
            None => groups.push(vec![rule]),
        }
    }

    let min_width = body_size * MIN_TABLE_RULE_WIDTH;
    groups.retain(|g| g.len() >= 2 && g.iter().any(|r| r.width() >= min_width));
    groups
}

/// How much of the shorter rule the two share horizontally.
fn overlap_fraction(a: &Rect, b: &Rect) -> f32 {
    let shorter = a.width().min(b.width()).max(1.0);
    a.x_overlap(b) / shorter
}

fn aligned(a: &Rect, b: &Rect) -> bool {
    let (shorter, longer) = if a.width() < b.width() {
        (a.width(), b.width())
    } else {
        (b.width(), a.width())
    };
    overlap_fraction(a, b) >= MIN_RULE_OVERLAP && shorter / longer.max(1.0) >= MIN_RULE_WIDTH_RATIO
}

/// Joins the fragments of a border line drawn cell by cell into one rule.
///
/// Sorting by y then x puts the fragments of one border next to each other, so a single
/// pass merges every run of collinear, abutting fragments — see [`MAX_FRAGMENT_GAP`] for
/// why the gap allowance is well under any real separation.
fn coalesce_fragments(mut rules: Vec<Rect>) -> Vec<Rect> {
    rules.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));

    let mut out: Vec<Rect> = Vec::with_capacity(rules.len());
    for rule in rules {
        if let Some(last) = out.last_mut() {
            let last_centre = (last.y0 + last.y1) * 0.5;
            let centre = (rule.y0 + rule.y1) * 0.5;
            if (centre - last_centre).abs() <= FRAGMENT_Y_TOLERANCE
                && rule.x0 - last.x1 <= MAX_FRAGMENT_GAP
            {
                last.x1 = last.x1.max(rule.x1);
                last.y0 = last.y0.min(rule.y0);
                last.y1 = last.y1.max(rule.y1);
                continue;
            }
        }
        out.push(rule);
    }
    out
}

/// The characters a page uses for a minus sign: the hyphen, U+2212 MINUS SIGN, and the
/// dashes maths and Word fonts stand in for it.
fn is_sign(c: char) -> bool {
    matches!(c, '-' | '\u{2212}' | '\u{2013}' | '\u{2014}')
}

/// Reattaches a detached minus to its number within one cell, and writes it as `-`.
///
/// A negative value often reaches the page as two text runs — the sign, then the digits a
/// word-space later — and joining the cell's words with spaces then prints `− 43.801`:
/// subtraction, to any reader downstream. A sign is joined only where nothing numeric
/// precedes it in the cell, so `12 – 15` stays the range it is, and only when digits
/// follow, so a lone placeholder dash is left alone.
fn repair_cell(cell: &str) -> String {
    let chars: Vec<char> = cell.chars().collect();
    let mut out = String::with_capacity(cell.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let at_boundary = out.is_empty() || out.ends_with([' ', '(', '[']);
        if is_sign(c) && at_boundary {
            let mut j = i + 1;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let digits_follow = chars.get(j).is_some_and(|d| {
                d.is_ascii_digit()
                    || (*d == '.' && chars.get(j + 1).is_some_and(char::is_ascii_digit))
            });
            let previous_token = out
                .trim_end()
                .chars()
                .rev()
                .take_while(|ch| *ch != ' ')
                .collect::<String>();
            // A numeric token right before the sign makes it a range (`12 – 15`) — unless
            // that token ends in a list separator, as in an interval `[-141.860, − 76.891]`,
            // where the sign belongs to the value after the comma.
            // `previous_token` is collected back to front, so its first character is the
            // one written last.
            let range_so_far = previous_token.chars().any(|ch| ch.is_ascii_digit())
                && !previous_token.starts_with([',', ';']);
            if digits_follow && !range_so_far {
                out.push('-');
                i = j;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Folds a column that holds nothing but minus signs into the number column beside it.
///
/// Right-aligned numbers sometimes set their signs far enough left that the whitespace
/// profile cuts a separator between sign and digits, and the signs become a column of
/// their own — a reader of the number column then sees every negative value as positive.
/// The merge is deliberately narrow, because tables also use a lone dash as a *placeholder*
/// for a missing value, and folding a placeholder column into its neighbour would invent
/// negative numbers: every non-empty cell must be a single sign whose right neighbour
/// starts with a digit, the column must be empty throughout the header, and both the sign
/// column and its distance to the digits must be narrower than `body_size` says a real
/// column and gap would be.
fn merge_sign_column(
    rows: &mut [Vec<String>],
    boundaries: &[(f32, f32)],
    header_rows: usize,
    body_size: f32,
) -> Vec<usize> {
    let columns = rows.first().map_or(0, Vec::len);
    let mut dropped = Vec::new();

    for c in 0..columns.saturating_sub(1) {
        let (lo, hi) = boundaries[c];
        let narrow = hi - lo <= body_size * 1.5;
        let adjacent = boundaries[c + 1].0 - hi <= body_size * 1.5;
        if !narrow || !adjacent {
            continue;
        }

        let mut signs = 0usize;
        let mut sign_column = true;
        for (r, row) in rows.iter().enumerate() {
            let cell = row[c].trim();
            if cell.is_empty() {
                continue;
            }
            let lone_sign = cell.chars().count() == 1 && cell.chars().all(is_sign);
            let number_beside = row[c + 1]
                .trim_start()
                .starts_with(|ch: char| ch.is_ascii_digit() || ch == '.');
            if r < header_rows || !lone_sign || !number_beside {
                sign_column = false;
                break;
            }
            signs += 1;
        }
        if !sign_column || signs == 0 {
            continue;
        }

        for row in rows.iter_mut() {
            if !row[c].trim().is_empty() {
                row[c] = String::new();
                row[c + 1] = format!("-{}", row[c + 1].trim_start());
            }
        }
        dropped.push(c);
    }

    dropped
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

    let mut rows: Vec<Vec<String>> = members
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

    let header = header_rows(rules, lines, &members);

    for row in &mut rows {
        for cell in row.iter_mut() {
            *cell = repair_cell(cell);
        }
    }
    let dropped = merge_sign_column(&mut rows, &boundaries, header, body_size);
    if !dropped.is_empty() {
        for row in &mut rows {
            let mut index = 0;
            row.retain(|_| {
                let keep = !dropped.contains(&index);
                index += 1;
                keep
            });
        }
    }

    for &i in &members {
        used[i] = true;
    }

    Some(Table {
        bbox: region,
        header_rows: header,
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

    for (i, count) in occupied.iter().enumerate() {
        let empty = *count <= ceiling;
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
                scripts: Vec::new(),
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
        let tables = detect(&page, &lines, 10.0, &[]);
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
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables[0].header_rows, 1);
    }

    #[test]
    fn two_rules_alone_give_no_header() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0), rule(170.0, 72.0, 400.0)]);
        let lines = vec![
            row(120.0, &[("a", 76.0, 90.0), ("b", 200.0, 214.0)]),
            row(140.0, &[("c", 76.0, 90.0), ("d", 200.0, 214.0)]),
        ];
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].header_rows, 0);
    }

    #[test]
    fn consumed_lines_are_reported() {
        let (page, lines) = booktabs();
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables[0].consumed, vec![0, 1, 2]);
    }

    /// A single rule is a section divider or a footnote separator, not a table.
    #[test]
    fn one_rule_is_not_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0)]);
        let lines = vec![row(120.0, &[("a", 76.0, 90.0), ("b", 200.0, 214.0)])];
        assert!(detect(&page, &lines, 10.0, &[]).is_empty());
    }

    /// One table's rules are one group, which is what they were before chains went plural.
    #[test]
    fn a_single_stack_of_rules_is_one_group() {
        let rules = [
            Rect::from_corners(72.0, 100.0, 400.0, 100.6),
            Rect::from_corners(72.0, 120.0, 400.0, 120.6),
            Rect::from_corners(72.0, 170.0, 400.0, 170.6),
        ];
        let groups = group_rules(&rules, 10.0, &[], &[]);
        assert_eq!(groups.len(), 1, "expected one group, got {groups:?}");
        assert_eq!(groups[0].len(), 3);
    }

    /// A rule that overlaps nothing joins nothing, however close in y it lands.
    #[test]
    fn a_rule_with_no_overlap_never_joins_a_group() {
        // The middle rule sits between the other two in y but shares no x with either. The
        // outer two must still find each other.
        let rules = [
            Rect::from_corners(50.0, 100.0, 280.0, 100.6),
            Rect::from_corners(320.0, 110.0, 540.0, 110.6),
            Rect::from_corners(50.0, 120.0, 280.0, 120.6),
        ];
        let groups = group_rules(&rules, 10.0, &[], &[]);
        assert_eq!(groups.len(), 1, "expected one group, got {groups:?}");
        assert_eq!(groups[0].len(), 2);
        assert!(
            groups[0].iter().all(|r| r.x1 <= 280.0),
            "the unrelated rule was pulled in: {:?}",
            groups[0]
        );
    }

    /// Two tables side by side interleave in y. Chaining only from the most recent rule made
    /// every group a singleton and dropped both tables.
    #[test]
    fn side_by_side_tables_both_survive() {
        let page = page_with(vec![
            rule(100.0, 60.0, 280.0),
            rule(102.0, 320.0, 540.0),
            rule(120.0, 60.0, 280.0),
            rule(122.0, 320.0, 540.0),
            rule(170.0, 60.0, 280.0),
            rule(172.0, 320.0, 540.0),
        ]);
        let lines = vec![
            row(115.0, &[("Model", 64.0, 100.0), ("Top-1", 200.0, 236.0)]),
            row(117.0, &[("Layers", 324.0, 366.0), ("Error", 460.0, 492.0)]),
            row(140.0, &[("A", 64.0, 78.0), ("21.4", 200.0, 226.0)]),
            row(142.0, &[("18", 324.0, 344.0), ("3.5", 460.0, 480.0)]),
            row(160.0, &[("B", 64.0, 78.0), ("20.1", 200.0, 226.0)]),
            row(162.0, &[("34", 324.0, 344.0), ("2.8", 460.0, 480.0)]),
        ];

        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 2, "expected two tables, got {tables:?}");
        assert_eq!(tables[0].rows[0], ["Model", "Top-1"]);
        assert_eq!(tables[1].rows[0], ["Layers", "Error"]);
        assert_eq!(tables[0].rows[2], ["B", "20.1"]);
        assert_eq!(tables[1].rows[2], ["34", "2.8"]);
    }

    /// `\frac` draws its bar as a filled rectangle, so a column of display fractions offers a
    /// stack of aligned "rules" with text between them — a table in every respect but width.
    #[test]
    fn a_stack_of_fraction_bars_is_not_a_table() {
        // Two numbered display equations, one above the other. The bars align, the denominators
        // sit between them, and the equation numbers off to the right read as a second column.
        let page = page_with(vec![rule(100.0, 150.0, 230.0), rule(140.0, 152.0, 232.0)]);
        let lines = vec![
            row(112.0, &[("x", 152.0, 160.0), ("(3)", 210.0, 228.0)]),
            row(136.0, &[("y", 154.0, 162.0), ("(4)", 212.0, 230.0)]),
        ];
        let tables = detect(&page, &lines, 10.0, &[]);
        assert!(tables.is_empty(), "fractions became a table: {tables:?}");
    }

    /// Rules with no horizontal relationship belong to different tables, or to none.
    #[test]
    fn unaligned_rules_do_not_form_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 200.0), rule(140.0, 400.0, 540.0)]);
        let lines = vec![row(120.0, &[("a", 76.0, 90.0), ("b", 420.0, 434.0)])];
        assert!(detect(&page, &lines, 10.0, &[]).is_empty());
    }

    /// A rule far above and one far below are page furniture, not a table's extent.
    #[test]
    fn rules_too_far_apart_are_not_one_table() {
        let page = page_with(vec![rule(60.0, 72.0, 540.0), rule(700.0, 72.0, 540.0)]);
        let lines = vec![
            row(300.0, &[("body", 76.0, 110.0), ("text", 200.0, 230.0)]),
            row(320.0, &[("more", 76.0, 110.0), ("text", 200.0, 230.0)]),
        ];
        assert!(detect(&page, &lines, 10.0, &[]).is_empty());
    }

    /// Centred prose between two rules has no column structure and must not become a table.
    #[test]
    fn single_column_content_is_not_a_table() {
        let page = page_with(vec![rule(100.0, 72.0, 400.0), rule(170.0, 72.0, 400.0)]);
        let lines = vec![
            row(120.0, &[("a line of running text here", 76.0, 390.0)]),
            row(140.0, &[("and another line of text", 76.0, 380.0)]),
        ];
        assert!(detect(&page, &lines, 10.0, &[]).is_empty());
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
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].columns(),
            3,
            "the spanning title erased the columns"
        );
        assert_eq!(tables[0].rows[1], ["A", "1.0", "2.0"]);
    }

    /// A plot's own lines, wide enough to pass for `\toprule` and `\bottomrule`, with the plot's
    /// legend text between them. Only the figure region tells them apart.
    fn plot_shaped_like_a_table() -> (PageRaw, Vec<Line>, Rect) {
        let page = page_with(vec![rule(100.0, 80.0, 280.0), rule(200.0, 80.0, 280.0)]);
        let lines = vec![
            row(130.0, &[("5 D Z", 90.0, 130.0), ("G D W D", 200.0, 250.0)]),
            row(160.0, &[("3 H D N", 90.0, 136.0), ("0 L V", 200.0, 240.0)]),
        ];
        (page, lines, Rect::from_corners(60.0, 60.0, 400.0, 300.0))
    }

    /// The rules inside a figure are the figure's, whatever they measure.
    #[test]
    fn rules_inside_a_figure_are_not_a_table() {
        let (page, lines, figure) = plot_shaped_like_a_table();
        assert_eq!(
            detect(&page, &lines, 10.0, &[]).len(),
            1,
            "the fixture must look exactly like a table without the figure"
        );

        let tables = detect(&page, &lines, 10.0, &[figure]);
        assert!(tables.is_empty(), "a plot became a table: {tables:?}");
    }

    /// Containment is in both axes. A table under a figure shares its column, and so lies inside
    /// the figure's *horizontal* span — testing x alone discarded ResNet's largest table.
    #[test]
    fn a_table_below_a_figure_survives_it() {
        let figure = Rect::from_corners(60.0, 60.0, 400.0, 300.0);
        let page = page_with(vec![rule(400.0, 80.0, 280.0), rule(470.0, 80.0, 280.0)]);
        let lines = vec![
            row(420.0, &[("plain", 90.0, 124.0), ("27.94", 200.0, 236.0)]),
            row(450.0, &[("ResNet", 90.0, 132.0), ("27.88", 200.0, 236.0)]),
        ];

        let tables = detect(&page, &lines, 10.0, &[figure]);
        assert_eq!(tables.len(), 1, "the figure above swallowed the table");
        assert_eq!(tables[0].rows[0], ["plain", "27.94"]);
    }

    /// ResNet's page 5: Table 1, then Figure 4, then Table 2. The two tables' rules pass
    /// `aligned`, and without the figure between them they fuse into one span of legend text.
    #[test]
    fn a_figure_between_two_tables_keeps_them_apart() {
        let page = page_with(vec![
            rule(100.0, 80.0, 420.0),
            rule(160.0, 80.0, 420.0),
            rule(400.0, 100.0, 280.0),
            rule(460.0, 100.0, 280.0),
        ]);
        let lines = vec![
            row(120.0, &[("layer", 90.0, 124.0), ("output", 300.0, 342.0)]),
            row(140.0, &[("conv1", 90.0, 124.0), ("112", 300.0, 322.0)]),
            row(420.0, &[("plain", 110.0, 144.0), ("27.94", 210.0, 246.0)]),
            row(440.0, &[("ResNet", 110.0, 152.0), ("27.88", 210.0, 246.0)]),
        ];
        assert_eq!(
            detect(&page, &lines, 10.0, &[]).len(),
            1,
            "without the figure the two must be indistinguishable from one table"
        );

        let figure = Rect::from_corners(80.0, 200.0, 420.0, 340.0);
        let tables = detect(&page, &lines, 10.0, &[figure]);
        assert_eq!(tables.len(), 2, "expected two tables, got {tables:?}");
        assert_eq!(tables[0].rows[0], ["layer", "output"]);
        assert_eq!(tables[1].rows[0], ["plain", "27.94"]);
    }

    /// Biology's page separator, above the running footer, spans the text block — so every
    /// `\frac` bar on the page lies wholly inside it and scores a perfect overlap against it.
    /// One rule that no table owns must not vouch for a stack of fraction bars.
    #[test]
    fn a_page_wide_rule_does_not_recruit_fraction_bars() {
        let page = page_with(vec![
            rule(100.0, 200.0, 212.0),
            rule(140.0, 200.0, 212.0),
            rule(180.0, 200.0, 212.0),
            rule(300.0, 50.0, 500.0),
        ]);
        let lines = vec![
            row(220.0, &[("dE", 60.0, 80.0), ("(1)", 300.0, 320.0)]),
            row(260.0, &[("dQ", 60.0, 80.0), ("(2)", 300.0, 320.0)]),
        ];

        let tables = detect(&page, &lines, 10.0, &[]);
        assert!(tables.is_empty(), "a footer rule made a table: {tables:?}");
    }

    /// The other side of that ratio: a `\cmidrule` really is much shorter than its `\toprule`,
    /// and still belongs. BERT's Table 8 sets the tightest real pairing in the corpus.
    #[test]
    fn a_cmidrule_still_belongs_to_its_table() {
        let page = page_with(vec![
            rule(100.0, 100.0, 320.0),
            rule(130.0, 100.0, 179.0),
            rule(200.0, 100.0, 320.0),
        ]);
        let lines = vec![
            row(120.0, &[("Masking", 110.0, 158.0), ("Dev", 240.0, 264.0)]),
            row(150.0, &[("80%", 110.0, 134.0), ("84.2", 240.0, 266.0)]),
            row(180.0, &[("100%", 110.0, 140.0), ("84.3", 240.0, 266.0)]),
        ];

        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1, "the cmidrule split its own table");
        assert_eq!(tables[0].header_rows, 1);
        assert_eq!(tables[0].rows[1], ["80%", "84.2"]);
    }

    /// ResNet's page 11 in miniature: two tables stacked in the same measure, the upper one's
    /// rules nested wholly inside the lower one's, and 29 pt of white between the pair. Every
    /// geometric test passes — the overlap is a perfect 1.0, the width ratio 0.64, the spacing a
    /// tenth of [`MAX_RULE_SPACING`] — so only what is written between them tells them apart.
    ///
    /// `middle` is the line that lands in that gap.
    fn stacked_tables(middle: &[(&str, f32, f32)]) -> (PageRaw, Vec<Line>) {
        let page = page_with(vec![
            rule(72.0, 139.8, 455.4),
            rule(96.0, 139.8, 455.4),
            rule(178.0, 139.8, 455.4),
            rule(208.0, 51.7, 543.5),
            rule(218.6, 51.7, 543.5),
            rule(250.1, 51.7, 543.5),
        ]);
        let lines = vec![
            row(90.0, &[("training", 144.0, 190.0), ("COCO", 300.0, 336.0)]),
            row(120.0, &[("+context", 144.0, 190.0), ("51.1", 300.0, 336.0)]),
            row(160.0, &[("ensemble", 144.0, 190.0), ("59.0", 300.0, 336.0)]),
            row(190.0, middle),
            row(215.5, &[("system", 56.0, 102.0), ("mAP", 300.0, 336.0)]),
            row(226.1, &[("baseline", 56.0, 102.0), ("73.2", 300.0, 336.0)]),
            row(
                236.4,
                &[("baseline+++", 56.0, 102.0), ("85.6", 300.0, 336.0)],
            ),
        ];
        (page, lines)
    }

    /// A line of body prose in that gap leaves the two welded, which is what they were before
    /// captions were consulted. The pair is genuinely indistinguishable by geometry.
    #[test]
    fn stacked_tables_with_nothing_between_them_stay_one() {
        let (page, lines) = stacked_tables(&[
            ("by", 130.0, 142.0),
            ("over", 144.0, 172.0),
            ("2", 174.0, 180.0),
            ("points", 182.0, 216.0),
            ("(Table", 218.0, 250.0),
            ("9).", 252.0, 266.0),
        ]);
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(
            tables.len(),
            1,
            "the fixture must weld without a caption, or it proves nothing: {tables:?}"
        );
        assert_eq!(tables[0].rows.len(), 7);
    }

    /// The same stack, with `Table 9.` where the prose was. The caption parts them.
    #[test]
    fn a_caption_parts_two_stacked_tables() {
        let (page, lines) = stacked_tables(&[
            ("Table", 130.0, 158.0),
            ("9.", 160.0, 170.0),
            ("Object", 172.0, 205.0),
            ("detection", 207.0, 260.0),
        ]);
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 2, "expected two tables, got {tables:?}");

        assert_eq!(tables[0].rows.len(), 3);
        assert_eq!(tables[0].rows[0], ["training", "COCO"]);
        assert_eq!(tables[0].rows[2], ["ensemble", "59.0"]);
        assert_eq!(tables[1].rows.len(), 3);
        assert_eq!(tables[1].rows[0], ["system", "mAP"]);
        assert_eq!(tables[1].rows[2], ["baseline+++", "85.6"]);
    }

    /// REVTeX and IEEEtran number their tables in Roman.
    #[test]
    fn a_roman_numbered_caption_parts_them_too() {
        let (page, lines) = stacked_tables(&[
            ("TABLE", 130.0, 162.0),
            ("II", 164.0, 174.0),
            ("Symmetry", 176.0, 226.0),
            ("labels", 228.0, 260.0),
        ]);
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 2, "expected two tables, got {tables:?}");
    }

    /// A paper names its own tables constantly, and a sentence that mentions one is not its
    /// caption. Only the head of the line counts.
    #[test]
    fn a_mention_mid_sentence_is_not_a_caption() {
        let (page, lines) = stacked_tables(&[
            ("shown", 130.0, 164.0),
            ("in", 166.0, 176.0),
            ("Table", 178.0, 206.0),
            ("9", 208.0, 214.0),
            ("above", 216.0, 248.0),
        ]);
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(
            tables.len(),
            1,
            "a mention split a table that geometry said was one: {tables:?}"
        );
    }

    /// Nor is the bare word. `Table` opens plenty of sentences that are not captions, and it is
    /// the number after it that makes one.
    #[test]
    fn the_word_alone_is_not_a_caption() {
        let (page, lines) = stacked_tables(&[
            ("Table", 130.0, 158.0),
            ("entries", 160.0, 200.0),
            ("are", 202.0, 220.0),
            ("means", 222.0, 258.0),
        ]);
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1, "a bare mention split a table: {tables:?}");
    }

    /// A caption belongs to the column it sits in. One in the *other* column must not reach
    /// across and part a table that has nothing to do with it.
    #[test]
    fn a_caption_in_the_other_column_parts_nothing() {
        let page = page_with(vec![
            rule(100.0, 60.0, 280.0),
            rule(120.0, 60.0, 280.0),
            rule(200.0, 60.0, 280.0),
        ]);
        let lines = vec![
            row(115.0, &[("Model", 64.0, 100.0), ("Top-1", 200.0, 236.0)]),
            row(140.0, &[("A", 64.0, 100.0), ("21.4", 200.0, 236.0)]),
            row(180.0, &[("B", 64.0, 100.0), ("20.1", 200.0, 236.0)]),
            row(
                150.0,
                &[
                    ("Table", 320.0, 348.0),
                    ("7.", 350.0, 360.0),
                    ("Ablations", 362.0, 412.0),
                ],
            ),
        ];

        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(
            tables.len(),
            1,
            "the next column's caption split this one's table: {tables:?}"
        );
        assert_eq!(tables[0].rows.len(), 3);
    }

    /// A `\cmidrule` is the case the split must never touch: a short rule inside a table, with
    /// the table's own rows either side of it and no caption anywhere between.
    #[test]
    fn a_caption_below_a_table_does_not_reach_back_into_it() {
        let page = page_with(vec![
            rule(100.0, 100.0, 320.0),
            rule(130.0, 100.0, 179.0),
            rule(200.0, 100.0, 320.0),
        ]);
        let lines = vec![
            row(120.0, &[("Masking", 110.0, 158.0), ("Dev", 240.0, 264.0)]),
            row(150.0, &[("80%", 110.0, 158.0), ("84.2", 240.0, 264.0)]),
            row(180.0, &[("100%", 110.0, 158.0), ("84.3", 240.0, 264.0)]),
            row(
                215.0,
                &[
                    ("Table", 100.0, 128.0),
                    ("8:", 130.0, 142.0),
                    ("Ablation", 144.0, 190.0),
                ],
            ),
        ];

        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1, "the caption below split the table above");
        assert_eq!(tables[0].rows.len(), 3);
        assert_eq!(tables[0].header_rows, 1);
    }

    /// A grid drawn cell by cell: every border is three abutting fragments, none wide
    /// enough to pass `MIN_TABLE_RULE_WIDTH` on its own. Coalescing must recover it.
    #[test]
    fn a_grid_of_border_fragments_is_detected() {
        let page = page_with(vec![
            rule(100.0, 72.0, 180.0),
            rule(100.0, 179.8, 290.0),
            rule(100.0, 289.8, 400.0),
            rule(120.0, 72.0, 180.0),
            rule(120.0, 179.8, 290.0),
            rule(120.0, 289.8, 400.0),
            rule(170.0, 72.0, 180.0),
            rule(170.0, 179.8, 290.0),
            rule(170.0, 289.8, 400.0),
        ]);
        let lines = vec![
            row(115.0, &[("name", 80.0, 110.0), ("value", 200.0, 230.0)]),
            row(140.0, &[("alpha", 80.0, 112.0), ("3.2", 200.0, 220.0)]),
            row(160.0, &[("beta", 80.0, 108.0), ("4.7", 200.0, 220.0)]),
        ];
        let tables = detect(&page, &lines, 10.0, &[]);
        assert_eq!(tables.len(), 1, "the fragmented grid was lost: {tables:?}");
        assert_eq!(tables[0].rows.len(), 3);
        assert_eq!(tables[0].header_rows, 1);
    }

    /// Fragments separated by real gaps — a dashed rule, side-by-side tables — stay apart.
    #[test]
    fn fragments_with_real_gaps_do_not_coalesce() {
        let coalesced = coalesce_fragments(vec![
            Rect::from_corners(72.0, 100.0, 100.0, 100.6),
            Rect::from_corners(110.0, 100.0, 138.0, 100.6),
        ]);
        assert_eq!(coalesced.len(), 2, "a 10pt gap was bridged");
    }

    /// The sign variants a cell can carry, and the ranges and placeholders it must keep.
    #[test]
    fn a_detached_sign_rejoins_its_number() {
        assert_eq!(repair_cell("\u{2212} 43.801"), "-43.801");
        assert_eq!(
            repair_cell("156.907 (\u{2212} 43.801)"),
            "156.907 (-43.801)"
        );
        assert_eq!(repair_cell("\u{2013}3.2"), "-3.2");
        assert_eq!(repair_cell("- 0.8"), "-0.8");
        assert_eq!(repair_cell("12 \u{2013} 15"), "12 \u{2013} 15");
        assert_eq!(repair_cell("3\u{2013}5"), "3\u{2013}5");
        assert_eq!(repair_cell("-"), "-");
        assert_eq!(repair_cell("Mann\u{2013}Whitney"), "Mann\u{2013}Whitney");
        // A sign after a list separator belongs to the next value, not a range.
        assert_eq!(
            repair_cell("[\u{2212}141.860, \u{2212} 76.891]"),
            "[-141.860, -76.891]"
        );
    }

    /// A column of bare signs beside a number column folds into it.
    #[test]
    fn a_sign_column_folds_into_its_numbers() {
        let mut rows = vec![
            vec!["CI".to_owned(), String::new(), "x".to_owned()],
            vec!["a".to_owned(), "-".to_owned(), "1.1".to_owned()],
            vec!["b".to_owned(), String::new(), "2.5".to_owned()],
            vec!["c".to_owned(), "-".to_owned(), "7.4".to_owned()],
        ];
        let boundaries = [(60.0, 90.0), (100.0, 103.0), (110.0, 140.0)];
        let dropped = merge_sign_column(&mut rows, &boundaries, 1, 10.0);
        assert_eq!(dropped, [1]);
        assert_eq!(rows[1][2], "-1.1");
        assert_eq!(rows[2][2], "2.5");
        assert_eq!(rows[3][2], "-7.4");
    }

    /// A placeholder-dash column has a header of its own, and folding it would invent
    /// negative numbers — it must survive untouched.
    #[test]
    fn a_placeholder_dash_column_is_not_a_sign_column() {
        let mut rows = vec![
            vec!["name".to_owned(), "run".to_owned(), "x".to_owned()],
            vec!["a".to_owned(), "-".to_owned(), "1.1".to_owned()],
            vec!["b".to_owned(), "-".to_owned(), "2.5".to_owned()],
        ];
        let boundaries = [(60.0, 90.0), (100.0, 103.0), (110.0, 140.0)];
        let dropped = merge_sign_column(&mut rows, &boundaries, 1, 10.0);
        assert!(dropped.is_empty(), "a headed column was folded away");
        assert_eq!(rows[1][2], "1.1");
    }

    /// A sign column a full column gap from its digits is a column, not a stranded sign.
    #[test]
    fn a_distant_dash_column_is_not_merged() {
        let mut rows = vec![
            vec!["h".to_owned(), String::new(), "x".to_owned()],
            vec!["a".to_owned(), "-".to_owned(), "1.1".to_owned()],
            vec!["b".to_owned(), "-".to_owned(), "2.5".to_owned()],
        ];
        let boundaries = [(60.0, 90.0), (100.0, 103.0), (130.0, 160.0)];
        let dropped = merge_sign_column(&mut rows, &boundaries, 1, 10.0);
        assert!(dropped.is_empty(), "a 27pt gap was treated as adjacency");
    }
}
