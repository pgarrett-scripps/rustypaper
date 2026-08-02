//! Glyphs to lines to words.
//!
//! This is the first interpretive pass. Three things make it harder than it looks on scientific
//! PDFs:
//!
//! * **Sub- and superscripts sit on their own baseline.** Naive baseline clustering makes `x_i`
//!   into three lines. They have to be absorbed into their host line, and knowing which glyphs
//!   are scripts is exactly the signal maths reconstruction needs later, so the classification
//!   is recorded rather than thrown away.
//! * **LaTeX frequently emits no space glyphs at all.** Word boundaries come from measuring
//!   inter-glyph gaps against the font size, not from looking for `' '`.
//! * **Lines can span columns.** Both columns of a two-column paper usually sit on the same
//!   baseline grid, so a baseline cluster is often *two* lines of text. Splitting them needs the
//!   gutter positions, which the layout pass finds; [`split_at_gutters`] applies the result.

use crate::ir::{Glyph, PageRaw, Rect};

/// Fraction of the font size within which two baselines count as the same line.
const BASELINE_TOLERANCE: f32 = 0.28;

/// A glyph must be at most this fraction of the host line's size to be considered a script.
/// TeX sets scripts at 70% of the body size and second-order scripts at 50%.
const SCRIPT_MAX_SIZE_RATIO: f32 = 0.85;

/// A script cluster must overlap its host line vertically by at least this fraction of its own
/// height, which is what separates a superscript from the line above it.
const SCRIPT_MIN_OVERLAP: f32 = 0.25;

/// Fallback inter-glyph gap, as a fraction of font size, that implies a word break. Only used
/// when a line does not have enough gaps to infer its own threshold.
const WORD_GAP_RATIO: f32 = 0.19;

/// No gap below this fraction of the font size is ever a word break, whatever the line's own
/// distribution says. This is what stops a single-word line from being split at its widest
/// kern pair.
const MIN_WORD_GAP_RATIO: f32 = 0.10;

/// The inferred word-gap class must be this many times wider than the intra-word class for the
/// split to be believed.
const GAP_CLASS_SEPARATION: f32 = 2.5;

/// Beyond this many degrees off horizontal, a glyph is not part of running text.
const MAX_TEXT_ANGLE_DEGREES: f32 = 5.0;

/// Where a glyph sits relative to its line's baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Normal,
    Superscript,
    Subscript,
}

/// One glyph, placed in a line.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    /// Index into [`PageRaw::glyphs`].
    pub index: usize,
    pub script: Script,
    /// A whitespace glyph preceded this one, so a word starts here.
    pub break_before: bool,
}

/// A run of glyphs sharing a baseline, in left-to-right order.
#[derive(Debug, Clone)]
pub struct Line {
    pub bbox: Rect,
    /// The dominant baseline: that of the non-script glyphs.
    pub baseline: f32,
    /// The dominant font size, taken from the widest run of same-size glyphs.
    pub size: f32,
    pub glyphs: Vec<Placed>,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone)]
pub struct Word {
    pub bbox: Rect,
    pub text: String,
    /// Range within [`Line::glyphs`].
    pub start: usize,
    pub end: usize,
}

