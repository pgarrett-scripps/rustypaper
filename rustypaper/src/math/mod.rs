//! Finding and reconstructing mathematics.
//!
//! Detection seeds on evidence that is unambiguous — a glyph set in a TeX maths font, or a
//! character that occurs nowhere but in formulae — and grows outwards from those seeds through
//! the material that surrounds them. Growing is necessary because most of a formula is set in
//! ordinary digits, letters and parentheses: `x`, `2` and `(` carry no signal on their own, and
//! only their company makes them mathematical.

pub mod build;
pub mod latex;
pub mod symbols;

use crate::ir::{FontTable, PageRaw};
use crate::text::lines::Line;

pub use build::{Formula, Node};

/// Below this confidence, an equation is emitted as a picture instead of as LaTeX.
pub const MIN_CONFIDENCE: f32 = 0.55;

/// A display equation must be at least this fraction mathematical, by seed glyphs.
///
/// One seed is not enough. An author line reading `Diederik P. Kingma∗ Jimmy Lei Ba∗` is centred
/// and contains U+2217, and on that basis alone the pass reported 220 display equations in a
/// paper that has perhaps twenty.
const MIN_SEED_FRACTION: f32 = 0.10;

/// And at least this many seeds outright.
const MIN_SEEDS: usize = 2;

/// Characters an equation number may hold between its brackets.
const MAX_NUMBER_LENGTH: usize = 5;

/// A number floating mid-line is only a number if this many ems of space precede it.
const NUMBER_GAP: f32 = 2.0;

/// A line whose right edge comes this close to the column's is set against the margin.
const NUMBER_MARGIN_SLACK: f32 = 1.5;

/// Against the margin, this much of a gap is enough — see [`trailing_number`].
const NUMBER_GAP_AT_MARGIN: f32 = 0.6;

/// A stacked fragment may sit this many ems from the block it joins, vertically.
///
/// Generous, because the parts of one formula are set to touch: a numerator's box and its
/// denominator's overlap the rule between them, and the gap is often negative.
const STACK_MAX_GAP: f32 = 0.6;

/// And it must sit within this many ems of the block's horizontal span.
const STACK_SLACK: f32 = 1.0;

/// A fragment is short. Anything longer is a line in its own right.
const STACK_MAX_WORDS: usize = 6;

/// A fragment set below this fraction of the block's size is a script of it, whatever else it is.
const SCRIPT_SIZE_RATIO: f32 = 0.92;

/// A run of glyphs on one line that forms mathematics.
#[derive(Debug, Clone)]
pub struct Span {
    /// Index of the line within the page's line list.
    pub line: usize,
    /// Range within that line's glyph list.
    pub start: usize,
    pub end: usize,
    pub formula: Formula,
}

/// A whole line that is a display equation.
#[derive(Debug, Clone)]
pub struct Display {
    pub line: usize,
    pub formula: Formula,
    /// The equation number, if the line carries one.
    pub number: Option<String>,
    /// Every line the equation is built from, `line` included, in page order. A display equation
    /// is one *formula* but often several lines: a summation's limits, a fraction's numerator
    /// and denominator and a grown parenthesis each arrive as a line of their own.
    pub lines: Vec<usize>,
    /// The union of those lines' boxes.
    pub bbox: crate::ir::Rect,
}

/// Whether a glyph is unambiguous evidence of mathematics.
fn is_seed(page: &PageRaw, fonts: &FontTable, index: usize) -> bool {
    let glyph = &page.glyphs[index];
    if symbols::is_math_font(fonts.name(glyph.font)) {
        return true;
    }
    match glyph.text {
        crate::ir::GlyphText::Char(c) => symbols::is_math_symbol(c),
        crate::ir::GlyphText::Expanded(_) => false,
    }
}

/// Whether a character can appear inside a formula without being evidence for one.
fn is_filler_char(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            '+' | '-'
                | '='
                | '/'
                | '('
                | ')'
                | '['
                | ']'
                | '|'
                | ','
                | '.'
                | '\''
                | '<'
                | '>'
                | '!'
                | '*'
                | '^'
                | '_'
                | ':'
                | ';'
        )
}

