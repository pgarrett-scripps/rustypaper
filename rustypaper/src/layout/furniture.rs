//! Removing running heads, folios and watermarks.
//!
//! Page furniture is the text that belongs to the *publication* rather than the document:
//! journal names along the top of every page, page numbers, conference footers, preprint
//! watermarks. It is cheap to find — it repeats in the same place on most pages — and removing
//! it strips a large fraction of the noise that generic extractors ship.
//!
//! Two conditions must both hold, which keeps genuinely repeated body text safe: the line has to
//! sit in a margin band, and its signature has to recur on most pages.

use std::collections::HashMap;

use crate::text::lines::Line;

/// Fraction of the page height at the top and bottom within which furniture can live.
const MARGIN_BAND: f32 = 0.12;

/// A signature must appear on at least this fraction of pages to count as furniture.
const MIN_PAGE_FRACTION: f32 = 0.5;

/// Below this many pages there is no evidence of repetition worth acting on.
const MIN_PAGES: usize = 4;

/// Drops page furniture from each page's lines, returning how many lines were removed.
pub fn strip(pages: &mut [Vec<Line>], page_heights: &[f32]) -> usize {
    if pages.len() < MIN_PAGES {
        return 0;
    }

    // Count the *pages* a signature appears on, not its total occurrences, so a line repeated
    // several times on one page cannot vote itself out.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (lines, &height) in pages.iter().zip(page_heights) {
        let mut on_this_page: Vec<String> = lines
            .iter()
            .filter(|l| in_margin(l, height))
            .map(signature)
            .collect();
        on_this_page.sort();
        on_this_page.dedup();
        for key in on_this_page {
            *seen.entry(key).or_default() += 1;
        }
    }

    let quorum = (pages.len() as f32 * MIN_PAGE_FRACTION).ceil() as usize;
    let mut removed = 0;

    for (lines, &height) in pages.iter_mut().zip(page_heights) {
        lines.retain(|line| {
            let furniture =
                in_margin(line, height) && seen.get(&signature(line)).is_some_and(|&n| n >= quorum);
            removed += usize::from(furniture);
            !furniture
        });
    }

    removed
}

fn in_margin(line: &Line, page_height: f32) -> bool {
    if page_height <= 0.0 {
        return false;
    }
    let y = line.baseline / page_height;
    y <= MARGIN_BAND || y >= 1.0 - MARGIN_BAND
}

/// A line's identity for repetition purposes.
///
/// Digits collapse to `#` so that folios (`3`, `4`, `5`) and running heads carrying a page or
/// section number all share one signature. Case and spacing are normalised for the same reason.
fn signature(line: &Line) -> String {
    let mut out = String::new();
    let mut last_was_digit = false;
    for c in line.text().chars() {
        if c.is_ascii_digit() {
            if !last_was_digit {
                out.push('#');
            }
            last_was_digit = true;
        } else if c.is_whitespace() {
            last_was_digit = false;
        } else {
            out.extend(c.to_lowercase());
            last_was_digit = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Rect;
    use crate::text::lines::{Line, Word};

    const HEIGHT: f32 = 792.0;

    fn line(text: &str, baseline: f32) -> Line {
        let bbox = Rect::from_corners(72.0, baseline - 10.0, 540.0, baseline);
        Line {
            bbox,
            baseline,
            size: 10.0,
            bold: false,
            italic: false,
            glyphs: Vec::new(),
            words: vec![Word {
                bbox,
                text: text.to_owned(),
                start: 0,
                end: 1,
            }],
        }
    }

    fn document(n: usize) -> (Vec<Vec<Line>>, Vec<f32>) {
        let pages = (0..n)
            .map(|i| {
                vec![
                    line("Preprint. Under review.", 40.0),
                    line("body text that differs per page", 300.0),
                    line(&format!("{}", i + 1), 760.0),
                ]
            })
            .collect();
        (pages, vec![HEIGHT; n])
    }

    #[test]
    fn running_heads_and_folios_are_removed() {
        let (mut pages, heights) = document(8);
        let removed = strip(&mut pages, &heights);
        assert_eq!(removed, 16, "expected one head and one folio per page");
        for page in &pages {
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].text(), "body text that differs per page");
        }
    }

    #[test]
    fn page_numbers_collapse_to_one_signature_despite_differing() {
        let (mut pages, heights) = document(8);
        strip(&mut pages, &heights);
        assert!(
            pages.iter().all(|p| p.iter().all(|l| l.text() != "1")),
            "folios differ per page and must still be recognised as one signature"
        );
    }

    #[test]
    fn body_text_in_the_middle_of_the_page_is_never_furniture() {
        // The same line in the body band on every page: repeated, but not in a margin.
        let mut pages: Vec<Vec<Line>> = (0..8)
            .map(|_| vec![line("identical body", 400.0)])
            .collect();
        let heights = vec![HEIGHT; 8];
        assert_eq!(strip(&mut pages, &heights), 0);
        assert!(pages.iter().all(|p| p.len() == 1));
    }

    #[test]
    fn a_margin_line_appearing_once_is_kept() {
        let (mut pages, heights) = document(8);
        pages[0].push(line("Accepted at NeurIPS 2024", 55.0));
        strip(&mut pages, &heights);
        assert!(
            pages[0].iter().any(|l| l.text().starts_with("Accepted")),
            "a one-off margin note is not furniture"
        );
    }

    #[test]
    fn short_documents_are_left_alone() {
        let (mut pages, heights) = document(3);
        assert_eq!(strip(&mut pages, &heights), 0);
        assert!(pages.iter().all(|p| p.len() == 3));
    }
}
