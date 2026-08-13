//! Glyphs to lines to words.
//!
//! This is the first interpretive pass. Three things make it harder than it looks on scientific
//! PDFs:
//!
//! * **Sub- and superscripts sit on their own baseline.** Naive baseline clustering makes `x_i`
//!   into three lines. They have to be absorbed into their host line, and knowing which glyphs
//!   are scripts is exactly the signal maths reconstruction needs later, so the classification
//!   is recorded rather than thrown away.
//! * **LaTeX frequently emits no space glyphs at all.** Where the backend marks word breaks —
//!   reading them from the content stream or synthesising them — those marks decide the
//!   boundaries outright. Measuring inter-glyph gaps is only the fallback for a line that carries
//!   no marks, because a gap after a narrow letter looks exactly like a space (`I mage`).
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

/// A script's baseline is displaced from its host's by at most this fraction of the host's size.
/// TeX raises a superscript by about 0.45 em and drops a subscript by about 0.25 em.
const SCRIPT_MAX_BASELINE_SHIFT: f32 = 0.6;

/// A run of this many glyphs is a phrase rather than a script, and is only absorbed if it sits
/// on the host's own baseline.
const SCRIPT_MAX_DISPLACED_GLYPHS: usize = 20;

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

/// A drop capital is set at least this many times the size of the text that flows around it.
/// Two lines deep is the shallowest any template sets one, and that is already 1.9 or so.
const DROP_CAP_MIN_SIZE_RATIO: f32 = 1.8;

/// ...and at most this many. Beyond it the glyph belongs to display type — a title, a section
/// ornament — rather than to a paragraph.
const DROP_CAP_MAX_SIZE_RATIO: f32 = 6.0;

/// The first line of the paragraph starts within this many body sizes of the capital's right
/// edge. It has to be set *into* the paragraph, not merely somewhere to its left, which is what
/// keeps a figure label or an equation number in another part of the page from claiming a line.
const DROP_CAP_INDENT_SLACK: f32 = 2.0;

/// A line takes a drop capital only if it is set at the same size, within this many points.
const DROP_CAP_SIZE_SLACK: f32 = 0.5;

/// Text set around a drop capital may tuck under its right edge by this fraction of the body
/// size, and no more. A line that starts level with the capital is not indented around it.
const DROP_CAP_OVERHANG: f32 = 0.25;

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
    /// Most of the line's glyphs are bold. Heading detection leans on this for templates that
    /// set section titles in bold at body size rather than enlarging them.
    pub bold: bool,
    /// Most of the line's glyphs are italic.
    pub italic: bool,
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

    reunite_drop_capitals(page, &mut lines);

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
///
/// How far apart two baselines may be is judged at the *smaller* of the two sizes involved.
/// Reading it off the incoming glyph alone lets a large one reach further than the line it is
/// joining is tall: on a page of `medimaging.pdf` where a 10pt appendix column runs beside an 8pt
/// bibliography, a 10pt line 2.5pt below an 8pt one was inside the 10pt tolerance and outside the
/// 8pt one, and joined it. The merged cluster then spanned both columns, which let
/// [`absorb_scripts`] take the *next* bibliography line as a script of it, and three lines of two
/// different columns came out zipped together a character at a time
/// (`AnaEvnig, YM.,e Kd oBgiaonl`).
fn cluster_by_baseline(page: &PageRaw, order: &[usize]) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    // Each open cluster's baseline and the size of the glyph that opened it.
    let mut open: Vec<(f32, f32)> = Vec::new();

    for &index in order {
        let glyph = &page.glyphs[index];

        // The sort means only the most recent clusters can still be in range.
        let candidate = open
            .iter()
            .enumerate()
            .rev()
            .take(8)
            .filter(|(_, &(baseline, size))| {
                let tolerance = (size.min(glyph.size) * BASELINE_TOLERANCE).max(0.5);
                (baseline - glyph.origin.y).abs() <= tolerance
            })
            .min_by(|(_, (a, _)), (_, (b, _))| {
                (*a - glyph.origin.y)
                    .abs()
                    .total_cmp(&(*b - glyph.origin.y).abs())
            })
            .map(|(i, _)| i);

        match candidate {
            Some(i) => clusters[i].push(index),
            None => {
                clusters.push(vec![index]);
                open.push((glyph.origin.y, glyph.size));
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
                bold: false,
                italic: false,
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
                && !is_another_line(small, host)
        })
        .min_by(|(_, a), (_, b)| {
            let da = (a.baseline - small.baseline).abs();
            let db = (b.baseline - small.baseline).abs();
            da.total_cmp(&db)
        })
        .map(|(i, _)| i)
}