impl Line {
    /// The line's text, with word breaks as single spaces.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (i, word) in self.words.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&word.text);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// Builds lines for a page. The result is in top-to-bottom order and may contain lines that span
/// several columns; call [`split_at_gutters`] once the gutters are known.
pub fn build_lines(page: &PageRaw) -> Vec<Line> {
    let mut order: Vec<usize> = (0..page.glyphs.len())
        .filter(|&i| is_layout_relevant(&page.glyphs[i]))
        .collect();

    // Sort by baseline, then left to right. `total_cmp` because glyph coordinates are f32 and a
    // NaN from a malformed PDF must not poison the sort.
    order.sort_by(|&a, &b| {
        let (ga, gb) = (&page.glyphs[a], &page.glyphs[b]);
        ga.origin
            .y
            .total_cmp(&gb.origin.y)
            .then(ga.origin.x.total_cmp(&gb.origin.x))
    });

    let clusters = cluster_by_baseline(page, &order);
    let mut lines = absorb_scripts(page, clusters);

    for line in &mut lines {
        line.glyphs.sort_by(|a, b| {
            page.glyphs[a.index]
                .origin
                .x
                .total_cmp(&page.glyphs[b.index].origin.x)
        });
        compact_spaces(page, line);
        recompute(page, line);
        segment_words(page, line);
    }

    lines.sort_by(|a, b| {
        a.baseline
            .total_cmp(&b.baseline)
            .then(a.bbox.x0.total_cmp(&b.bbox.x0))
    });
    lines
}

/// Glyphs that contribute to running text.
///
/// Whitespace glyphs are kept at this stage — they are the word boundaries, and
/// [`compact_spaces`] converts them into flags once the line is ordered.
///
/// Rotated glyphs are excluded. Every arXiv preprint carries a sideways stamp down the left
/// margin, and interleaving it with body text by baseline produces exactly the garbage that
/// makes naive extractors unusable. Sideways table headers are excluded by the same rule and are
/// not yet handled; [`rotated_glyphs`] makes what was set aside inspectable rather than silent.
fn is_layout_relevant(glyph: &Glyph) -> bool {
    is_horizontal(glyph)
}

fn is_space(glyph: &Glyph) -> bool {
    match glyph.text {
        crate::ir::GlyphText::Char(c) => c.is_whitespace(),
        crate::ir::GlyphText::Expanded(_) => false,
    }
}

fn is_horizontal(glyph: &Glyph) -> bool {
    let degrees = glyph.angle.to_degrees();
    degrees.is_finite()
        && degrees
            .rem_euclid(360.0)
            .min(360.0 - degrees.rem_euclid(360.0))
            <= MAX_TEXT_ANGLE_DEGREES
}

/// Indices of glyphs that [`build_lines`] set aside because they are not horizontal.
pub fn rotated_glyphs(page: &PageRaw) -> Vec<usize> {
    (0..page.glyphs.len())
        .filter(|&i| !page.glyphs[i].generated && !is_horizontal(&page.glyphs[i]))
        .collect()
}

/// Groups glyphs whose baselines agree, assigning each to the nearest open cluster rather than
/// chaining, so a long run of slightly drifting baselines cannot merge two lines.
fn cluster_by_baseline(page: &PageRaw, order: &[usize]) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut baselines: Vec<f32> = Vec::new();

    for &index in order {
        let glyph = &page.glyphs[index];
        let tolerance = (glyph.size * BASELINE_TOLERANCE).max(0.5);

        // The sort means only the most recent clusters can still be in range.
        let candidate = baselines
            .iter()
            .enumerate()
            .rev()
            .take(8)
            .filter(|(_, &b)| (b - glyph.origin.y).abs() <= tolerance)
            .min_by(|(_, a), (_, b)| {
                (*a - glyph.origin.y)
                    .abs()
                    .total_cmp(&(*b - glyph.origin.y).abs())
            })
            .map(|(i, _)| i);

        match candidate {
            Some(i) => clusters[i].push(index),
            None => {
                clusters.push(vec![index]);
                baselines.push(glyph.origin.y);
            }
        }
    }

    clusters
}

