//! Strips margin line numbers from Word-style preprints.
//!
//! bioRxiv and medRxiv manuscripts come out of Word with every body line numbered in the
//! left margin. The numbers share the body's baselines, so depending on how wide the gap
//! reads they either fuse into the body lines or survive column splitting as a column of
//! one-word lines — and both ways they leak into every output format as stray integers,
//! which a reader downstream then takes for data.
//!
//! The pass is deliberately strict, because a stray removal deletes a real number from the
//! text. A word is only a line number if it is a short bare integer standing in the margin
//! — fully outside the left or right edge that the page's own prose establishes — and a
//! margin column is only stripped when at least [`MIN_COUNT`] such numbers increase
//! strictly down the page, which is what a numbering is and almost nothing else is.
//! Bibliography markers sit at the body edge, not outside it; equation numbers carry
//! parentheses; a lone page number in the margin is one value, not a run.

use crate::ir::Rect;
use crate::text::lines::Line;

/// A margin numbering has at least this many numbers on a page. A title page whose
/// abstract is unnumbered still numbers its opening lines well past this.
const MIN_COUNT: usize = 5;

/// A fused margin number is separated from the body text by at least this many line sizes.
/// A justified word space compresses to about a fifth of an em; the gutter between a line
/// number and the text it labels is an order of magnitude wider.
const MIN_GAP_EMS: f32 = 1.5;

/// A margin number ends at least this far outside the prose edge, in points.
const MARGIN_CLEARANCE: f32 = 1.0;

/// A candidate margin number: a short bare integer at the edge of its line.
struct Candidate {
    line: usize,
    word: usize,
    only_word: bool,
    value: u32,
    baseline: f32,
    bbox: Rect,
}

/// Removes margin line numbers from every page, returning how many were stripped.
pub fn strip(pages: &mut [Vec<Line>]) -> usize {
    let mut stripped = 0;
    for page in pages {
        stripped += strip_page(page);
    }
    stripped
}

fn strip_page(lines: &mut Vec<Line>) -> usize {
    let Some((body_x0, body_x1)) = prose_margins(lines) else {
        return 0;
    };

    let mut left = Vec::new();
    let mut right = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for candidate in candidates(i, line) {
            if candidate.bbox.x1 <= body_x0 - MARGIN_CLEARANCE {
                left.push(candidate);
            } else if candidate.bbox.x0 >= body_x1 + MARGIN_CLEARANCE {
                right.push(candidate);
            }
        }
    }

    let mut stripped = 0;
    for mut side in [left, right] {
        side.sort_by(|a, b| a.baseline.total_cmp(&b.baseline));
        if side.len() < MIN_COUNT || !side.windows(2).all(|pair| pair[0].value < pair[1].value) {
            continue;
        }
        stripped += side.len();

        let mut drop_lines = Vec::new();
        for candidate in &side {
            if candidate.only_word {
                drop_lines.push(candidate.line);
            } else {
                remove_word(&mut lines[candidate.line], candidate.word);
            }
        }
        if !drop_lines.is_empty() {
            let mut index = 0;
            lines.retain(|_| {
                let keep = !drop_lines.contains(&index);
                index += 1;
                keep
            });
        }
    }
    stripped
}

/// The left and right edges of the page's prose, from lines that are unmistakably prose:
/// at least four words of three or more alphabetic characters. Digit-shaped words are left
/// out of the edges so a fused line number cannot vouch for its own margin.
fn prose_margins(lines: &[Line]) -> Option<(f32, f32)> {
    let mut x0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut found = false;
    for line in lines {
        let wordy = line
            .words
            .iter()
            .filter(|w| w.text.chars().filter(|c| c.is_alphabetic()).count() >= 3)
            .count();
        if wordy < 4 {
            continue;
        }
        for word in &line.words {
            if is_bare_integer(&word.text) {
                continue;
            }
            x0 = x0.min(word.bbox.x0);
            x1 = x1.max(word.bbox.x1);
            found = true;
        }
    }
    found.then_some((x0, x1))
}

fn is_bare_integer(text: &str) -> bool {
    !text.is_empty() && text.len() <= 4 && text.bytes().all(|b| b.is_ascii_digit())
}

/// The line-number candidates on one line: a bare integer that is the whole line, or its
/// first or last word with a margin-sized gap to the rest.
fn candidates(index: usize, line: &Line) -> Vec<Candidate> {
    let gap = line.size * MIN_GAP_EMS;
    let mut out = Vec::new();
    for (w, word) in line.words.iter().enumerate() {
        if !is_bare_integer(&word.text) {
            continue;
        }
        let only_word = line.words.len() == 1;
        let at_edge = only_word
            || (w == 0 && line.words[1].bbox.x0 - word.bbox.x1 >= gap)
            || (w == line.words.len() - 1 && word.bbox.x0 - line.words[w - 1].bbox.x1 >= gap);
        if !at_edge {
            continue;
        }
        let Ok(value) = word.text.parse() else {
            continue;
        };
        out.push(Candidate {
            line: index,
            word: w,
            only_word,
            value,
            baseline: line.baseline,
            bbox: word.bbox,
        });
    }
    out
}