/// Moves an initial capital set into the margin back to the word it opens.
///
/// `\IEEEPARstart` and `\lettrine` set the first letter of a paragraph two or three lines deep in
/// the margin, so its baseline is the *last* of the lines it spans and reading order places it
/// there: `Conformal antennas...` comes out as `onformal antennas are essential components in
/// appli-Ccations`. That is worse than losing the letter, because nothing about it looks wrong.
///
/// The capital is recognised by the shape the typesetter made, not by its size alone: it is the
/// leftmost glyph of its line, several times the size of every other glyph on it, and the lines
/// it rises through are indented to just past its right edge because the paragraph was set to
/// flow around it. Display type fails the last test — nothing is indented around a title — which
/// is what keeps section ornaments, figure labels and equation numbers out.
fn reunite_drop_capitals(page: &PageRaw, lines: &mut [Line]) {
    let moves: Vec<(usize, usize)> = (0..lines.len())
        .filter_map(|index| drop_capital_target(page, lines, index).map(|target| (index, target)))
        .collect();

    for (from, to) in moves {
        // A line can only give up its capital once, and a line indented around one is not itself
        // opened by one, so the plan cannot have gone stale — but it costs nothing to check.
        if from == to || lines[from].glyphs.len() < 2 {
            continue;
        }
        let capital = lines[from].glyphs.remove(0);
        lines[to].glyphs.insert(
            0,
            Placed {
                script: Script::Normal,
                // No break before it: the capital and the word it opens are one word.
                break_before: false,
                ..capital
            },
        );
        for line in [from, to] {
            recompute(page, &mut lines[line]);
            segment_words(page, &mut lines[line]);
        }
    }
}

/// The line whose first word an oversized initial belongs to, if the line at `index` opens with
/// one at all.
fn drop_capital_target(page: &PageRaw, lines: &[Line], index: usize) -> Option<usize> {
    let line = &lines[index];
    let (first, rest) = line.glyphs.split_first()?;
    if rest.is_empty() {
        return None;
    }

    let capital = &page.glyphs[first.index];
    let crate::ir::GlyphText::Char(letter) = capital.text else {
        return None;
    };
    if !letter.is_alphabetic() || !letter.is_uppercase() {
        return None;
    }

    // Measured against the line's own dominant size, which is the body size: the capital is one
    // glyph among a line of them and cannot be the dominant size itself.
    let body = line.size;
    if body <= 0.0
        || capital.size < body * DROP_CAP_MIN_SIZE_RATIO
        || capital.size > body * DROP_CAP_MAX_SIZE_RATIO
    {
        return None;
    }

    // One outsized glyph is a drop capital; two are a line of display type.
    if rest
        .iter()
        .any(|p| page.glyphs[p.index].size > body * DROP_CAP_MIN_SIZE_RATIO)
    {
        return None;
    }

    // The rest of this line is set past the capital rather than running into it.
    let wrapped = rest
        .iter()
        .map(|p| page.glyphs[p.index].bbox.x0)
        .fold(f32::MAX, f32::min);
    if wrapped < capital.bbox.x1 - body * DROP_CAP_OVERHANG {
        return None;
    }

    // The paragraph's first line is the topmost line the capital rises through that is indented
    // around it: above this one, starting above the capital's own ascent, set at the same size,
    // and beginning just past the capital's right edge.
    lines
        .iter()
        .enumerate()
        .filter(|&(other, candidate)| {
            other != index
                && !candidate.is_empty()
                && (candidate.size - body).abs() <= DROP_CAP_SIZE_SLACK
                && candidate.baseline < line.baseline
                && candidate.baseline > capital.bbox.y0
                && candidate.bbox.x0 >= capital.bbox.x1 - body * DROP_CAP_OVERHANG
                && candidate.bbox.x0 <= capital.bbox.x1 + body * DROP_CAP_INDENT_SLACK
        })
        .min_by(|(_, a), (_, b)| a.baseline.total_cmp(&b.baseline))
        .map(|(other, _)| other)
}