/// Merges script clusters into the lines they belong to.
///
/// A superscript run forms its own baseline cluster, so `x^2 + y^2` yields three clusters. Each
/// small cluster is offered to the nearest larger cluster it overlaps vertically and abuts
/// horizontally; if none takes it, it stands as a line in its own right.
fn absorb_scripts(page: &PageRaw, clusters: Vec<Vec<usize>>) -> Vec<Line> {
    let mut lines: Vec<Line> = clusters
        .into_iter()
        .map(|glyphs| {
            let mut line = Line {
                bbox: bounds(page, &glyphs),
                baseline: 0.0,
                size: 0.0,
                glyphs: glyphs
                    .into_iter()
                    .map(|index| Placed {
                        index,
                        script: Script::Normal,
                        break_before: false,
                    })
                    .collect(),
                words: Vec::new(),
            };
            recompute(page, &mut line);
            line
        })
        .collect();

    // Largest first, so a script is offered to the body line before any smaller neighbour.
    let mut by_size: Vec<usize> = (0..lines.len()).collect();
    by_size.sort_by(|&a, &b| lines[b].size.total_cmp(&lines[a].size));

    let mut absorbed = vec![false; lines.len()];
    let mut moves: Vec<(usize, usize)> = Vec::new();

    for &candidate in &by_size {
        if absorbed[candidate] {
            continue;
        }
        let Some(host) = find_host(&lines, &absorbed, candidate) else {
            continue;
        };
        absorbed[candidate] = true;
        moves.push((candidate, host));
    }

    // Apply after the search so hosts are matched against original geometry.
    for (candidate, host) in moves {
        let taken = std::mem::take(&mut lines[candidate].glyphs);
        let host_baseline = lines[host].baseline;
        for mut placed in taken {
            placed.script = if page.glyphs[placed.index].origin.y < host_baseline {
                Script::Superscript
            } else {
                Script::Subscript
            };
            lines[host].glyphs.push(placed);
        }
    }

    lines.retain(|line| !line.is_empty());
    lines
}

/// Finds the line that a small cluster is a script of, if any.
fn find_host(lines: &[Line], absorbed: &[bool], candidate: usize) -> Option<usize> {
    let small = &lines[candidate];
    if small.bbox.height() <= 0.0 {
        return None;
    }

    lines
        .iter()
        .enumerate()
        .filter(|&(i, host)| {
            i != candidate
                && !absorbed[i]
                && !host.is_empty()
                && small.size <= host.size * SCRIPT_MAX_SIZE_RATIO
                && host.bbox.y_overlap(&small.bbox) >= small.bbox.height() * SCRIPT_MIN_OVERLAP
                // Must sit against the host's text, not merely share a band of the page: a
                // superscript is within roughly a quad of the host on one side or inside it.
                && host.bbox.x_overlap(&small.bbox) > -host.size * 1.5
        })
        .min_by(|(_, a), (_, b)| {
            let da = (a.baseline - small.baseline).abs();
            let db = (b.baseline - small.baseline).abs();
            da.total_cmp(&db)
        })
        .map(|(i, _)| i)
}

fn bounds(page: &PageRaw, glyphs: &[usize]) -> Rect {
    glyphs
        .iter()
        .map(|&i| page.glyphs[i].bbox)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::from_corners(0.0, 0.0, 0.0, 0.0))
}

/// Recomputes a line's bbox, dominant size and baseline from its glyphs.
///
/// "Dominant" is by total glyph width rather than count, so a line of body text with a couple of
/// large drop capitals or a footnote marker keeps the body size.
fn recompute(page: &PageRaw, line: &mut Line) {
    if line.glyphs.is_empty() {
        return;
    }

    let indices: Vec<usize> = line.glyphs.iter().map(|p| p.index).collect();
    line.bbox = bounds(page, &indices);

    let mut buckets: Vec<(f32, f32)> = Vec::new();
    for &i in &indices {
        let glyph = &page.glyphs[i];
        let weight = glyph.bbox.width().max(0.1);
        match buckets
            .iter_mut()
            .find(|(size, _)| (*size - glyph.size).abs() < 0.25)
        {
            Some((_, w)) => *w += weight,
            None => buckets.push((glyph.size, weight)),
        }
    }
    line.size = buckets
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(size, _)| *size)
        .unwrap_or(0.0);

    // The baseline is that of the dominant-size glyphs; scripts must not drag it.
    let mut baselines: Vec<f32> = line
        .glyphs
        .iter()
        .filter(|p| (page.glyphs[p.index].size - line.size).abs() < 0.25)
        .map(|p| page.glyphs[p.index].origin.y)
        .collect();
    if baselines.is_empty() {
        baselines = line
            .glyphs
            .iter()
            .map(|p| page.glyphs[p.index].origin.y)
            .collect();
    }
    baselines.sort_by(f32::total_cmp);
    line.baseline = baselines[baselines.len() / 2];
}