/// How a word relates to a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// Contains a glyph that only occurs in mathematics.
    Seed,
    /// Could be part of a formula: an operator, a number, a lone variable.
    Connector,
    /// Ordinary prose.
    Prose,
}

/// Classifies a word for span growth.
///
/// Growth happens over *words*, not glyphs, and this is why. Treating every ASCII letter as
/// possible formula material lets a single `α` absorb the whole line it sits in — the pass
/// wrapped entire paragraphs of prose in `$...$` before this was word-based. A variable in
/// running text is one or two characters long; a word of prose is not.
fn classify(
    page: &PageRaw,
    fonts: &FontTable,
    line: &Line,
    word: &crate::text::lines::Word,
) -> Role {
    let has_seed = line.glyphs[word.start..word.end]
        .iter()
        .any(|p| is_seed(page, fonts, p.index));
    if has_seed {
        return Role::Seed;
    }

    // Only wholly non-alphabetic words connect: operators, numbers, punctuation.
    //
    // Short *words* deliberately do not, even though a lone variable is short. A variable in
    // running text is set in a maths font and is therefore already a seed, whereas English is
    // full of two-letter function words - `of`, `to`, `in`, `is` - and admitting those on length
    // alone produced spans like `$of \alpha in$`.
    let chars: Vec<char> = word.text.chars().collect();
    let has_letters = chars.iter().any(|c| c.is_alphabetic());
    let all_filler = chars.iter().all(|c| is_filler_char(*c));

    if !has_letters && all_filler && !chars.is_empty() {
        Role::Connector
    } else {
        Role::Prose
    }
}

/// Finds the inline mathematics on one line.
///
/// A run is a maximal stretch of non-prose words containing at least one seed. Runs of a single
/// short word are discarded: an italic `a` in prose is not an equation, and treating it as one
/// litters the output with `$a$`.
pub fn spans(page: &PageRaw, fonts: &FontTable, lines: &[Line], line: usize) -> Vec<Span> {
    let target = &lines[line];
    if target.words.is_empty() {
        return Vec::new();
    }

    let roles: Vec<Role> = target
        .words
        .iter()
        .map(|w| classify(page, fonts, target, w))
        .collect();

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < roles.len() {
        if roles[i] == Role::Prose {
            i += 1;
            continue;
        }
        let start = i;
        while i < roles.len() && roles[i] != Role::Prose {
            i += 1;
        }
        if roles[start..i].contains(&Role::Seed) {
            runs.push((start, i));
        }
    }

    runs.into_iter()
        .filter(|&(first, last)| {
            // A single one-character word is a stray symbol, not a formula.
            last - first > 1 || target.words[first].text.chars().count() > 1
        })
        .map(|(first, last)| {
            let start = target.words[first].start;
            let end = target.words[last - 1].end;
            let indices: Vec<usize> = target.glyphs[start..end].iter().map(|p| p.index).collect();
            Span {
                line,
                start,
                end,
                formula: build::build(page, &indices),
            }
        })
        .collect()
}

/// What a line offers as evidence that it is a display equation.
///
/// Gathered in one place because the admission rule below weighs several signals against one
/// another, and because a diagnostic that cannot see the same numbers the rule sees is no
/// diagnostic at all.
#[derive(Debug, Clone)]
pub struct Evidence {
    /// Glyphs on the line.
    pub glyphs: usize,
    /// Of those, how many are unambiguous mathematics.
    pub seeds: usize,
    pub seed_fraction: f32,
    pub words: usize,
    /// Words that are ordinary prose.
    pub prose_words: usize,
    /// A trailing right-aligned equation number, if the line carries one.
    pub number: Option<String>,
    /// Where the body ends, once any equation number is split off.
    pub body_end: usize,
    /// Distance from the column's left edge to the line's, and from the line's right edge to
    /// the column's.
    pub left: f32,
    pub right: f32,
    /// Set symmetrically within its column, and clear of the left margin.
    pub centred: bool,
}

