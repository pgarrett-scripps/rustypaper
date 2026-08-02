//! Recovering the structure of a formula from where its glyphs sit.
//!
//! This is the MaxTract insight: a born-digital PDF already contains the exact characters of a
//! formula and their exact positions, so an image-to-LaTeX model would be re-deriving
//! information we already hold, and hallucinating where it is unsure. What has to be recovered
//! is only the *two-dimensional relationships* — that this glyph is a superscript of that one,
//! that these glyphs are above a rule and those below it.
//!
//! The recursion is over vertical structure first, then horizontal:
//!
//! 1. A fraction rule splits its region into a numerator and a denominator.
//! 2. A radical sign claims what sits under its overline.
//! 3. A large operator claims what sits above and below it as limits.
//! 4. What remains is a horizontal run, in which each glyph is on the baseline, raised, or
//!    lowered relative to the run's dominant baseline.
//!
//! Every node carries a confidence. Anything the pass is unsure of stays unsure all the way to
//! the emitter, which prefers a picture of an equation to a confident-looking wrong one.

use crate::ir::{Glyph, PageRaw, PathKind, Rect};

/// A script must be at most this fraction of the base's size.
const SCRIPT_MAX_SIZE_RATIO: f32 = 0.92;

/// A script's baseline must be displaced by at least this fraction of the base's size.
const SCRIPT_MIN_OFFSET: f32 = 0.18;

/// Glyphs further apart than this fraction of the font size have a space between them.
const SPACE_RATIO: f32 = 0.28;

/// A rule must be at least this wide, relative to the font size, to be a fraction bar rather
/// than a minus sign or an overline artefact.
const MIN_FRACTION_WIDTH_RATIO: f32 = 0.9;

/// Confidence retained for each construct the pass had to guess at. Applied multiplicatively,
/// so a formula with several guesses in it falls below the threshold for emitting LaTeX at all.
const UNCERTAIN: f32 = 0.75;

/// A node of a reconstructed formula.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A literal run of characters.
    Symbol(String),
    /// A horizontal sequence.
    Row(Vec<Node>),
    /// `base^{sup}_{sub}`.
    Scripted {
        base: Box<Node>,
        sup: Option<Box<Node>>,
        sub: Option<Box<Node>>,
    },
    /// `\frac{num}{den}`.
    Fraction { num: Box<Node>, den: Box<Node> },
    /// `\sqrt{arg}`.
    Radical { arg: Box<Node> },
    /// A large operator with optional limits, as in `\sum_{i=0}^{n}`.
    Operator {
        symbol: String,
        below: Option<Box<Node>>,
        above: Option<Box<Node>>,
    },
    /// Content between delimiters, sized to fit.
    Fenced {
        open: String,
        close: String,
        body: Box<Node>,
    },
}

/// A reconstructed formula and how much to trust it.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    pub root: Node,
    /// 1.0 when every construct was unambiguous.
    pub confidence: f32,
}

/// One glyph, prepared for reconstruction.
#[derive(Debug, Clone, Copy)]
struct Item {
    bbox: Rect,
    baseline: f32,
    size: f32,
    ch: char,
}

/// Builds a formula from the glyphs at `indices` on `page`.
pub fn build(page: &PageRaw, indices: &[usize]) -> Formula {
    let mut items: Vec<Item> = indices
        .iter()
        .filter_map(|&i| {
            let glyph: &Glyph = page.glyphs.get(i)?;
            let ch = match glyph.text {
                crate::ir::GlyphText::Char(c) => c,
                crate::ir::GlyphText::Expanded(e) => {
                    page.expansions.get(e as usize)?.chars().next()?
                }
            };
            (!ch.is_whitespace()).then_some(Item {
                bbox: glyph.bbox,
                baseline: glyph.origin.y,
                size: glyph.size,
                ch,
            })
        })
        .collect();
    items.sort_by(|a, b| a.bbox.x0.total_cmp(&b.bbox.x0));

    if items.is_empty() {
        return Formula {
            root: Node::Row(Vec::new()),
            confidence: 1.0,
        };
    }

    // Only rules that lie within the formula's own extent can be its fraction bars.
    let extent = items
        .iter()
        .map(|i| i.bbox)
        .reduce(|a, b| a.union(&b))
        .unwrap();
    let rules: Vec<Rect> = page
        .paths
        .iter()
        .filter(|p| p.kind == PathKind::HorizontalRule)
        .map(|p| p.bbox)
        .filter(|r| r.x_overlap(&extent) > 0.0 && r.y_overlap(&extent) > -2.0)
        .collect();

    let mut confidence = 1.0f32;
    let root = region(&items, &rules, &mut confidence);
    Formula { root, confidence }
}