/// Splits lines wherever they cross a gutter, so each resulting line belongs to one column.
pub fn split_at_gutters(page: &PageRaw, lines: Vec<Line>, gutters: &[(f32, f32)]) -> Vec<Line> {
    if gutters.is_empty() {
        return lines;
    }

    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let mut current: Vec<Placed> = Vec::new();
        let mut pieces: Vec<Vec<Placed>> = Vec::new();

        for placed in line.glyphs {
            let x = page.glyphs[placed.index].bbox.center_x();
            let crossed = current.last().is_some_and(|prev| {
                let px = page.glyphs[prev.index].bbox.center_x();
                gutters.iter().any(|&(g0, g1)| px < g0 && x > g1)
            });
            if crossed && !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            current.push(placed);
        }
        if !current.is_empty() {
            pieces.push(current);
        }

        for glyphs in pieces {
            let mut piece = Line {
                bbox: line.bbox,
                baseline: line.baseline,
                size: line.size,
                glyphs,
                words: Vec::new(),
            };
            recompute(page, &mut piece);
            segment_words(page, &mut piece);
            out.push(piece);
        }
    }

    out.sort_by(|a, b| {
        a.baseline
            .total_cmp(&b.baseline)
            .then(a.bbox.x0.total_cmp(&b.bbox.x0))
    });
    out
}

/// Removes whitespace glyphs from an ordered line, recording where they were.
///
/// A whitespace glyph is a word boundary stated outright, so it beats any gap measurement. That
/// matters more than it sounds: across the corpus, LaTeX output contains almost no real space
/// characters (9 to 236 per document) while pdfium *generates* 5 000 to 8 500 per document from
/// the font's advance widths — information the public API does not otherwise expose. Measured
/// against those, pure gap analysis mis-segments kerned pairs such as `learning framework`,
/// where the ink gap is 0.18 em against a 0.30 em typical space.
fn compact_spaces(page: &PageRaw, line: &mut Line) {
    let mut compacted: Vec<Placed> = Vec::with_capacity(line.glyphs.len());
    let mut pending_break = false;

    for placed in line.glyphs.drain(..) {
        if is_space(&page.glyphs[placed.index]) {
            // A leading space says nothing about word structure within the line.
            pending_break = !compacted.is_empty();
            continue;
        }
        compacted.push(Placed {
            break_before: pending_break,
            ..placed
        });
        pending_break = false;
    }

    line.glyphs = compacted;
}

/// Splits a line into words.
///
/// Whitespace glyphs decide it where they exist. Where a document emits none at all, the
/// fallback measures inter-glyph gaps against a threshold inferred from the line's own
/// distribution — see [`word_gap_threshold`].
fn segment_words(page: &PageRaw, line: &mut Line) {
    line.words.clear();
    if line.glyphs.is_empty() {
        return;
    }

    let marked = line.glyphs.iter().any(|p| p.break_before);
    let ratios: Vec<f32> = (1..line.glyphs.len())
        .map(|i| gap_ratio(page, line, i))
        .collect();
    let threshold = word_gap_threshold(&ratios);

    let mut start = 0usize;
    let mut buf = [0u8; 4];
    let mut text = String::new();

    for i in 0..line.glyphs.len() {
        let split = if marked {
            line.glyphs[i].break_before
        } else {
            i > 0 && ratios[i - 1] > threshold
        };
        if split && i > 0 {
            push_word(page, line, start, i, std::mem::take(&mut text));
            start = i;
        }
        let glyph = &page.glyphs[line.glyphs[i].index];
        text.push_str(page.glyph_str(glyph, &mut buf));
    }
    push_word(page, line, start, line.glyphs.len(), text);
}

/// Gap between glyph `i` and its predecessor, as a fraction of font size.
///
/// Normalised by the smaller of the two sizes so that a gap before a superscript is judged at
/// the superscript's scale rather than the body's.
fn gap_ratio(page: &PageRaw, line: &Line, i: usize) -> f32 {
    let prev = &page.glyphs[line.glyphs[i - 1].index];
    let curr = &page.glyphs[line.glyphs[i].index];
    let scale = prev.size.min(curr.size).max(1.0);
    (curr.bbox.x0 - prev.bbox.x1) / scale
}