/// Measures one line against everything the display rule cares about.
pub fn evidence(
    page: &PageRaw,
    fonts: &FontTable,
    target: &Line,
    column: crate::ir::Rect,
) -> Evidence {
    let seeds = target
        .glyphs
        .iter()
        .filter(|p| is_seed(page, fonts, p.index))
        .count();
    let prose_words = target
        .words
        .iter()
        .filter(|w| classify(page, fonts, target, w) == Role::Prose)
        .count();
    let (number, body_end) = trailing_number(page, target, column);
    let left = target.bbox.x0 - column.x0;
    let right = column.x1 - target.bbox.x1;
    Evidence {
        glyphs: target.glyphs.len(),
        seeds,
        seed_fraction: if target.glyphs.is_empty() {
            0.0
        } else {
            seeds as f32 / target.glyphs.len() as f32
        },
        words: target.words.len(),
        prose_words,
        number,
        body_end,
        left,
        right,
        centred: (left - right).abs() <= target.size * 2.0 && left > target.size * 2.0,
    }
}

/// Whether a line is a display equation, and what its number is.
///
/// Display maths is set apart from the text: centred in its column, or indented well clear of
/// the margin, and usually followed by a right-aligned `(3)`. Requiring the line to be mostly
/// mathematics as well keeps a centred section heading out.
/// `taken` marks the lines already claimed by an equation found earlier, which this one may not
/// claim again; pass an empty slice when nothing has been.
pub fn display(
    page: &PageRaw,
    fonts: &FontTable,
    lines: &[Line],
    line: usize,
    column: crate::ir::Rect,
    taken: &[bool],
) -> Option<Display> {
    let target = &lines[line];
    if target.glyphs.is_empty() {
        return None;
    }

    let ev = evidence(page, fonts, target, column);
    if ev.seeds < MIN_SEEDS || ev.seed_fraction < MIN_SEED_FRACTION {
        return None;
    }
    if ev.prose_words * 4 > ev.words {
        return None;
    }

    // Centred in its column, or carrying an equation number. Mere indentation is not enough:
    // in a single-column paper with wide margins, almost every line is indented relative to the
    // text block once a figure or a table has widened it.
    if !(ev.centred || ev.number.is_some()) {
        return None;
    }

    let (used, bbox) = stack(page, fonts, lines, line, column, ev.body_end, taken);
    let mut indices: Vec<usize> = Vec::new();
    for &i in &used {
        let end = if i == line {
            ev.body_end
        } else {
            lines[i].glyphs.len()
        };
        indices.extend(lines[i].glyphs[..end].iter().map(|p| p.index));
    }
    Some(Display {
        line,
        formula: build::build(page, &indices),
        number: ev.number,
        lines: used,
        bbox,
    })
}

/// The lines one display equation is set across, and the box they occupy.
///
/// TeX breaks a display equation into as many lines as it has vertical structure: a summation's
/// sign, the limits under it, a fraction's numerator, its denominator and a grown parenthesis are
/// each their own run of glyphs at their own baseline, and line assembly — which knows only about
/// baselines — hands them over separately. Reconstruction is two-dimensional and wants them
/// *together*: given the whole stack, `build` recovers the fraction and the limits, and given one
/// line of it, it can only emit the fragment. unet's `E = \sum_{x \in \Omega} w(x) \log(...)` came
/// out as `E = w(x) log(...)` for exactly this reason, and matched nothing.
///
/// A fragment has to be short, free of prose, close above or below, inside the block's horizontal
/// span, and — the rule that earned itself the hard way — *material `build` knows how to place*:
/// a large operator's limits, a fraction's numerator or denominator, or a grown delimiter. Merging
/// on adjacency alone fuses two equations set one above the other, and since reconstruction sorts
/// a region left to right, the result is the two of them interleaved a character at a time:
/// transformer's two positional-encoding equations came out as
/// `PEP_(E_pos,2i+1)^(pos,2i)==scoins(...)`, and four of optics's matrix rows likewise. Both had
/// scored well as separate lines. Prose is the other load-bearing exclusion: the line under a
/// display equation is usually the paragraph that continues past it.
fn stack(
    page: &PageRaw,
    fonts: &FontTable,
    lines: &[Line],
    line: usize,
    column: crate::ir::Rect,
    body_end: usize,
    taken: &[bool],
) -> (Vec<usize>, crate::ir::Rect) {
    let target = &lines[line];
    // The equation number is not part of the formula's extent, so a fragment is not asked to sit
    // inside the whole width of the column the number reaches across.
    let mut claimed: Vec<usize> = target.glyphs[..body_end].iter().map(|p| p.index).collect();
    let mut bbox = claimed
        .iter()
        .filter_map(|&i| page.glyphs.get(i).map(|g| g.bbox))
        .reduce(|a, b| a.union(&b))
        .unwrap_or(target.bbox);
    let size = target.size;
    let mut used = vec![line];

    let rules: Vec<crate::ir::Rect> = page
        .paths
        .iter()
        .filter(|p| p.kind == crate::ir::PathKind::HorizontalRule)
        .map(|p| p.bbox)
        .collect();

    for direction in [-1isize, 1] {
        let mut at = line as isize;
        loop {
            at += direction;
            let Some(index) = usize::try_from(at).ok().filter(|&i| i < lines.len()) else {
                break;
            };
            if taken.get(index).copied().unwrap_or(false) {
                break;
            }
            let candidate = &lines[index];
            if !is_fragment(page, fonts, candidate, column, bbox, size) {
                break;
            }
            if !is_placeable(page, &claimed, &rules, candidate, bbox, size) {
                break;
            }
            bbox = bbox.union(&candidate.bbox);
            claimed.extend(candidate.glyphs.iter().map(|p| p.index));
            used.push(index);
        }
    }

    used.sort_unstable();
    (used, bbox)
}