/// Reconstructs one rectangular region of a formula.
fn region(items: &[Item], rules: &[Rect], confidence: &mut f32) -> Node {
    if items.is_empty() {
        return Node::Row(Vec::new());
    }

    if let Some(node) = split_fraction(items, rules, confidence) {
        return node;
    }
    horizontal(items, rules, confidence)
}

/// Splits a region at the widest fraction bar it contains.
///
/// The bar has to span most of the material it divides, which is what separates `\frac` from a
/// minus sign: both are horizontal rules in the content stream, and only the width tells them
/// apart.
fn split_fraction(items: &[Item], rules: &[Rect], confidence: &mut f32) -> Option<Node> {
    let size = dominant_size(items);

    let bar = rules
        .iter()
        .filter(|rule| {
            let above = items.iter().any(|i| i.bbox.y1 <= rule.y0 + 1.0);
            let below = items.iter().any(|i| i.bbox.y0 >= rule.y1 - 1.0);
            above && below && rule.width() >= size * MIN_FRACTION_WIDTH_RATIO
        })
        .max_by(|a, b| a.width().total_cmp(&b.width()))?;

    let (num, den): (Vec<Item>, Vec<Item>) = items
        .iter()
        .filter(|i| i.bbox.x_overlap(bar) > -size)
        .partition(|i| i.bbox.center_y() < bar.center_y());

    if num.is_empty() || den.is_empty() {
        return None;
    }

    // Material to the left or right of the bar is not part of the fraction.
    let outside: Vec<Item> = items
        .iter()
        .filter(|i| i.bbox.x_overlap(bar) <= -size)
        .copied()
        .collect();

    let inner = rules
        .iter()
        .filter(|r| r.center_y() != bar.center_y())
        .copied()
        .collect::<Vec<_>>();

    let fraction = Node::Fraction {
        num: Box::new(region(&num, &inner, confidence)),
        den: Box::new(region(&den, &inner, confidence)),
    };

    if outside.is_empty() {
        return Some(fraction);
    }

    // Keep left-to-right order around the fraction.
    let (left, right): (Vec<Item>, Vec<Item>) = outside.iter().partition(|i| i.bbox.x1 <= bar.x0);
    let mut row = Vec::new();
    if !left.is_empty() {
        row.push(horizontal(&left, &inner, confidence));
    }
    row.push(fraction);
    if !right.is_empty() {
        row.push(horizontal(&right, &inner, confidence));
    }
    Some(Node::Row(row))
}

/// Reconstructs a horizontal run: baseline material with scripts attached.
fn horizontal(items: &[Item], rules: &[Rect], confidence: &mut f32) -> Node {
    let size = dominant_size(items);
    let baseline = dominant_baseline(items, size);

    let mut nodes: Vec<Node> = Vec::new();
    let mut text = String::new();
    let mut i = 0;

    while i < items.len() {
        let item = items[i];

        // A radical claims everything under its overline, which extends to its right.
        if item.ch == '√' {
            flush(&mut nodes, &mut text);
            let (end, certain) = radical_extent(items, i);
            let arg: Vec<Item> = items[i + 1..end].to_vec();
            if arg.is_empty() {
                // A radical with nothing under it is a stray glyph, not a root. Emitting
                // `\sqrt{}` would be worse than emitting the symbol.
                penalise(confidence);
                text.push_str(&symbol_text(item.ch));
                i += 1;
                continue;
            }
            if !certain {
                penalise(confidence);
            }
            nodes.push(Node::Radical {
                arg: Box::new(region(&arg, rules, confidence)),
            });
            i = end;
            continue;
        }

        // A large operator claims the material directly above and below it.
        if is_large_operator(&item, size) {
            flush(&mut nodes, &mut text);
            let (above, below, consumed) = operator_limits(items, i);
            nodes.push(Node::Operator {
                symbol: symbol_text(item.ch),
                above: above.map(|a| Box::new(region(&a, rules, confidence))),
                below: below.map(|b| Box::new(region(&b, rules, confidence))),
            });
            i = consumed;
            continue;
        }

        // Otherwise: baseline character, possibly carrying scripts.
        let (sup, sub, next) = scripts(items, i, baseline, size);
        if sup.is_some() || sub.is_some() {
            flush(&mut nodes, &mut text);
            nodes.push(Node::Scripted {
                base: Box::new(Node::Symbol(symbol_text(item.ch))),
                sup: sup.map(|s| Box::new(region(&s, rules, confidence))),
                sub: sub.map(|s| Box::new(region(&s, rules, confidence))),
            });
            i = next;
            continue;
        }

        // A character with no LaTeX name and no ASCII spelling is one we cannot write down.
        if !item.ch.is_ascii() && super::symbols::latex(item.ch).is_none() {
            penalise(confidence);
        }

        // Plain character. Insert a space where the typesetting left one.
        if i > 0 {
            let gap = item.bbox.x0 - items[i - 1].bbox.x1;
            if gap > size * SPACE_RATIO {
                text.push(' ');
            }
        }
        text.push_str(&symbol_text(item.ch));
        i += 1;
    }

    flush(&mut nodes, &mut text);

    match nodes.len() {
        0 => Node::Row(Vec::new()),
        1 => nodes.pop().unwrap(),
        _ => Node::Row(nodes),
    }
}