/// Finds the gap ratio separating intra-word from inter-word spacing on one line.
///
/// Otsu's method: the split maximising between-class variance. Three guards keep it honest — the
/// classes must be well separated, the boundary must clear an absolute floor, and the result is
/// clamped to at most the fixed default.
///
/// That last clamp matters because gaps are not reliably two-class. An author line reads
/// `Kaiming He   Xiangyu Zhang   Shaoqing Ren`, which has three: kerning, word spaces, and the
/// wide separators between authors. Otsu splits at the widest separation and would swallow the
/// word spaces, giving `KaimingHe`. Allowing inference only to *lower* the threshold keeps the
/// gain on kerned spaces without that risk.
fn word_gap_threshold(ratios: &[f32]) -> f32 {
    let fallback = WORD_GAP_RATIO;
    if ratios.len() < 4 {
        return fallback;
    }

    let mut sorted: Vec<f32> = ratios.iter().copied().filter(|r| r.is_finite()).collect();
    if sorted.len() < 4 {
        return fallback;
    }
    sorted.sort_by(f32::total_cmp);

    let n = sorted.len();
    let total: f32 = sorted.iter().sum();
    let mut running = 0.0;
    let mut best = (f32::NEG_INFINITY, 0usize);

    for i in 1..n {
        running += sorted[i - 1];
        let (w0, w1) = (i as f32, (n - i) as f32);
        let delta = running / w0 - (total - running) / w1;
        let between = w0 * w1 * delta * delta;
        if between > best.0 {
            best = (between, i);
        }
    }

    let split = best.1;
    if split == 0 || split == n {
        return fallback;
    }

    let lower_mean = sorted[..split].iter().sum::<f32>() / split as f32;
    let upper_mean = sorted[split..].iter().sum::<f32>() / (n - split) as f32;

    // An all-one-word line has no upper class to find; its "split" separates kerning from
    // slightly wider kerning, and both guards below reject it.
    if sorted[split] < MIN_WORD_GAP_RATIO || upper_mean < lower_mean.abs() * GAP_CLASS_SEPARATION {
        return fallback;
    }

    // Midway between the two classes, so borderline gaps fall on the safer side.
    ((sorted[split - 1] + sorted[split]) * 0.5).clamp(MIN_WORD_GAP_RATIO, fallback)
}