/// Whether a line is a piece of the display equation occupying `bbox`.
fn is_fragment(
    page: &PageRaw,
    fonts: &FontTable,
    candidate: &Line,
    column: crate::ir::Rect,
    bbox: crate::ir::Rect,
    size: f32,
) -> bool {
    if candidate.glyphs.is_empty() || candidate.words.len() > STACK_MAX_WORDS {
        return false;
    }
    // Its own equation number makes it another equation, not a piece of this one.
    if trailing_number(page, candidate, column).0.is_some() {
        return false;
    }
    if candidate
        .words
        .iter()
        .any(|w| classify(page, fonts, candidate, w) == Role::Prose)
    {
        return false;
    }
    let gap = if candidate.bbox.center_y() < bbox.center_y() {
        bbox.y0 - candidate.bbox.y1
    } else {
        candidate.bbox.y0 - bbox.y1
    };
    gap <= size * STACK_MAX_GAP
        && candidate.bbox.x0 >= bbox.x0 - size * STACK_SLACK
        && candidate.bbox.x1 <= bbox.x1 + size * STACK_SLACK
}

/// Whether reconstruction has somewhere to put this fragment.
///
/// Four shapes, and only these four, because they are the vertical structures
/// [`build`](build::build) recovers. Anything else it would sort into the block left to right,
/// which for two independent lines means interleaving them.
fn is_placeable(
    page: &PageRaw,
    claimed: &[usize],
    rules: &[crate::ir::Rect],
    candidate: &Line,
    bbox: crate::ir::Rect,
    size: f32,
) -> bool {
    let inner = candidate.bbox;

    // A script set low or high enough to have been given a line of its own. Size is what
    // separates it from the next equation of an aligned block, which is set at body size.
    if candidate.size < size * SCRIPT_SIZE_RATIO {
        return true;
    }
    let covers = |outer: crate::ir::Rect| {
        outer.x0 <= inner.x0 + size * STACK_SLACK && outer.x1 >= inner.x1 - size * STACK_SLACK
    };

    // A grown delimiter or a large operator, set on a line of its own because it is taller than
    // the body it belongs to. TeX gives `\sum` and a grown `(` their own baseline.
    let mut chars = candidate
        .glyphs
        .iter()
        .filter_map(|p| match page.glyphs.get(p.index)?.text {
            crate::ir::GlyphText::Char(c) => (!c.is_whitespace()).then_some(c),
            crate::ir::GlyphText::Expanded(_) => Some('x'),
        })
        .peekable();
    if chars.peek().is_some() && chars.all(is_grown_symbol) {
        return true;
    }

    // The limits of a large operator the block already holds.
    if claimed
        .iter()
        .filter_map(|&i| page.glyphs.get(i))
        .any(|glyph| {
            let large = matches!(glyph.text, crate::ir::GlyphText::Char(c) if is_grown_symbol(c));
            large && glyph.bbox.height() > glyph.size * 1.1 && covers(glyph.bbox)
        })
    {
        return true;
    }

    // A numerator or a denominator, across a fraction bar wide enough to span it.
    let top = bbox.y0.min(inner.y0);
    let bottom = bbox.y1.max(inner.y1);
    rules
        .iter()
        .any(|rule| covers(*rule) && rule.center_y() > top && rule.center_y() < bottom)
}