fn flush(nodes: &mut Vec<Node>, text: &mut String) {
    if !text.is_empty() {
        nodes.push(Node::Symbol(std::mem::take(text)));
    }
}

/// The scripts attached to the glyph at `i`, and the index to continue from.
fn scripts(
    items: &[Item],
    i: usize,
    baseline: f32,
    size: f32,
) -> (Option<Vec<Item>>, Option<Vec<Item>>, usize) {
    let mut sup: Vec<Item> = Vec::new();
    let mut sub: Vec<Item> = Vec::new();
    let mut j = i + 1;

    while j < items.len() {
        let next = items[j];
        if next.size > size * SCRIPT_MAX_SIZE_RATIO {
            break;
        }
        let offset = baseline - next.baseline;
        if offset > size * SCRIPT_MIN_OFFSET {
            sup.push(next);
        } else if -offset > size * SCRIPT_MIN_OFFSET {
            sub.push(next);
        } else {
            break;
        }
        j += 1;
    }

    (
        (!sup.is_empty()).then_some(sup),
        (!sub.is_empty()).then_some(sub),
        if j > i + 1 { j } else { i + 1 },
    )
}

/// How far a radical's overline reaches, and whether that was observed or guessed.
///
/// The overline is drawn as part of the `√` glyph in Computer Modern rather than as a separate
/// path, so its extent is not directly observable. The glyph's own bounding box is the best
/// available evidence; where it covers nothing, the argument is taken to be the following symbol
/// alone — right for `\sqrt{n}`, wrong for `\sqrt{a+b}` — and the guess is reported.
fn radical_extent(items: &[Item], i: usize) -> (usize, bool) {
    let reach = items[i].bbox.x1;
    let mut end = i + 1;
    while end < items.len() && items[end].bbox.x0 < reach {
        end += 1;
    }
    if end > i + 1 {
        (end.min(items.len()), true)
    } else {
        ((i + 2).min(items.len()), false)
    }
}

fn is_large_operator(item: &Item, size: f32) -> bool {
    matches!(item.ch, '∑' | '∏' | '∐' | '∫' | '∬' | '∮' | '⋃' | '⋂')
        // Inline `\sum` is set at body size; a display one is grown.
        && item.bbox.height() > size * 1.1
}

/// The material a large operator carries above and below it.
fn operator_limits(items: &[Item], i: usize) -> (Option<Vec<Item>>, Option<Vec<Item>>, usize) {
    let operator = items[i];
    let mut above = Vec::new();
    let mut below = Vec::new();
    let mut j = i + 1;

    while j < items.len() {
        let next = items[j];
        // A limit sits within the operator's horizontal span, not after it.
        if next.bbox.x0 > operator.bbox.x1 {
            break;
        }
        if next.bbox.center_y() < operator.bbox.y0 {
            above.push(next);
        } else if next.bbox.center_y() > operator.bbox.y1 {
            below.push(next);
        } else {
            break;
        }
        j += 1;
    }

    (
        (!above.is_empty()).then_some(above),
        (!below.is_empty()).then_some(below),
        j.max(i + 1),
    )
}