fn push_word(page: &PageRaw, line: &mut Line, start: usize, end: usize, text: String) {
    if start >= end || text.is_empty() {
        return;
    }
    let indices: Vec<usize> = line.glyphs[start..end].iter().map(|p| p.index).collect();
    line.words.push(Word {
        bbox: bounds(page, &indices),
        text,
        start,
        end,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FontFlags, FontId, GlyphText, Point, Rgba};

    /// Places a glyph with its baseline origin at `(x, baseline)` and a plausible ink box.
    fn glyph(c: char, x: f32, baseline: f32, size: f32) -> Glyph {
        let width = size * 0.5;
        Glyph {
            text: GlyphText::Char(c),
            bbox: Rect::from_corners(x, baseline - size * 0.7, x + width, baseline),
            origin: Point::new(x, baseline),
            font: FontId(0),
            size,
            angle: 0.0,
            flags: FontFlags::default(),
            color: Rgba::BLACK,
            generated: false,
        }
    }

    fn page_of(glyphs: Vec<Glyph>) -> PageRaw {
        PageRaw {
            index: 0,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            glyphs,
            ..Default::default()
        }
    }

    /// Lays out a string on one baseline with no gaps, advancing by the glyph width.
    fn run(text: &str, x0: f32, baseline: f32, size: f32) -> Vec<Glyph> {
        let mut x = x0;
        text.chars()
            .map(|c| {
                let g = glyph(c, x, baseline, size);
                x += size * 0.5;
                g
            })
            .collect()
    }

    #[test]
    fn one_run_is_one_line_and_one_word() {
        let page = page_of(run("hello", 72.0, 100.0, 10.0));
        let lines = build_lines(&page);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "hello");
        assert_eq!(lines[0].size, 10.0);
        assert_eq!(lines[0].baseline, 100.0);
    }

    #[test]
    fn gaps_split_words_without_space_glyphs() {
        // Two runs separated by a 3pt gap at 10pt size: 0.3em, comfortably a space.
        let mut glyphs = run("hello", 72.0, 100.0, 10.0);
        glyphs.extend(run("world", 72.0 + 5.0 * 5.0 + 3.0, 100.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].words.len(), 2, "expected two words");
        assert_eq!(lines[0].text(), "hello world");
    }

    #[test]
    fn kerning_does_not_split_words() {
        // A 0.5pt gap at 10pt is 0.05em: kerning, not a space.
        let mut glyphs = run("ab", 72.0, 100.0, 10.0);
        glyphs.extend(run("cd", 72.0 + 10.0 + 0.5, 100.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines[0].words.len(), 1);
        assert_eq!(lines[0].text(), "abcd");
    }

    #[test]
    fn separate_baselines_are_separate_lines() {
        let mut glyphs = run("first", 72.0, 100.0, 10.0);
        glyphs.extend(run("second", 72.0, 112.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text(), "first");
        assert_eq!(lines[1].text(), "second");
    }

    #[test]
    fn superscript_joins_its_host_line() {
        // `x` at 10pt, then a 7pt `2` raised by 4pt: TeX's superscript geometry.
        let mut glyphs = run("x", 72.0, 100.0, 10.0);
        glyphs.extend(run("2", 77.0, 96.0, 7.0));
        glyphs.extend(run("+y", 81.5, 100.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 1, "superscript must not become its own line");
        assert_eq!(lines[0].text(), "x2+y");
        assert_eq!(
            lines[0].size, 10.0,
            "a script must not become the line size"
        );
        assert_eq!(lines[0].baseline, 100.0);

        let scripts: Vec<Script> = lines[0].glyphs.iter().map(|p| p.script).collect();
        assert_eq!(scripts[1], Script::Superscript);
        assert_eq!(scripts[0], Script::Normal);
    }

    #[test]
    fn subscript_is_distinguished_from_superscript() {
        let mut glyphs = run("x", 72.0, 100.0, 10.0);
        glyphs.extend(run("i", 77.0, 102.5, 7.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].glyphs[1].script, Script::Subscript);
    }

    #[test]
    fn a_small_line_far_below_is_not_a_script() {
        // A footnote in 7pt, well clear of the body line: its own line.
        let mut glyphs = run("body", 72.0, 100.0, 10.0);
        glyphs.extend(run("footnote", 72.0, 140.0, 7.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text(), "footnote");
    }

    #[test]
    fn generated_and_whitespace_glyphs_are_ignored() {
        let mut glyphs = run("ab", 72.0, 100.0, 10.0);
        let mut synthetic = glyph(' ', 82.0, 100.0, 10.0);
        synthetic.generated = true;
        glyphs.push(synthetic);
        glyphs.push(glyph(' ', 87.0, 100.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].glyphs.len(), 2);
        assert_eq!(lines[0].text(), "ab");
    }

    #[test]
    fn gutters_split_a_two_column_baseline() {
        // Left column at x=72, right column at x=320, gutter between 290 and 310.
        let mut glyphs = run("left", 72.0, 100.0, 10.0);
        glyphs.extend(run("right", 320.0, 100.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(
            lines.len(),
            1,
            "both columns share a baseline before splitting"
        );

        let split = split_at_gutters(&page, lines, &[(290.0, 310.0)]);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].text(), "left");
        assert_eq!(split[1].text(), "right");
        assert!(split[0].bbox.x1 < split[1].bbox.x0);
    }

    #[test]
    fn empty_page_yields_no_lines() {
        let page = page_of(Vec::new());
        assert!(build_lines(&page).is_empty());
    }
}