/// Symbols TeX grows to fit their contents, and therefore sets on their own baseline.
fn is_grown_symbol(c: char) -> bool {
    matches!(
        c,
        '∑' | '∏'
            | '∐'
            | '∫'
            | '∬'
            | '∮'
            | '⋃'
            | '⋂'
            | '√'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '|'
            | '⟨'
            | '⟩'
            | '⌈'
            | '⌉'
            | '⌊'
            | '⌋'
    )
}

/// Splits off a trailing right-aligned equation number, returning it and where the body ends.
///
/// Read off the glyphs rather than the words. The number is often the only thing on its side of
/// a wide gap, and a gap that wide is exactly where a `PageSource` may leave no space mark — so
/// `w(x) = wc(x) + w0 · exp(...)` and its `(2)` arrive as *one* word, and a rule phrased over
/// words never sees the number at all. Both of unet's display equations were lost that way.
fn trailing_number(
    page: &PageRaw,
    line: &Line,
    column: crate::ir::Rect,
) -> (Option<String>, usize) {
    let chars: Vec<(usize, char, crate::ir::Rect)> = line
        .glyphs
        .iter()
        .enumerate()
        .filter_map(|(at, placed)| {
            let glyph = page.glyphs.get(placed.index)?;
            let crate::ir::GlyphText::Char(c) = glyph.text else {
                return None;
            };
            (!c.is_whitespace()).then_some((at, c, glyph.bbox))
        })
        .collect();

    let none = (None, line.glyphs.len());
    if !matches!(chars.last(), Some((_, ')', _))) {
        return none;
    }
    // Walk back to the opening bracket over the label's own characters. `(3)`, `(2a)` and
    // `(A.12)` are all numbers a paper uses; anything longer is a parenthesis, not a label.
    let opening = chars[..chars.len() - 1]
        .iter()
        .rposition(|&(_, c, _)| c == '(')
        .filter(|&open| {
            let inner = &chars[open + 1..chars.len() - 1];
            !inner.is_empty()
                && inner.len() <= MAX_NUMBER_LENGTH
                && inner
                    .iter()
                    .all(|&(_, c, _)| c.is_ascii_alphanumeric() || c == '.')
        });
    let Some(open) = opening else { return none };
    let Some(&(_, _, before)) = chars.get(open.wrapping_sub(1)) else {
        return none;
    };

    // It has to be set apart from the formula, or `(3)` is just a factor. A number pushed out to
    // the column's right edge needs less of a gap to be unmistakable than one floating mid-line:
    // a two-column measure leaves a long equation barely a word space to spare, and requiring
    // two ems there loses the number on a line that is manifestly numbered.
    let gap = chars[open].2.x0 - before.x1;
    let at_the_margin = column.x1 - line.bbox.x1 <= line.size * NUMBER_MARGIN_SLACK;
    let detached =
        gap > line.size * NUMBER_GAP || (at_the_margin && gap > line.size * NUMBER_GAP_AT_MARGIN);
    if !detached {
        return none;
    }

    let label: String = chars[open + 1..chars.len() - 1]
        .iter()
        .map(|&(_, c, _)| c)
        .collect();
    (Some(label), chars[open].0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FontFlags, FontId, Glyph, GlyphText, PageRaw, Point, Rect, Rgba};
    use crate::text::lines::build_lines;

    struct Builder {
        glyphs: Vec<Glyph>,
        fonts: FontTable,
    }

    impl Builder {
        fn new() -> Self {
            let mut fonts = FontTable::new();
            fonts.intern("NimbusRomNo9L-Regu");
            fonts.intern("CMMI10");
            fonts.intern("CMEX10");
            Self {
                glyphs: Vec::new(),
                fonts,
            }
        }

        /// Adds a run in the body font.
        fn text(mut self, s: &str, x: f32, baseline: f32) -> Self {
            self.push(s, x, baseline, 10.0, 0);
            self
        }

        /// Adds a run in a maths font.
        fn math(mut self, s: &str, x: f32, baseline: f32, size: f32) -> Self {
            self.push(s, x, baseline, size, 1);
            self
        }

        fn push(&mut self, s: &str, mut x: f32, baseline: f32, size: f32, font: u32) {
            for c in s.chars() {
                let w = if c == ' ' { size * 0.3 } else { size * 0.5 };
                self.glyphs.push(Glyph {
                    text: GlyphText::Char(c),
                    bbox: Rect::from_corners(x, baseline - size * 0.7, x + w, baseline),
                    origin: Point::new(x, baseline),
                    font: FontId(font),
                    size,
                    angle: 0.0,
                    flags: FontFlags::default(),
                    color: Rgba::BLACK,
                    generated: false,
                });
                x += w;
            }
        }

        fn page(self) -> (PageRaw, FontTable) {
            (
                PageRaw {
                    index: 0,
                    width: 612.0,
                    height: 792.0,
                    rotation: 0,
                    glyphs: self.glyphs,
                    paths: Vec::new(),
                    images: Vec::new(),
                    expansions: Vec::new(),
                },
                self.fonts,
            )
        }
    }

    /// The display equation at `line`, with no other equation yet claiming anything.
    fn display_at(
        page: &PageRaw,
        fonts: &FontTable,
        lines: &[Line],
        line: usize,
        column: Rect,
    ) -> Option<Display> {
        display(page, fonts, lines, line, column, &[])
    }

    #[test]
    fn inline_maths_is_found_inside_prose() {
        let (page, fonts) = Builder::new()
            .text("we set ", 72.0, 200.0)
            .math("x", 107.0, 200.0, 10.0)
            .text("=1 today", 112.0, 200.0)
            .page();

        let lines = build_lines(&page);
        let found = spans(&page, &fonts, &lines, 0);
        assert_eq!(found.len(), 1, "expected one span, got {found:?}");
        assert!(found[0].formula.confidence > 0.0);
    }

    #[test]
    fn prose_alone_contains_no_maths() {
        let (page, fonts) = Builder::new()
            .text("an ordinary sentence with no formulae", 72.0, 200.0)
            .page();
        let lines = build_lines(&page);
        assert!(spans(&page, &fonts, &lines, 0).is_empty());
    }

    /// A lone italic letter in prose is not an equation.
    #[test]
    fn a_single_symbol_is_not_a_span() {
        let (page, fonts) = Builder::new().math("x", 72.0, 200.0, 10.0).page();
        let lines = build_lines(&page);
        assert!(spans(&page, &fonts, &lines, 0).is_empty());
    }

    #[test]
    fn a_centred_line_of_maths_is_a_display_equation() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new().math("a=b+c", 293.0, 300.0, 10.0).page();
        let lines = build_lines(&page);
        let found = display_at(&page, &fonts, &lines, 0, column);
        assert!(
            found.is_some(),
            "centred formula was not a display equation"
        );
    }

    #[test]
    fn a_full_width_line_of_prose_is_not_a_display_equation() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .text("this line starts at the margin and is prose", 72.0, 300.0)
            .page();
        let lines = build_lines(&page);
        assert!(display_at(&page, &fonts, &lines, 0, column).is_none());
    }

    /// A centred line with one stray symbol is not an equation. This is the author line of a
    /// real paper: `Diederik P. Kingma∗ Jimmy Lei Ba∗`.
    #[test]
    fn a_centred_author_line_is_not_a_display_equation() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .text("Diederik P. Kingma", 200.0, 120.0)
            .math("\u{2217}", 290.0, 120.0, 10.0)
            .text(" Jimmy Lei Ba", 296.0, 120.0)
            .math("\u{2217}", 360.0, 120.0, 10.0)
            .page();
        let lines = build_lines(&page);
        assert!(display_at(&page, &fonts, &lines, 0, column).is_none());
    }

    #[test]
    fn a_trailing_equation_number_is_split_off() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("a=b", 300.0, 300.0, 10.0)
            .text("(3)", 520.0, 300.0)
            .page();
        let lines = build_lines(&page);
        let found = display_at(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.number.as_deref(), Some("3"));
    }

    /// The number is read off the glyphs, not the words. A gap wide enough to hold an equation
    /// number is exactly where a reader may leave no space mark, so the formula and its `(2)`
    /// arrive as one word — and a two-em rule phrased over words never sees it. Both of unet's
    /// display equations were lost this way.
    #[test]
    fn a_number_at_the_margin_needs_no_word_break() {
        let column = Rect::from_corners(72.0, 0.0, 355.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("a=b+c", 300.0, 300.0, 10.0)
            .text("(2)", 340.0, 300.0)
            .page();
        let lines = build_lines(&page);
        let found = display_at(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.number.as_deref(), Some("2"));
        // The number is not part of the formula.
        assert!(!latex::render(&found.formula.root).contains('2'));
    }

    /// A line of prose that happens to end in a citation is not a numbered equation: the gap
    /// before it is a word space, not the width of a column's tail.
    #[test]
    fn a_word_space_before_a_bracket_is_not_a_number() {
        let column = Rect::from_corners(72.0, 0.0, 355.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("a=b+c", 300.0, 300.0, 10.0)
            .text(" (2)", 325.0, 300.0)
            .page();
        let lines = build_lines(&page);
        assert!(display_at(&page, &fonts, &lines, 0, column).is_none());
    }

    /// A display equation is one formula and often several lines. Given the whole stack,
    /// reconstruction recovers the summation; given the middle line alone, it emits the body and
    /// silently drops the operator.
    #[test]
    fn a_summation_set_across_three_lines_is_one_equation() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("\u{2211}", 280.0, 288.0, 10.0)
            .math("E=w(x)+log(y)", 270.0, 300.0, 10.0)
            .math("x\u{2208}\u{03A9}", 280.0, 306.0, 7.0)
            .page();
        let lines = build_lines(&page);
        assert_eq!(lines.len(), 3, "expected three lines, got {}", lines.len());
        let found = display_at(&page, &fonts, &lines, 1, column).expect("display equation");
        assert_eq!(found.lines, vec![0, 1, 2]);
        let latex = latex::render(&found.formula.root);
        assert!(latex.contains(r"\sum"), "the operator was dropped: {latex}");
        assert!(latex.contains(r"\Omega"), "the limit was dropped: {latex}");
    }

    /// The equation *below* a display equation is another equation, not a piece of it. Merging
    /// on adjacency alone interleaved transformer's two positional-encoding equations a
    /// character at a time, and both had scored well apart.
    #[test]
    fn the_next_equation_down_is_not_absorbed() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("E=w(x)+log(y)", 270.0, 300.0, 10.0)
            .math("E=w(x)+cos(y)", 272.0, 312.0, 10.0)
            .page();
        let lines = build_lines(&page);
        assert_eq!(lines.len(), 2);
        let found = display_at(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.lines, vec![0], "the second equation was swallowed");
    }

    /// A line already lifted into the equation above cannot be lifted into this one as well.
    #[test]
    fn a_claimed_line_is_left_alone() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("\u{2211}", 280.0, 288.0, 10.0)
            .math("E=w(x)+log(y)", 270.0, 300.0, 10.0)
            .page();
        let lines = build_lines(&page);
        let found =
            display(&page, &fonts, &lines, 1, column, &[true, false]).expect("display equation");
        assert_eq!(found.lines, vec![1]);
    }

    /// `(3)` immediately after a formula is a factor, not an equation number.
    #[test]
    fn an_adjacent_parenthesis_is_not_an_equation_number() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new().math("a=b(3)", 291.0, 300.0, 10.0).page();
        let lines = build_lines(&page);
        let found = display_at(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.number, None);
    }
}