fn symbol_text(c: char) -> String {
    match super::symbols::latex(c) {
        Some(latex) if latex.starts_with('\\') => format!("{latex} "),
        Some(literal) => literal.to_owned(),
        None => c.to_string(),
    }
}

/// The size most of the material is set at, weighted by glyph width.
fn dominant_size(items: &[Item]) -> f32 {
    let mut buckets: Vec<(f32, f32)> = Vec::new();
    for item in items {
        let weight = item.bbox.width().max(0.1);
        match buckets
            .iter_mut()
            .find(|(s, _)| (*s - item.size).abs() < 0.3)
        {
            Some((_, w)) => *w += weight,
            None => buckets.push((item.size, weight)),
        }
    }
    buckets
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(s, _)| *s)
        .unwrap_or(10.0)
}

/// The baseline of the full-size material.
fn dominant_baseline(items: &[Item], size: f32) -> f32 {
    let mut baselines: Vec<f32> = items
        .iter()
        .filter(|i| (i.size - size).abs() < 0.3)
        .map(|i| i.baseline)
        .collect();
    if baselines.is_empty() {
        baselines = items.iter().map(|i| i.baseline).collect();
    }
    crate::util::median(&mut baselines).unwrap_or(0.0)
}

/// Lowers confidence, saturating at zero.
pub fn penalise(confidence: &mut f32) {
    *confidence *= UNCERTAIN;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FontFlags, FontId, GlyphText, PathItem, Point, Rgba};

    fn glyph(c: char, x: f32, baseline: f32, size: f32) -> Glyph {
        let w = size * 0.5;
        Glyph {
            text: GlyphText::Char(c),
            bbox: Rect::from_corners(x, baseline - size * 0.7, x + w, baseline),
            origin: Point::new(x, baseline),
            font: FontId(0),
            size,
            angle: 0.0,
            flags: FontFlags::default(),
            color: Rgba::BLACK,
            generated: false,
        }
    }

    fn page_of(glyphs: Vec<Glyph>, paths: Vec<PathItem>) -> PageRaw {
        PageRaw {
            index: 0,
            width: 612.0,
            height: 792.0,
            rotation: 0,
            glyphs,
            paths,
            images: Vec::new(),
            expansions: Vec::new(),
        }
    }

    fn rule(x0: f32, y: f32, x1: f32) -> PathItem {
        PathItem {
            bbox: Rect::from_corners(x0, y, x1, y + 0.4),
            kind: PathKind::HorizontalRule,
            thickness: 0.4,
            color: Rgba::BLACK,
            filled: true,
            stroked: false,
        }
    }

    fn build_all(page: &PageRaw) -> Formula {
        let indices: Vec<usize> = (0..page.glyphs.len()).collect();
        build(page, &indices)
    }

    #[test]
    fn a_plain_run_is_one_symbol() {
        let page = page_of(
            vec![
                glyph('a', 100.0, 200.0, 10.0),
                glyph('+', 105.0, 200.0, 10.0),
                glyph('b', 110.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        assert_eq!(build_all(&page).root, Node::Symbol("a+b".into()));
    }

    #[test]
    fn a_superscript_is_recognised() {
        // `x` at 10pt, then `2` at 7pt raised by 4pt.
        let page = page_of(
            vec![
                glyph('x', 100.0, 200.0, 10.0),
                glyph('2', 105.0, 196.0, 7.0),
            ],
            Vec::new(),
        );
        let formula = build_all(&page);
        assert_eq!(
            formula.root,
            Node::Scripted {
                base: Box::new(Node::Symbol("x".into())),
                sup: Some(Box::new(Node::Symbol("2".into()))),
                sub: None,
            }
        );
    }

    #[test]
    fn a_subscript_is_recognised() {
        let page = page_of(
            vec![
                glyph('x', 100.0, 200.0, 10.0),
                glyph('i', 105.0, 202.5, 7.0),
            ],
            Vec::new(),
        );
        let formula = build_all(&page);
        match formula.root {
            Node::Scripted {
                sub: Some(sub),
                sup: None,
                ..
            } => {
                assert_eq!(*sub, Node::Symbol("i".into()));
            }
            other => panic!("expected a subscript, got {other:?}"),
        }
    }

    #[test]
    fn a_rule_between_material_is_a_fraction() {
        // `a` over `b`, with a bar between them.
        let page = page_of(
            vec![
                glyph('a', 100.0, 194.0, 10.0),
                glyph('b', 100.0, 210.0, 10.0),
            ],
            vec![rule(99.0, 197.0, 111.0)],
        );
        let formula = build_all(&page);
        assert_eq!(
            formula.root,
            Node::Fraction {
                num: Box::new(Node::Symbol("a".into())),
                den: Box::new(Node::Symbol("b".into())),
            }
        );
    }

    /// A minus sign is also a horizontal rule in some fonts. Width is what tells them apart.
    #[test]
    fn a_short_rule_is_not_a_fraction() {
        let page = page_of(
            vec![
                glyph('a', 100.0, 194.0, 10.0),
                glyph('b', 100.0, 210.0, 10.0),
            ],
            vec![rule(100.0, 197.0, 103.0)],
        );
        assert!(!matches!(build_all(&page).root, Node::Fraction { .. }));
    }

    #[test]
    fn a_large_operator_takes_limits() {
        // A grown sigma with `n` above and `i` below.
        let mut sigma = glyph('∑', 100.0, 205.0, 10.0);
        sigma.bbox = Rect::from_corners(100.0, 192.0, 112.0, 205.0);
        let page = page_of(
            vec![
                sigma,
                glyph('n', 103.0, 190.0, 7.0),
                glyph('i', 103.0, 214.0, 7.0),
            ],
            Vec::new(),
        );
        match build_all(&page).root {
            Node::Operator {
                symbol,
                above,
                below,
            } => {
                assert_eq!(symbol.trim(), r"\sum");
                assert_eq!(*above.expect("upper limit"), Node::Symbol("n".into()));
                assert_eq!(*below.expect("lower limit"), Node::Symbol("i".into()));
            }
            other => panic!("expected an operator, got {other:?}"),
        }
    }

    #[test]
    fn a_radical_claims_what_sits_under_it() {
        let mut root = glyph('√', 100.0, 200.0, 10.0);
        root.bbox = Rect::from_corners(100.0, 193.0, 118.0, 200.0);
        let page = page_of(
            vec![
                root,
                glyph('n', 106.0, 200.0, 10.0),
                glyph('+', 111.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        assert!(matches!(build_all(&page).root, Node::Radical { .. }));
    }

    #[test]
    fn symbols_are_named_in_latex() {
        let page = page_of(
            vec![
                glyph('α', 100.0, 200.0, 10.0),
                glyph('≤', 106.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        match build_all(&page).root {
            Node::Symbol(text) => {
                assert!(text.contains(r"\alpha"), "got {text:?}");
                assert!(text.contains(r"\leq"), "got {text:?}");
            }
            other => panic!("expected symbols, got {other:?}"),
        }
    }

    #[test]
    fn wide_gaps_become_spaces() {
        let page = page_of(
            vec![
                glyph('a', 100.0, 200.0, 10.0),
                glyph('b', 115.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        assert_eq!(build_all(&page).root, Node::Symbol("a b".into()));
    }

    #[test]
    fn an_unknown_symbol_lowers_confidence() {
        // A private-use codepoint: a grown-delimiter piece, which we cannot name.
        let page = page_of(
            vec![
                glyph('a', 100.0, 200.0, 10.0),
                glyph('\u{F8EE}', 106.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        let formula = build_all(&page);
        assert!(
            formula.confidence < 1.0,
            "an unnameable glyph must cost confidence"
        );
    }

    #[test]
    fn a_clean_formula_keeps_full_confidence() {
        let page = page_of(
            vec![
                glyph('x', 100.0, 200.0, 10.0),
                glyph('2', 105.0, 196.0, 7.0),
            ],
            Vec::new(),
        );
        assert_eq!(build_all(&page).confidence, 1.0);
    }

    #[test]
    fn a_guessed_radical_extent_lowers_confidence() {
        // A `√` whose box covers nothing after it, so its reach has to be guessed.
        let page = page_of(
            vec![
                glyph('√', 100.0, 200.0, 10.0),
                glyph('n', 106.0, 200.0, 10.0),
                glyph('+', 111.0, 200.0, 10.0),
            ],
            Vec::new(),
        );
        assert!(build_all(&page).confidence < 1.0);
    }

    #[test]
    fn an_empty_formula_is_handled() {
        let page = page_of(Vec::new(), Vec::new());
        assert_eq!(build_all(&page).root, Node::Row(Vec::new()));
    }
}