/// Whether a small cluster is a line of the page in its own right rather than a script.
///
/// Ink boxes are the only thing separating the two, and they are not enough on their own: a page
/// setting an 8pt bibliography beside a 10pt column has lines of one column that overlap lines of
/// the other, and the smaller one is inside the size ratio a script is allowed. `medimaging.pdf`
/// absorbed a whole bibliography line into a line of the appendix that way, and — because the
/// host already spanned both columns — sorting the result by x zipped two columns together a
/// character at a time (`AnaEvnig, YM.,e Kd oBgiaonl`).
///
/// What distinguishes them is that a script is a *short run near its host's baseline*. Either
/// property alone is common enough in real maths — a summation's limits are far off the baseline,
/// a long subscript is a mouthful of glyphs — so both are required before the cluster is refused.
fn is_another_line(small: &Line, host: &Line) -> bool {
    let displaced = (small.baseline - host.baseline).abs() > host.size * SCRIPT_MAX_BASELINE_SHIFT;
    displaced && small.glyphs.len() >= SCRIPT_MAX_DISPLACED_GLYPHS
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

    line.size = crate::util::dominant(
        indices.iter().map(|&i| &page.glyphs[i]),
        0.25,
        |g| g.size,
        |g| g.bbox.width().max(0.1),
    )
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
    line.baseline = crate::util::median(&mut baselines).unwrap_or(0.0);

    // Emphasis is judged over the line as a whole: a single italic symbol in a sentence does not
    // make the line italic, but a fully italicised caption lead-in is worth knowing about.
    let total = line.glyphs.len().max(1);
    let bold = line
        .glyphs
        .iter()
        .filter(|p| page.glyphs[p.index].flags.is_bold())
        .count();
    let italic = line
        .glyphs
        .iter()
        .filter(|p| page.glyphs[p.index].flags.is_italic())
        .count();
    line.bold = bold * 2 > total;
    line.italic = italic * 2 > total;
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
                bold: line.bold,
                italic: line.italic,
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
/// characters (9 to 236 per document), and the reader *generates* thousands more from the font's
/// advance widths — 5 000 to 8 500 per document. Measured against those, pure gap analysis
/// mis-segments kerned pairs such as `learning framework`, where the ink gap is 0.18 em against
/// a 0.30 em typical space.
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
///
/// The marks win outright on a line that has any, rather than being unioned with the inferred
/// gaps. Both alternatives were tried and are worse: unioning splits *inside* words, because the
/// ink gap after a narrow letter is indistinguishable from a space (`I mage`, `M i crosoft`).
/// The cost is that a backend which under-marks a line runs the rest of that line together, so
/// completeness of the marks is a real obligation on a [`PageSource`](crate::backend::PageSource).
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

    /// A page that sets an 8pt bibliography beside a 10pt column, as `medimaging.pdf` does.
    ///
    /// The two lines are 2.5pt apart, which is inside a 10pt glyph's share of the baseline
    /// tolerance and outside an 8pt one's. Reading the tolerance off the arriving glyph alone
    /// merged them into a cluster spanning both columns.
    #[test]
    fn a_line_of_a_neighbouring_column_stays_its_own_line() {
        let mut glyphs = run("entry in the bibliography", 320.0, 130.5, 8.0);
        glyphs.extend(run("a line of the column beside it", 64.5, 133.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert_eq!(
            texts(&lines),
            [
                "entry in the bibliography",
                "a line of the column beside it"
            ],
            "two columns at two sizes were read as one line"
        );
    }

    /// What separates a script from a line of the page: a script is a short run near its host's
    /// baseline, and needs to be both.
    #[test]
    fn a_displaced_run_of_text_is_not_a_script() {
        let placed = |count: usize| {
            (0..count)
                .map(|index| Placed {
                    index,
                    script: Script::Normal,
                    break_before: false,
                })
                .collect::<Vec<_>>()
        };
        let line = |baseline: f32, size: f32, glyphs: Vec<Placed>| Line {
            bbox: Rect::from_corners(0.0, baseline - size, 100.0, baseline),
            baseline,
            size,
            bold: false,
            italic: false,
            glyphs,
            words: Vec::new(),
        };
        let host = line(100.0, 10.0, placed(40));

        // A whole bibliography line, most of an em off the host's baseline.
        assert!(is_another_line(&line(93.0, 8.0, placed(56)), &host));
        // A superscript is as far off the baseline as TeX puts one, and is three glyphs.
        assert!(!is_another_line(&line(95.5, 7.0, placed(3)), &host));
        // A mouthful of a subscript is long, but it sits where a subscript sits.
        assert!(!is_another_line(&line(102.0, 7.0, placed(30)), &host));
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

    /// Lays out `\IEEEPARstart`: an initial two lines deep in the margin, the two lines it rises
    /// through indented past it, and the paragraph then running to the full measure.
    fn drop_capital_paragraph(initial: char) -> Vec<Glyph> {
        // The initial's baseline is the *second* line's, which is why reading order puts it there.
        let mut glyphs = vec![glyph(initial, 49.0, 112.0, 20.0)];
        glyphs.extend(run("onformal antennas are", 71.0, 100.0, 10.0));
        glyphs.extend(run("essential where a surface", 71.0, 112.0, 10.0));
        glyphs.extend(run("curves away.", 49.0, 124.0, 10.0));
        glyphs
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn a_drop_capital_opens_the_word_it_belongs_to() {
        let page = page_of(drop_capital_paragraph('C'));
        let lines = build_lines(&page);

        assert_eq!(lines.len(), 3, "the capital must not stand as its own line");
        assert_eq!(
            texts(&lines),
            [
                "Conformal antennas are",
                "essential where a surface",
                "curves away."
            ]
        );
        assert_eq!(
            lines[0].size, 10.0,
            "the capital must not become the line's size"
        );
        assert_eq!(lines[0].baseline, 100.0, "nor drag its baseline");
    }

    /// The capital is recognised by the paragraph set around it. Without the indent it is
    /// display type — a section ornament, a figure label — and stays where it was found.
    #[test]
    fn an_initial_with_nothing_set_around_it_is_left_alone() {
        let mut glyphs = vec![glyph('C', 49.0, 112.0, 20.0)];
        // Both lines at the full measure: nothing is wrapped around the capital.
        glyphs.extend(run("onformal antennas are", 49.0, 100.0, 10.0));
        glyphs.extend(run("essential where a surface", 49.0, 112.0, 10.0));
        let page = page_of(glyphs);

        let lines = build_lines(&page);
        assert!(
            !texts(&lines).iter().any(|t| t.starts_with("Conformal")),
            "an unwrapped initial was moved: {:?}",
            texts(&lines)
        );
    }

    /// A digit is an equation number or a figure label, never the first letter of a word.
    #[test]
    fn an_oversized_digit_is_not_a_drop_capital() {
        let page = page_of(drop_capital_paragraph('2'));
        let lines = build_lines(&page);
        assert_eq!(
            texts(&lines)[0],
            "onformal antennas are",
            "a digit was treated as a drop capital"
        );
    }

    #[test]
    fn empty_page_yields_no_lines() {
        let page = page_of(Vec::new());
        assert!(build_lines(&page).is_empty());
    }
}
