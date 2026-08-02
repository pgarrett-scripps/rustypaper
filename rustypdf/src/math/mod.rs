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

/// Whether a line is a display equation, and what its number is.
///
/// Display maths is set apart from the text: centred in its column, or indented well clear of
/// the margin, and usually followed by a right-aligned `(3)`. Requiring the line to be mostly
/// mathematics as well keeps a centred section heading out.
pub fn display(
    page: &PageRaw,
    fonts: &FontTable,
    lines: &[Line],
    line: usize,
    column: crate::ir::Rect,
) -> Option<Display> {
    let target = &lines[line];
    if target.glyphs.is_empty() {
        return None;
    }

    let seeds = target
        .glyphs
        .iter()
        .filter(|p| is_seed(page, fonts, p.index))
        .count();
    if seeds < MIN_SEEDS || (seeds as f32) < target.glyphs.len() as f32 * MIN_SEED_FRACTION {
        return None;
    }

    let prose_words = target
        .words
        .iter()
        .filter(|w| classify(page, fonts, target, w) == Role::Prose)
        .count();
    if prose_words * 4 > target.words.len() {
        return None;
    }

    let (number, body_end) = trailing_number(target);

    // Centred in its column, or carrying an equation number. Mere indentation is not enough:
    // in a single-column paper with wide margins, almost every line is indented relative to the
    // text block once a figure or a table has widened it.
    let left = target.bbox.x0 - column.x0;
    let right = column.x1 - target.bbox.x1;
    let centred = (left - right).abs() <= target.size * 2.0 && left > target.size * 2.0;
    if !(centred || number.is_some()) {
        return None;
    }

    let indices: Vec<usize> = target.glyphs[..body_end].iter().map(|p| p.index).collect();
    Some(Display {
        line,
        formula: build::build(page, &indices),
        number,
    })
}

/// Splits off a trailing right-aligned equation number, returning it and where the body ends.
fn trailing_number(line: &Line) -> (Option<String>, usize) {
    let Some(last) = line.words.last() else {
        return (None, line.glyphs.len());
    };
    let looks_numbered = last.text.starts_with('(')
        && last.text.ends_with(')')
        && last.text[1..last.text.len() - 1]
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'a'..='z' | 'A'..='Z'));

    // It has to be set apart from the formula, or `(3)` is just a factor.
    let detached = line.words.len() > 1 && {
        let previous = &line.words[line.words.len() - 2];
        last.bbox.x0 - previous.bbox.x1 > line.size * 2.0
    };

    if looks_numbered && detached {
        (
            Some(last.text[1..last.text.len() - 1].to_owned()),
            last.start,
        )
    } else {
        (None, line.glyphs.len())
    }
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
        let found = display(&page, &fonts, &lines, 0, column);
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
        assert!(display(&page, &fonts, &lines, 0, column).is_none());
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
        assert!(display(&page, &fonts, &lines, 0, column).is_none());
    }

    #[test]
    fn a_trailing_equation_number_is_split_off() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new()
            .math("a=b", 300.0, 300.0, 10.0)
            .text("(3)", 520.0, 300.0)
            .page();
        let lines = build_lines(&page);
        let found = display(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.number.as_deref(), Some("3"));
    }

    /// `(3)` immediately after a formula is a factor, not an equation number.
    #[test]
    fn an_adjacent_parenthesis_is_not_an_equation_number() {
        let column = Rect::from_corners(72.0, 0.0, 540.0, 792.0);
        let (page, fonts) = Builder::new().math("a=b(3)", 291.0, 300.0, 10.0).page();
        let lines = build_lines(&page);
        let found = display(&page, &fonts, &lines, 0, column).expect("display equation");
        assert_eq!(found.number, None);
    }
}