/// Removes one word from a line, keeping the glyph indices of the words after it valid and
/// shrinking the line's box to the words that remain.
fn remove_word(line: &mut Line, index: usize) {
    let word = line.words.remove(index);
    let removed = word.end - word.start;
    line.glyphs.drain(word.start..word.end);
    for later in &mut line.words[index..] {
        later.start -= removed;
        later.end -= removed;
    }

    let mut bbox: Option<Rect> = None;
    for word in &line.words {
        bbox = Some(match bbox {
            None => word.bbox,
            Some(b) => Rect {
                x0: b.x0.min(word.bbox.x0),
                y0: b.y0.min(word.bbox.y0),
                x1: b.x1.max(word.bbox.x1),
                y1: b.y1.max(word.bbox.y1),
            },
        });
    }
    if let Some(b) = bbox {
        line.bbox = b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::lines::Word;

    fn word(text: &str, x0: f32, x1: f32, baseline: f32) -> Word {
        Word {
            bbox: Rect::from_corners(x0, baseline - 8.0, x1, baseline),
            text: text.to_owned(),
            start: 0,
            end: 0,
            scripts: Vec::new(),
        }
    }

    fn line_of(words: Vec<Word>, baseline: f32) -> Line {
        let mut bbox = words[0].bbox;
        for w in &words[1..] {
            bbox.x0 = bbox.x0.min(w.bbox.x0);
            bbox.x1 = bbox.x1.max(w.bbox.x1);
            bbox.y0 = bbox.y0.min(w.bbox.y0);
            bbox.y1 = bbox.y1.max(w.bbox.y1);
        }
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

    fn prose(baseline: f32) -> Line {
        line_of(
            vec![
                word("steady", 100.0, 130.0, baseline),
                word("readings", 134.0, 170.0, baseline),
                word("were", 174.0, 196.0, baseline),
                word("recorded", 200.0, 240.0, baseline),
            ],
            baseline,
        )
    }

    /// Standalone margin numbers on the body's baselines are removed as whole lines.
    #[test]
    fn a_column_of_standalone_numbers_is_stripped() {
        let mut pages = vec![(0..6)
            .flat_map(|i| {
                let y = 100.0 + 12.0 * i as f32;
                let n = (i + 1).to_string();
                [line_of(vec![word(&n, 80.0, 86.0, y)], y), prose(y)]
            })
            .collect::<Vec<_>>()];

        assert_eq!(strip(&mut pages), 6);
        assert_eq!(pages[0].len(), 6);
        assert!(pages[0].iter().all(|l| l.text().starts_with("steady")));
    }

    /// A number fused into its body line is removed as a word, not a line.
    #[test]
    fn a_fused_margin_number_is_removed_from_its_line() {
        let mut pages = vec![(0..5)
            .map(|i| {
                let y = 100.0 + 12.0 * i as f32;
                let n = (i + 1).to_string();
                let mut words = vec![word(&n, 78.0, 84.0, y)];
                words.extend(prose(y).words);
                line_of(words, y)
            })
            .collect::<Vec<_>>()];

        assert_eq!(strip(&mut pages), 5);
        assert_eq!(pages[0].len(), 5);
        for line in &pages[0] {
            assert_eq!(line.text(), "steady readings were recorded");
            assert!((line.bbox.x0 - 100.0).abs() < 0.01, "bbox not shrunk");
        }
    }

    /// Four numbers are not a numbering: a short run stays, so a stray figure label or a
    /// page number is never deleted.
    #[test]
    fn a_short_run_is_left_alone() {
        let mut pages = vec![(0..4)
            .flat_map(|i| {
                let y = 100.0 + 12.0 * i as f32;
                let n = (i + 1).to_string();
                [line_of(vec![word(&n, 80.0, 86.0, y)], y), prose(y)]
            })
            .collect::<Vec<_>>()];

        assert_eq!(strip(&mut pages), 0);
        assert_eq!(pages[0].len(), 8);
    }

    /// Numbers that do not increase down the page are data, not a numbering.
    #[test]
    fn non_increasing_numbers_are_left_alone() {
        let values = [3u32, 1, 4, 1, 5, 9];
        let mut pages = vec![values
            .iter()
            .enumerate()
            .flat_map(|(i, v)| {
                let y = 100.0 + 12.0 * i as f32;
                [
                    line_of(vec![word(&v.to_string(), 80.0, 86.0, y)], y),
                    prose(y),
                ]
            })
            .collect::<Vec<_>>()];

        assert_eq!(strip(&mut pages), 0);
    }

    /// Numbers at the body's own left edge — bibliography markers, list numbers — are
    /// inside the prose margin and never candidates.
    #[test]
    fn numbers_at_the_body_edge_are_left_alone() {
        let mut pages = vec![(0..6)
            .flat_map(|i| {
                let y = 100.0 + 12.0 * i as f32;
                let n = (i + 1).to_string();
                [line_of(vec![word(&n, 100.0, 106.0, y)], y), prose(y)]
            })
            .collect::<Vec<_>>()];

        assert_eq!(strip(&mut pages), 0);
    }
}
